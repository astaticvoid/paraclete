use std::borrow::Cow;
use std::collections::HashMap;

use paraclete_node_api::{
    AffordanceHint, CapabilityDocument, DebugEventKind, EnvelopeGroup, Event, Node, PageRef,
    MachineVariant, ParamDescriptor, ParamOverlay, ParamUnit, ParameterBank, PortDescriptor,
    PortDirection, PortType,
    ProcessInput, ProcessOutput, Rule, StateBusValue, UmpMessage, ViewPlugin,
    midi::ChannelVoice2, CMD_TRIGGER,
};

use crate::engine_dsp::{
    AdState, LfoDestLabels, LfoHost, LfoMode, LfoSettings, LfoShape, LFO_PAGE_ORDER,
    lfo_params, note_to_hz,
    soft_clip, sub_blocks,
};

fn fp(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

// ── Machine variant ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FmMachine { Kick, Bell, Bass }

impl FmMachine {
    /// Declaration order, and therefore the `machine` param's value order.
    /// **Append-only** — a saved project stores the numeric value, so
    /// reordering silently re-points every stored `machine` at another engine.
    pub const ALL: [FmMachine; 3] = [FmMachine::Kick, FmMachine::Bell, FmMachine::Bass];

    pub fn value(self) -> u32 {
        match self {
            FmMachine::Kick => 0,
            FmMachine::Bell => 1,
            FmMachine::Bass => 2,
        }
    }

    /// Out-of-range clamps to the last machine rather than panicking — this
    /// reads an `f64` bank slot a malformed project could carry.
    pub fn from_value(v: u32) -> Self {
        *Self::ALL.get(v as usize).unwrap_or(&FmMachine::Bass)
    }

    pub fn doc_name(self) -> &'static str {
        match self {
            FmMachine::Kick => "FmKick",
            FmMachine::Bell => "FmBell",
            FmMachine::Bass => "FmBass",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            FmMachine::Kick => "FM Kick",
            FmMachine::Bell => "FM Bell",
            FmMachine::Bass => "FM Bass",
        }
    }
}

/// MM §0 D1, as amended by MM-C3: ramp to silence, then swap. Bidirectional,
/// because a cancelled or retargeted switch must return to unity continuously.
const MACHINE_SWITCH_FADE_SECS: f32 = 0.005;

/// See `AnalogEngine`'s copy — same contract, same reasoning.
#[derive(Clone, Copy, Debug)]
struct SwitchFade {
    remaining: u32,
    /// `Some` = fading out toward this machine; `None` = a cancelled switch
    /// ramping back to unity, which swaps nothing.
    target: Option<FmMachine>,
}

// ── FmEngine ──────────────────────────────────────────────────────────────────

/// Two-operator FM synthesizer (phase modulation). Three machine variants.
/// Topology: modulator output → scales carrier phase. Modulator has self-feedback.
/// Ports: events_in (0, Event), audio_out_l (1, Mono), audio_out_r (2, Mono).
pub struct FmEngine {
    machine:        FmMachine,
    bank:           ParameterBank,
    sample_rate:    f32,

    carrier_phase:   f32,
    modulator_phase: f32,
    prev_mod_out:    f32,

    pitch_env: AdState,
    mod_env:   AdState,
    amp_env:   AdState,

    current_hz: f32,
    active:     bool,
    node_id:    u32,
    /// Note of the last retrigger — used as the CMD_TRIGGER default note (arg0 < 0).
    last_note:  u8,
    /// Linear output-level multiplier derived from trigger velocity (0.0..=1.0).
    /// 1.0 = unity gain (full velocity, matches pre-W1 output level).
    velocity_level: f32,
    /// ParamLock overrides for the note in flight (ADR-019 — locks must never
    /// mutate the bank). Retired at the next trigger, not each `process()`;
    /// see `AnalogEngine::consume_pending_locks` for why (#169).
    node_locks: Vec<(u32, f64)>,
    /// #169 (BUG-063): set when a `ParamLock` arrives, consumed by the
    /// `retrigger()` it belongs to.
    locks_pending: bool,

    /// In-flight gain ramp for a machine switch (MM §0 D1).
    switch_fade: Option<SwitchFade>,

    /// MM-C9: one LFO per hosting node (ADR-042 decision 1).
    lfo: LfoHost,

    render_l: Vec<f32>,
    render_r: Vec<f32>,

    pending_initial_params: HashMap<String, f64>,

    ports: [PortDescriptor; 3],
}

/// The dest table as **names**, in the same order as `FmEngine::LFO_DESTS`.
///
/// Two lists rather than one because `ParamDescriptor::id_for_name` is a
/// `const fn` over a `&str` but there is no stable way to map an array of
/// names to an array of ids in a `const` initialiser. A test asserts they
/// correspond entry-for-entry, so a drift fails loudly rather than mislabelling
/// an encoder.
/// #179: no longer the offered list, and there is no union label adapter any
/// more — the cap-doc carries the ACTIVE machine's labels
/// (`FmEngine::dest_labels`), because a machine-invariant list named
/// destinations the machine could not reach. This survives as the union
/// *envelope*: the set every per-machine table must draw from, which is what
/// keeps `LFO_DESTS.len()` — the width `lfo_dest` persists at — honest.
/// Referenced by the append-only tests.
#[allow(dead_code)]
static FM_DEST_NAMES: &[&str] = &["tune", "decay", "ratio", "index", "feedback", "drive", "punch", "attack"];


// ── Per-machine destination tables (#179) ────────────────────────────────────
//
// **Each list is APPEND ONLY** — `lfo_dest` persists a one-based index into
// the active machine's list. See `AnalogEngine`'s equivalent for the full
// contract; `the_per_machine_dest_tables_are_append_only` and
// `every_machines_dests_are_exactly_its_params` hold both families to it.
static FM_KICK_DEST_NAMES: &[&str] = &["tune", "punch", "decay", "feedback", "drive"];
static FM_BELL_DEST_NAMES: &[&str] = &["tune", "ratio", "index", "decay", "feedback"];
static FM_BASS_DEST_NAMES: &[&str] = &["tune", "ratio", "index", "attack", "decay", "drive"];

// Per-machine **id** tables, mirroring the name tables above — see
// `AnalogEngine::KICK_DEST_IDS` for the contract. `lfo_dest_id` reads these
// on the audio thread instead of hashing a name per sub-block.
// APPEND ONLY, same contract as the names they mirror.
const FM_KICK_DEST_IDS: &[u32] = &[
    ParamDescriptor::id_for_name("tune"),
    ParamDescriptor::id_for_name("punch"),
    ParamDescriptor::id_for_name("decay"),
    ParamDescriptor::id_for_name("feedback"),
    ParamDescriptor::id_for_name("drive"),
];
const FM_BELL_DEST_IDS: &[u32] = &[
    ParamDescriptor::id_for_name("tune"),
    ParamDescriptor::id_for_name("ratio"),
    ParamDescriptor::id_for_name("index"),
    ParamDescriptor::id_for_name("decay"),
    ParamDescriptor::id_for_name("feedback"),
];
const FM_BASS_DEST_IDS: &[u32] = &[
    ParamDescriptor::id_for_name("tune"),
    ParamDescriptor::id_for_name("ratio"),
    ParamDescriptor::id_for_name("index"),
    ParamDescriptor::id_for_name("attack"),
    ParamDescriptor::id_for_name("decay"),
    ParamDescriptor::id_for_name("drive"),
];

// Per-machine (id, min, max) range tables, for the LFO's scale and clamp
// (BUG-069) — see `AnalogEngine::KICK_DEST_RANGES` for the full contract.
// APPEND ONLY, same contract as the id tables;
// `the_lfo_range_tables_match_machine_params` pins every entry against
// `machine_params` so the two copies cannot drift.
const FM_KICK_DEST_RANGES: &[(u32, f32, f32)] = &[
    (ParamDescriptor::id_for_name("tune"),     -24.0, 24.0),
    (ParamDescriptor::id_for_name("punch"),      0.0, 1.0),
    (ParamDescriptor::id_for_name("decay"),      0.01, 2.0),
    (ParamDescriptor::id_for_name("feedback"),   0.0, 1.0),
    (ParamDescriptor::id_for_name("drive"),      0.0, 1.0),
];
const FM_BELL_DEST_RANGES: &[(u32, f32, f32)] = &[
    (ParamDescriptor::id_for_name("tune"),     -24.0, 24.0),
    (ParamDescriptor::id_for_name("ratio"),      0.5, 8.0),
    (ParamDescriptor::id_for_name("index"),      0.0, 8.0),
    (ParamDescriptor::id_for_name("decay"),      0.05, 8.0),
    (ParamDescriptor::id_for_name("feedback"),   0.0, 0.5),
];
const FM_BASS_DEST_RANGES: &[(u32, f32, f32)] = &[
    (ParamDescriptor::id_for_name("tune"),     -24.0, 24.0),
    (ParamDescriptor::id_for_name("ratio"),      0.5, 4.0),
    (ParamDescriptor::id_for_name("index"),      0.0, 8.0),
    (ParamDescriptor::id_for_name("attack"),     0.001, 0.5),
    (ParamDescriptor::id_for_name("decay"),      0.05, 4.0),
    (ParamDescriptor::id_for_name("drive"),      0.0, 1.0),
];

static FM_KICK_DEST_LABELS: LfoDestLabels = LfoDestLabels(FM_KICK_DEST_NAMES);
static FM_BELL_DEST_LABELS: LfoDestLabels = LfoDestLabels(FM_BELL_DEST_NAMES);
static FM_BASS_DEST_LABELS: LfoDestLabels = LfoDestLabels(FM_BASS_DEST_NAMES);

impl FmEngine {
    pub const PORT_EVENTS_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;

    pub fn new(machine: FmMachine) -> Self {
        let doc = Self::build_doc(machine);
        Self {
            machine,
            bank:           ParameterBank::from_capability_document(&doc),
            sample_rate:    44100.0,
            carrier_phase:   0.0,
            modulator_phase: 0.0,
            prev_mod_out:    0.0,
            pitch_env: AdState::new(),
            mod_env:   AdState::new(),
            amp_env:   AdState::new(),
            current_hz: 65.41, // C2
            active:     false,
            node_id:    0,
            last_note:  36, // C2 — matches current_hz's initial value
            velocity_level: 1.0,
            node_locks: Vec::new(),
            locks_pending: false,
            switch_fade: None,
            lfo:            LfoHost::new(),
            render_l:   Vec::new(),
            render_r:   Vec::new(),
            pending_initial_params: HashMap::new(),
            ports: Self::default_ports(),
        }
    }

    pub fn kick() -> Self { Self::new(FmMachine::Kick) }
    pub fn bell() -> Self { Self::new(FmMachine::Bell) }
    pub fn bass() -> Self { Self::new(FmMachine::Bass) }

    /// **APPEND ONLY** — see `AnalogEngine::LFO_DESTS` for the full contract.
    /// `lfo_dest` stores a one-based index into this, so a reorder re-points
    /// every saved patch. `lfo_*` and `machine` are absent by construction.
    const LFO_DESTS: &'static [u32] = &[
        ParamDescriptor::id_for_name("tune"),
        ParamDescriptor::id_for_name("decay"),
        ParamDescriptor::id_for_name("ratio"),
        ParamDescriptor::id_for_name("index"),
        ParamDescriptor::id_for_name("feedback"),
        ParamDescriptor::id_for_name("drive"),
        ParamDescriptor::id_for_name("punch"),
        ParamDescriptor::id_for_name("attack"),
    ];

    /// The LFO's contribution to `tune`, in semitones, and `current_hz` with
    /// it applied — see `AnalogEngine::lfo_tune_semitones` / `swept_hz` for
    /// why `tune` is swept here rather than re-read per span (#175).
    fn lfo_tune_semitones(&self) -> f32 {
        let base = self.raw_param(fp("tune"));
        self.lfo.apply(fp("tune"), base) - base
    }

    fn swept_hz(&self) -> f32 {
        let semis = self.lfo_tune_semitones();
        if semis == 0.0 {
            return self.current_hz;
        }
        self.current_hz * 2.0f32.powf(semis / 12.0)
    }

    /// See `AnalogEngine::push_lock` (#169).
    fn push_lock(&mut self, param_id: u32, value: f64) {
        if !self.locks_pending {
            self.node_locks.clear();
            self.locks_pending = true;
        }
        self.node_locks.push((param_id, value));
    }

    /// See `AnalogEngine::consume_pending_locks` (#169).
    fn consume_pending_locks(&mut self) {
        if self.locks_pending {
            self.locks_pending = false;
        } else {
            self.node_locks.clear();
        }
    }

    /// Bank/lock value, **before** the LFO — see `AnalogEngine::raw_param`.
    fn raw_param(&self, param_id: u32) -> f32 {
        for &(id, val) in &self.node_locks {
            if id == param_id { return val as f32; }
        }
        self.bank.get(param_id) as f32
    }

    /// Parameter read honoring per-cycle ParamLock overrides (ADR-019) **and**
    /// the LFO (ADR-042 amendment 1) — see `AnalogEngine::get_param`.
    fn get_param(&self, param_id: u32) -> f32 {
        self.lfo.apply(param_id, self.raw_param(param_id))
    }

    fn lfo_settings(&self) -> LfoSettings {
        LfoSettings {
            shape: LfoShape::from_value(self.raw_param(fp("lfo_shape"))),
            mode: LfoMode::from_value(self.raw_param(fp("lfo_mode"))),
            speed_hz: self.raw_param(fp("lfo_speed")),
            start_phase: self.raw_param(fp("lfo_start_phase")),
            fade: self.raw_param(fp("lfo_fade")),
        }
    }

    /// One-based index into `LFO_DESTS`; 0 and out-of-range read as off.
    /// The destination names offered on `machine` (#179), in index order.
    fn dest_names(machine: FmMachine) -> &'static [&'static str] {
        match machine {
            FmMachine::Kick => FM_KICK_DEST_NAMES,
            FmMachine::Bell => FM_BELL_DEST_NAMES,
            FmMachine::Bass => FM_BASS_DEST_NAMES,
        }
    }

    /// The value-indexed `lfo_dest` labels for `machine`, for
    /// `MachineVariant::param_labels` — see
    /// `AnalogEngine::dest_param_labels` for the contract.
    fn dest_param_labels(machine: FmMachine) -> Vec<Option<Cow<'static, str>>> {
        let names = Self::dest_names(machine);
        let mut out: Vec<Option<Cow<'static, str>>> = Vec::with_capacity(Self::LFO_DESTS.len() + 1);
        out.push(Some(Cow::Borrowed("off")));
        out.extend(names.iter().map(|n| Some(Cow::Borrowed(*n))));
        while out.len() <= Self::LFO_DESTS.len() {
            out.push(None);
        }
        out
    }

    /// The same tables as [`Self::dest_names`], as compile-time ids. The
    /// audio thread reads these; `dest_names` feeds labels and the append-only
    /// pins.
    fn dest_ids(machine: FmMachine) -> &'static [u32] {
        match machine {
            FmMachine::Kick => FM_KICK_DEST_IDS,
            FmMachine::Bell => FM_BELL_DEST_IDS,
            FmMachine::Bass => FM_BASS_DEST_IDS,
        }
    }

    /// The active machine's declared (id, min, max) for each destination
    /// (BUG-069). The LFO scales and clamps against these instead of the
    /// bank's union range — see `AnalogEngine::KICK_DEST_RANGES` for why.
    fn dest_ranges(machine: FmMachine) -> &'static [(u32, f32, f32)] {
        match machine {
            FmMachine::Kick => FM_KICK_DEST_RANGES,
            FmMachine::Bell => FM_BELL_DEST_RANGES,
            FmMachine::Bass => FM_BASS_DEST_RANGES,
        }
    }

    fn dest_labels(machine: FmMachine) -> &'static LfoDestLabels {
        match machine {
            FmMachine::Kick => &FM_KICK_DEST_LABELS,
            FmMachine::Bell => &FM_BELL_DEST_LABELS,
            FmMachine::Bass => &FM_BASS_DEST_LABELS,
        }
    }

    /// Resolve `lfo_dest` against the **active machine's** table (#179).
    /// One-based; an index past the machine's list reads as off, which is
    /// also what a dest belonging to a longer-listed machine does after a
    /// switch — see `AnalogEngine::apply_machine_switch`.
    ///
    /// Zero-cost slice read through `FM_KICK_DEST_IDS` &c — no name hash on
    /// the audio thread.
    fn lfo_dest_id(&self) -> Option<u32> {
        let v = self.raw_param(fp("lfo_dest"));
        // Non-integral values read as off, not truncate — see
        // `AnalogEngine::lfo_dest_id` for the reasoning (p-locks bypass the
        // bank's clamp, so this path can carry a malformed value).
        if !v.is_finite() || v < 1.0 || v.fract() != 0.0 {
            return None;
        }
        Self::dest_ids(self.machine)
            .get(v as usize - 1)
            .copied()
    }

    fn update_lfo(&mut self, samples: usize) {
        let dest = self.lfo_dest_id();
        // BUG-069: the range is the ACTIVE machine's declared (min, max), not
        // the bank's union — see `AnalogEngine::update_lfo` for why. Only
        // meaningful when there IS a destination; `update` ignores the range
        // when `dest` is `None`.
        let range = dest
            .and_then(|d| {
                Self::dest_ranges(self.machine)
                    .iter()
                    .find(|(id, _, _)| *id == d)
                    .map(|&(_, lo, hi)| (lo, hi))
            })
            .unwrap_or((0.0, 1.0));
        let depth = self.raw_param(fp("lfo_depth"));
        let settings = self.lfo_settings();
        let sr = self.sample_rate;
        self.lfo.update(settings, dest, range, depth, sr, samples);
    }

    /// The params one machine actually reads. **Not** what the bank stores —
    /// see `union_params`.
    fn machine_params(machine: FmMachine) -> Vec<ParamDescriptor> {
        match machine {
            FmMachine::Kick => vec![
                ParamDescriptor { id: fp("tune"),     name: "tune".into(),     min: -24.0, max: 24.0, default: 0.0, stepped: false, in_kit: true,  unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: fp("punch"),    name: "punch".into(),    min: 0.0,   max: 1.0,  default: 0.7, stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("decay"),    name: "decay".into(),    min: 0.01,  max: 2.0,  default: 0.5, stepped: false, in_kit: true,  unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("feedback"), name: "feedback".into(), min: 0.0,   max: 1.0,  default: 0.2, stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("drive"),    name: "drive".into(),    min: 0.0,   max: 1.0,  default: 0.0, stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
            ],
            FmMachine::Bell => vec![
                ParamDescriptor { id: fp("tune"),     name: "tune".into(),     min: -24.0, max: 24.0, default: 0.0,  stepped: false, in_kit: true,  unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: fp("ratio"),    name: "ratio".into(),    min: 0.5,   max: 8.0,  default: 3.5,  stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("index"),    name: "index".into(),    min: 0.0,   max: 8.0,  default: 2.0,  stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("decay"),    name: "decay".into(),    min: 0.05,  max: 8.0,  default: 2.0,  stepped: false, in_kit: true,  unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("feedback"), name: "feedback".into(), min: 0.0,   max: 0.5,  default: 0.1,  stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
            ],
            FmMachine::Bass => vec![
                ParamDescriptor { id: fp("tune"),  name: "tune".into(),  min: -24.0, max: 24.0, default: 0.0,  stepped: false, in_kit: true,  unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: fp("ratio"), name: "ratio".into(), min: 0.5,   max: 4.0,  default: 1.0,  stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("index"), name: "index".into(), min: 0.0,   max: 8.0,  default: 2.0,  stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: fp("attack"),name: "attack".into(),min: 0.001, max: 0.5,  default: 0.01, stepped: false, in_kit: true,  unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("decay"), name: "decay".into(), min: 0.05,  max: 4.0,  default: 0.5,  stepped: false, in_kit: true,  unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: fp("drive"), name: "drive".into(), min: 0.0,   max: 1.0,  default: 0.0,  stepped: false, in_kit: true,  unit: ParamUnit::Generic,   display: None },
            ],
        }
    }

    /// The bank's parameter set: **every machine's params merged at the widest
    /// envelope**, plus `machine` itself (ADR-041 §0 A1).
    ///
    /// `active` picks each param's *default* and nothing else. Ranges are the
    /// union unconditionally, for the lifetime of the node — narrowing them to
    /// the active machine truncates storage on load, which is the phase's one
    /// unrecoverable mistake (MM §3.4). Per-machine ranges live in
    /// `MachineVariant::overlays`; this engine never sees them.
    ///
    /// FmEngine is where the widest-envelope rule actually bites: `decay`
    /// spans 0.01-2.0 / 0.05-8.0 / 0.05-4.0 across the three machines and
    /// `feedback` 0-1.0 / 0-0.5, so a Bell patch's 6-second decay only
    /// survives selecting Kick and coming back because the bank holds the
    /// union.
    fn union_params(active: FmMachine) -> Vec<ParamDescriptor> {
        let mut out: Vec<ParamDescriptor> = vec![ParamDescriptor {
            id: fp("machine"),
            name: "machine".into(),
            min: 0.0,
            max: (FmMachine::ALL.len() - 1) as f64,
            default: active.value() as f64,
            stepped: true,
            in_kit: false,
            unit: ParamUnit::Generic,
            display: None,
        }];

        // MM-C9: machine-invariant, so they join the union once.
        // #179: union WIDTH (never narrowed — see this function's header),
        // active machine's LABELS. `machine_overlays` narrows what the encoder
        // can reach; indices between the two are gaps by construction.
        out.extend(lfo_params(
            Self::LFO_DESTS.len(),
            Some(Self::dest_labels(active)),
        ));

        for m in FmMachine::ALL {
            for p in Self::machine_params(m) {
                match out.iter_mut().find(|q| q.id == p.id) {
                    Some(q) => {
                        q.min = q.min.min(p.min);
                        q.max = q.max.max(p.max);
                        if m == active {
                            q.default = p.default;
                        }
                    }
                    None => out.push(p),
                }
            }
        }
        out
    }

    /// The node's ports, as **one** definition shared by the struct field and
    /// the capability document.
    ///
    /// #160 (BUG-057): `build_doc` used to set `ports: vec![]` while
    /// `Node::ports()` returned the real list, so the cap-doc and the node
    /// disagreed. Chain derivation reads the *cap-doc*
    /// (`main.rs` `is_audio_out`), so it never left the engine and
    /// `CompositeView::chain` was empty for every track in the app — no
    /// per-track effect could appear in a track's pages at all. Invisible
    /// until `instrument-fx.yaml` became the first fixture to wire one.
    fn default_ports() -> [PortDescriptor; 3] {
        [
            PortDescriptor { id: Self::PORT_EVENTS_IN,   name: "events_in".into(),   direction: PortDirection::Input,  port_type: PortType::Event },
            PortDescriptor { id: Self::PORT_AUDIO_OUT_L, name: "audio_out_l".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            PortDescriptor { id: Self::PORT_AUDIO_OUT_R, name: "audio_out_r".into(), direction: PortDirection::Output, port_type: PortType::Audio },
        ]
    }

    fn build_doc(machine: FmMachine) -> CapabilityDocument {
        CapabilityDocument {
            // The active machine's name; touches no range.
            name: machine.doc_name().into(),
            vendor: "Paraclete".into(),
            version: (0, 6, 0),
            ports: Self::default_ports().to_vec(),
            params: Self::union_params(machine),
            extensions: vec!["paraclete.instrument".into()],
            view: None,
        }
    }

    /// Has the `machine` param been moved? Called at the block boundary.
    ///
    /// Reads the **bank**, not `get_param` — a `ParamLock` on `machine` must
    /// never switch machines mid-step (ADR-041 decision 6).
    fn poll_machine_param(&mut self) {
        let target = FmMachine::from_value(self.bank.get(fp("machine")).max(0.0) as u32);
        let total = self.fade_len();

        if target == self.machine {
            if let Some(f) = self.switch_fade {
                if f.target.is_some() {
                    self.switch_fade = Some(SwitchFade {
                        remaining: total.saturating_sub(f.remaining).max(1),
                        target: None,
                    });
                }
            }
            return;
        }

        if self.switch_fade.map(|f| f.target) == Some(Some(target)) {
            return;
        }
        if !self.active {
            self.apply_machine_switch(target);
            self.switch_fade = None;
            return;
        }

        let remaining = match self.switch_fade {
            Some(f) if f.target.is_some() => f.remaining.min(total),
            Some(f) => total.saturating_sub(f.remaining).max(1),
            None => total,
        };
        self.switch_fade = Some(SwitchFade { remaining, target: Some(target) });
    }

    fn fade_len(&self) -> u32 {
        (MACHINE_SWITCH_FADE_SECS * self.sample_rate).max(1.0) as u32
    }

    /// Ramp the rendered block, and once a fade-out reaches silence, swap.
    fn apply_switch_fade(&mut self, block_size: usize) {
        let Some(fade) = self.switch_fade else {
            return;
        };
        let total = self.fade_len() as f32;
        let fading_out = fade.target.is_some();
        let mut left = fade.remaining;

        for i in 0..block_size.min(self.render_l.len()) {
            if left == 0 {
                if fading_out {
                    self.render_l[i] = 0.0;
                    self.render_r[i] = 0.0;
                }
                continue;
            }
            let g = if fading_out {
                left as f32 / total
            } else {
                1.0 - (left as f32 / total)
            };
            self.render_l[i] *= g;
            self.render_r[i] *= g;
            left -= 1;
        }

        if left == 0 {
            if let Some(target) = fade.target {
                self.apply_machine_switch(target);
            }
            self.switch_fade = None;
        } else {
            self.switch_fade = Some(SwitchFade { remaining: left, target: fade.target });
        }
    }

    /// ADR-041 decision 4: voice state resets on switch.
    ///
    /// **The bank is not rebuilt** — that is `activate()`'s job and it resets
    /// every slot to defaults, the same cross-machine data loss the union bank
    /// exists to prevent, by another route (MM §3.5).
    fn apply_machine_switch(&mut self, target: FmMachine) {
        self.machine = target;
        self.carrier_phase = 0.0;
        self.modulator_phase = 0.0;
        self.prev_mod_out = 0.0;
        self.pitch_env = AdState::new();
        self.mod_env = AdState::new();
        self.amp_env = AdState::new();
        self.active = false;
    }

    fn retrigger(&mut self, note: u8, velocity: f32) {
        // Before any param read: this note's lock set is now final.
        self.consume_pending_locks();
        // #175: `raw_param`, not `get_param` — see `AnalogEngine::retrigger`.
        let tune = self.raw_param(fp("tune"));
        self.current_hz      = note_to_hz(note, tune);
        self.last_note        = note;
        self.velocity_level   = velocity.clamp(0.0, 1.0);
        self.carrier_phase   = 0.0;
        self.modulator_phase = 0.0;
        self.prev_mod_out    = 0.0;
        // Only Kick uses pitch_env for the pitch-drop chirp.
        // P7: re-enable pitch_env.trigger() for Bell/Bass when pitch-chirp
        // parameters are added to those machine variants.
        if self.machine == FmMachine::Kick {
            self.pitch_env.trigger();
        }
        self.mod_env.trigger();
        self.amp_env.trigger();
        self.lfo.trigger(self.lfo_settings());
        self.active = true;
    }

    /// Render `[start, end)` with the current voice state, dispatched by
    /// machine. A no-op span (or inactive voice) leaves the zeroed buffer.
    /// MM-C7: chunked into `LFO_SUB_BLOCK` pieces relative to `start` — see
    /// `AnalogEngine::render_span` for why relative, and for why a voice that
    /// goes idle mid-span is deliberately not cut short.
    fn render_span(&mut self, start: usize, end: usize) {
        if start >= end || !self.active {
            return;
        }
        for (lo, hi) in sub_blocks(start, end) {
            self.update_lfo(hi - lo);
            match self.machine {
                FmMachine::Kick => self.process_kick(lo, hi),
                FmMachine::Bell => self.process_bell(lo, hi),
                FmMachine::Bass => self.process_bass(lo, hi),
            }
        }
        // Velocity is baked into the span at render time (review finding):
        // a whole-block output multiplier would rescale an earlier span —
        // a different note, or a prior voice's tail — to the LAST
        // retrigger's velocity when two notes share a block.
        let v = self.velocity_level;
        if v != 1.0 {
            for s in &mut self.render_l[start..end] { *s *= v; }
            for s in &mut self.render_r[start..end] { *s *= v; }
        }
    }

    fn process_kick(&mut self, start: usize, end: usize) {
        let punch    = self.get_param(fp("punch"));
        let decay_s  = self.get_param(fp("decay"));
        let feedback = self.get_param(fp("feedback"));
        let drive    = self.get_param(fp("drive"));
        let sr = self.sample_rate;
        let tau = std::f32::consts::TAU;

        let pitch_attack_inc  = 1.0 / (0.002 * sr);
        let pitch_decay_coeff = 0.001f32.powf(1.0 / ((0.05 + punch * 0.1) * sr));
        let mod_attack_inc    = 1.0 / (0.001 * sr);
        let mod_decay_coeff   = 0.001f32.powf(1.0 / ((0.02 + punch * 0.05) * sr));
        let amp_attack_inc    = 1.0 / (0.001 * sr);
        let amp_decay_coeff   = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        // #175: hoisted with the other per-chunk reads, so the `tune` sweep
        // advances once per LFO sub-block like every other destination.
        let swept_hz = self.swept_hz();

        for i in start..end {
            let pitch_val  = self.pitch_env.tick(pitch_attack_inc, pitch_decay_coeff);
            let carrier_hz = swept_hz * 2.0f32.powf(pitch_val * punch * 24.0 / 12.0);

            let mod_env_val = self.mod_env.tick(mod_attack_inc, mod_decay_coeff);
            let mod_hz = carrier_hz;
            self.modulator_phase = (self.modulator_phase + mod_hz / sr).fract();
            let fb_term = feedback * self.prev_mod_out;
            let m = (self.modulator_phase * tau + fb_term * tau).sin();
            self.prev_mod_out = m;
            let mod_out = m * mod_env_val;

            let index = 2.0 + punch * 4.0;
            self.carrier_phase = (self.carrier_phase + carrier_hz / sr + index * mod_out / tau).fract();
            let carrier_out = (self.carrier_phase * tau).sin();
            let amp = self.amp_env.tick(amp_attack_inc, amp_decay_coeff);
            let out = soft_clip(carrier_out * amp * (1.0 + drive * 9.0));
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }

    fn process_bell(&mut self, start: usize, end: usize) {
        let ratio    = self.get_param(fp("ratio"));
        let index    = self.get_param(fp("index"));
        let decay_s  = self.get_param(fp("decay"));
        let feedback = self.get_param(fp("feedback"));
        let sr = self.sample_rate;
        let tau = std::f32::consts::TAU;

        let decay_coeff = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let attack_inc  = 1.0 / (0.001 * sr);
        let swept_hz = self.swept_hz(); // #175

        for i in start..end {
            let mod_env_val = self.mod_env.tick(attack_inc, decay_coeff);
            let amp_val     = self.amp_env.tick(attack_inc, decay_coeff);

            let mod_hz = swept_hz * ratio;
            self.modulator_phase = (self.modulator_phase + mod_hz / sr).fract();
            let fb_term = feedback * self.prev_mod_out;
            let mod_out = (self.modulator_phase * tau + fb_term * tau).sin();
            self.prev_mod_out = mod_out;

            self.carrier_phase = (self.carrier_phase + swept_hz / sr
                + index * mod_out * mod_env_val / tau).fract();
            let out = (self.carrier_phase * tau).sin() * amp_val;
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }

    fn process_bass(&mut self, start: usize, end: usize) {
        let ratio    = self.get_param(fp("ratio"));
        let index    = self.get_param(fp("index"));
        let attack_s = self.get_param(fp("attack"));
        let decay_s  = self.get_param(fp("decay"));
        let drive    = self.get_param(fp("drive"));
        let sr = self.sample_rate;
        let tau = std::f32::consts::TAU;

        let attack_inc      = 1.0 / (attack_s * sr).max(1.0);
        let decay_coeff     = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let mod_decay_coeff = 0.001f32.powf(1.0 / ((decay_s * 0.3) * sr).max(1.0));
        let swept_hz = self.swept_hz(); // #175

        for i in start..end {
            let mod_env_val = self.mod_env.tick(attack_inc, mod_decay_coeff);
            let amp_val     = self.amp_env.tick(attack_inc, decay_coeff);

            let mod_hz = swept_hz * ratio;
            self.modulator_phase = (self.modulator_phase + mod_hz / sr).fract();
            let mod_out = (self.modulator_phase * tau).sin() * mod_env_val;

            self.carrier_phase = (self.carrier_phase + swept_hz / sr
                + index * mod_out / tau).fract();
            let out = soft_clip((self.carrier_phase * tau).sin() * amp_val * (1.0 + drive * 9.0));
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }
}

impl FmEngine {
    /// One machine's page placements — **this is the #47 (BUG-037) fix**.
    ///
    /// The single machine-invariant page set this replaces named `ratio`,
    /// `index`, `feedback`, `drive` and `attack` for every machine, while
    /// FmKick declares none of `ratio`/`index`/`attack`, FmBell none of
    /// `drive`/`attack`, and FmBass no `feedback`. Composite assembly degraded
    /// each unmatched ref to a `param_{id}` placeholder, so FmBass — node 27
    /// in the shipped instrument — drew a dead control at SRC slot 2. And
    /// `tune`, which all three declare, appeared on **no** page at all, so it
    /// was unreachable from any surface.
    ///
    /// Slots are assigned per *param*, not packed per machine, so a param
    /// keeps the same encoder on every machine that declares it. Machines
    /// that do not declare one leave its column empty — MM-C0's slot honouring
    /// is what makes that render correctly.
    ///
    /// **`AnalogEngine` packs instead, and that divergence is deliberate.**
    /// There, every param two machines share (`tune`, `tone`, `decay`) is
    /// already at a fixed slot, and the rest are machine-*exclusive*
    /// (`punch`/`snap`, `drive`/`noise`), so packing them collides nothing a
    /// performer could be holding across a switch. Here half the set is shared
    /// across *some* pair — `feedback` by Kick and Bell, `drive` by Kick and
    /// Bass, `ratio`/`index` by Bell and Bass — so packing would move a
    /// control that exists on both sides of the switch. The invariant both
    /// engines keep is the one that matters: **a shared param never moves.**
    fn machine_page_refs(machine: FmMachine) -> Vec<(u32, PageRef)> {
        const SRC: &str = "SRC";
        let src = |name: &str, slot: u8| {
            (fp(name), PageRef { page: Cow::Borrowed(SRC), slot })
        };
        // MM-C6 item 2 / ADR-041 amendment 2: machine-select lives on the
        // TRIG page. Declared by the engine, not synthesized by one surface,
        // so every surface inherits it through the machinery MM-C5 built —
        // and so `machine` stops being a declared param that no page reaches.
        // Slot 0 of TRIG on every machine: a shared param never moves.
        let mut refs = vec![
            (fp("machine"), PageRef { page: Cow::Borrowed("TRIG"), slot: 0 }),
            (fp("decay"), PageRef { page: Cow::Borrowed("AMP"), slot: 0 })];
        // MM-C9: machine-invariant MOD page, laid out identically to
        // AnalogEngine's so a performer's muscle memory carries across tracks.
        for (i, id) in LFO_PAGE_ORDER.iter().enumerate() {
            refs.push((*id, PageRef { page: Cow::Borrowed("MOD"), slot: i as u8 }));
        }
        refs.push(src("tune", 0));
        match machine {
            FmMachine::Kick => {
                refs.push(src("feedback", 3));
                refs.push(src("drive", 4));
                refs.push(src("punch", 5));
            }
            FmMachine::Bell => {
                refs.push(src("ratio", 1));
                refs.push(src("index", 2));
                refs.push(src("feedback", 3));
            }
            FmMachine::Bass => {
                refs.push(src("ratio", 1));
                refs.push(src("index", 2));
                refs.push(src("drive", 4));
                refs.push(src("attack", 6));
            }
        }
        refs
    }

    /// This machine's per-param ranges, for a surface to display and clamp
    /// against. The bank stores the union and is never narrowed to these.
    fn machine_overlays(machine: FmMachine) -> Vec<(u32, ParamOverlay)> {
        let mut out: Vec<(u32, ParamOverlay)> = vec![(
            fp("machine"),
            ParamOverlay {
                min: 0.0,
                max: (FmMachine::ALL.len() - 1) as f64,
                default: machine.value() as f64,
                identity: true,
            },
        )];
        // #179: the dest encoder reaches only this machine's destinations.
        out.push((
            fp("lfo_dest"),
            ParamOverlay {
                min: 0.0,
                max: Self::dest_names(machine).len() as f64,
                default: 0.0,
                identity: false,
            },
        ));
        for p in Self::machine_params(machine) {
            out.push((
                p.id,
                ParamOverlay { min: p.min, max: p.max, default: p.default, identity: false },
            ));
        }
        out
    }

    fn machine_variants() -> Vec<MachineVariant> {
        FmMachine::ALL
            .iter()
            .map(|&m| MachineVariant {
                value: m.value(),
                name: Cow::Borrowed(m.doc_name()),
                page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC"), Cow::Borrowed("AMP"), Cow::Borrowed("MOD")]),
                pages: Cow::Owned(Self::machine_page_refs(m)),
                overlays: Cow::Owned(Self::machine_overlays(m)),
                // ADR-041 amendment 2026-08-02: the dest labels are this
                // machine's — see `AnalogEngine::machine_variants`.
                param_labels: Cow::Owned(vec![(
                    fp("lfo_dest"),
                    Self::dest_param_labels(m).into(),
                )]),
            })
            .collect()
    }
}

impl ViewPlugin for FmEngine {
    fn to_rule(&self, _node_id: u64, _sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule {
        let decay_id = fp("decay");
        Rule {
            name: Cow::Borrowed(self.machine.display_name()),
            page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC"), Cow::Borrowed("AMP"), Cow::Borrowed("MOD")]),
            // Base fields stay the ACTIVE machine's, so a consumer that
            // ignores `variants` renders what it did before (ADR-041
            // decision 3). MM-C5 teaches composite assembly to prefer the
            // variant.
            param_pages: Cow::Owned(Self::machine_page_refs(self.machine)),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Owned(vec![
                (decay_id, AffordanceHint::EnvelopeCurve { group_idx: 0 }),
                // MM-C11: see AnalogEngine's copy — first real LfoShape.
                (fp("lfo_shape"), AffordanceHint::LfoShape),
            ]),
            envelopes: Cow::Owned(vec![EnvelopeGroup {
                env_type: Cow::Borrowed("AD"),
                label: Cow::Borrowed("Amp Envelope"),
                param_ids: [decay_id, 0, 0, 0],
            }]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Owned(Self::machine_variants()),
        }
    }
}

impl Node for FmEngine {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }
    fn capability_document(&self) -> CapabilityDocument {
        let mut doc = Self::build_doc(self.machine);
        doc.view = Some(self.to_rule(0, &[]));
        doc
    }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    // published_state() runs on the main thread after process() returns
    // on the audio thread — no concurrent access to self.amp_env.value.
    /// See `AnalogEngine::serialize` — same reasoning, same union bank. The
    /// bank is all of it; operator phase and envelope state are transient.
    fn serialize(&self) -> Vec<u8> {
        self.bank.serialize()
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.bank.deserialize(data);
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
        buf.push((
            format!("/node/{}/state/env_level", self.node_id),
            StateBusValue::Float(self.amp_env.value as f64),
        ));
        // ADR-042 decision 7.
        buf.push((
            format!("/node/{}/state/lfo_phase", self.node_id),
            StateBusValue::Float(self.lfo.phase() as f64),
        ));
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate    = sample_rate;
        let doc = Self::build_doc(self.machine);
        self.bank           = ParameterBank::from_capability_document(&doc);
        // BUG-008 fix: consume the pending map so a re-activate (dynamic
        // topology rebuild, P9 C4) cannot overwrite deserialized state.
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, value);
            }
        }
        self.render_l       = vec![0.0; block_size];
        self.render_r       = vec![0.0; block_size];
        self.carrier_phase   = 0.0;
        self.modulator_phase = 0.0;
        self.prev_mod_out    = 0.0;
        self.pitch_env  = AdState::new();
        self.mod_env    = AdState::new();
        self.amp_env    = AdState::new();
        self.current_hz = 65.41;
        self.active     = false;
        self.last_note  = 36;
        self.velocity_level = 1.0;
        self.switch_fade = None;
        // #169: locks outlive a block now, so a re-activate must retire them.
        self.node_locks.clear();
        self.locks_pending = false;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);
        // Block boundary: the one place a machine switch may begin.
        self.poll_machine_param();

        let block_size = input.block_size;
        for s in &mut self.render_l { *s = 0.0; }
        for s in &mut self.render_r { *s = 0.0; }

        // #169: `node_locks` deliberately survives the block boundary — see
        // `AnalogEngine::process`. Only the pending flag is per-cycle.
        self.locks_pending = false;

        // Handle NodeCommands: CMD_TRIGGER live-triggers a voice (same retrigger
        // path as NoteOn). arg0 = note (< 0 → last-triggered note); arg1 = velocity
        // 0.0..=1.0 (<= 0.0 → default 0.79).
        for cmd in input.commands {
            if cmd.type_id == CMD_TRIGGER {
                let note: u8 = if cmd.arg0 < 0 {
                    self.last_note
                } else {
                    cmd.arg0.clamp(0, 127) as u8
                };
                let velocity: f32 = if cmd.arg1 <= 0.0 {
                    0.79
                } else {
                    cmd.arg1.clamp(0.0, 1.0) as f32
                };
                self.retrigger(note, velocity);
                output.emit_debug(0, DebugEventKind::VoiceTrigger, note as i64, velocity as f64);
            }
        }

        // Handle events in offset order (the executor sorts by
        // (sample_offset, priority)). A NoteOn mid-block splits the render
        // at its offset (BUG-013) — sample-accurate voice starts; see
        // AnalogEngine::process for the full rationale.
        let mut cursor = 0usize;
        for timed in input.events {
            match timed.event {
                Event::ParamLock(ref pl) if pl.node_id == self.node_id => {
                    // Per-note override, never a bank write (BUG-015 /
                    // ADR-019): a locked step must not bleed into the next.
                    self.push_lock(pl.param_id, pl.value);
                }
                Event::Midi2(UmpMessage::ChannelVoice2(ChannelVoice2::NoteOn(n))) => {
                    let off = timed.sample_offset as usize;
                    if off > cursor {
                        self.render_span(cursor, off);
                        cursor = off;
                    }
                    let velocity = n.velocity() as f32 / 65535.0;
                    self.retrigger(u8::from(n.note_number()), velocity);
                    output.emit_debug(off as u32, DebugEventKind::VoiceTrigger, u8::from(n.note_number()) as i64, velocity as f64);
                }
                _ => {}
            }
        }
        self.render_span(cursor, block_size);
        self.apply_switch_fade(block_size);

        if let Some(buf) = output.audio_outputs.first_mut() {
            if buf.channels() >= 2 {
                for (dst, &src) in buf.channel_mut(0).iter_mut().zip(self.render_l[..block_size].iter()) {
                    *dst = src;
                }
                for (dst, &src) in buf.channel_mut(1).iter_mut().zip(self.render_r[..block_size].iter()) {
                    *dst = src;
                }
            } else if buf.channels() == 1 {
                for (i, (&l, &r)) in self.render_l[..block_size].iter()
                    .zip(self.render_r[..block_size].iter()).enumerate()
                {
                    buf.channel_mut(0)[i] = (l + r) * 0.5;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        AudioBuffer, Event, EventOutputBuffer, ExtendedEventSlab,
        NodeCommand, CMD_SET_PARAM, CMD_TRIGGER, ParamLockEvent, TimedEvent, TransportInfo,
        UmpMessage, midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7},
    };

    fn make_note_on(note: u8) -> TimedEvent {
        let mut msg = NoteOn::<[u32; 4]>::new();
        msg.set_group(u4::new(0));
        msg.set_channel(u4::new(0));
        msg.set_note_number(u7::new(note & 0x7F));
        msg.set_velocity(32768);
        TimedEvent::new(0, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
    }

    fn run_fm(eng: &mut FmEngine, events: &[TimedEvent]) -> Vec<f32> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let ap: *mut AudioBuffer = &mut audio;
        let ar: &mut AudioBuffer = unsafe { &mut *ap };
        let mut outs = [ar];
        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: &[],
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        eng.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    fn run_fm_cmds(eng: &mut FmEngine, events: &[TimedEvent], cmds: &[NodeCommand]) -> Vec<f32> {
        let block = 512usize;
        let mut audio = AudioBuffer::new(2, block);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        let ap: *mut AudioBuffer = &mut audio;
        let ar: &mut AudioBuffer = unsafe { &mut *ap };
        let mut outs = [ar];
        let input = ProcessInput {
            audio_inputs: &[], signal_inputs: &[], events,
            transport: &transport, sample_rate: 44100.0, block_size: block,
            extended_events: &slab, commands: cmds,
        };
        let mut output = ProcessOutput::new(&mut outs, &mut [], &mut events_out);
        eng.process(&input, &mut output);
        audio.channel(0).to_vec()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|&x| x*x).sum::<f32>() / v.len() as f32).sqrt()
    }

    // ── MM-C4: union bank, machine identity, and the #47 page fix ─────────

    /// **The phase's load-bearing invariant** (MM §3.4), and FmEngine is where
    /// it actually bites: `decay` spans three different ranges across the
    /// machines and `feedback` two, so a narrowed bank would truncate a Bell
    /// patch's long decay the moment Kick was selected.
    #[test]
    fn union_bank_covers_every_variant_overlay() {
        for constructed in FmMachine::ALL {
            let doc = FmEngine::build_doc(constructed);
            for variant in FmEngine::machine_variants() {
                for (pid, overlay) in variant.overlays.iter() {
                    let slot = doc.params.iter().find(|p| p.id == *pid).unwrap_or_else(|| {
                        panic!("variant {} declares param {pid} the union doc lacks", variant.name)
                    });
                    assert!(
                        slot.min <= overlay.min && slot.max >= overlay.max,
                        "bank range [{}, {}] for param {pid} does not cover {}'s overlay \
                         [{}, {}] (constructed as {constructed:?}) — narrowing truncates \
                         stored values on load",
                        slot.min, slot.max, variant.name, overlay.min, overlay.max
                    );
                }
            }
        }
    }

    /// The conflict this engine exists to demonstrate: Bell's `decay` reaches
    /// 8 s, Kick's stops at 2 s. A Kick-constructed engine must still store 8.
    #[test]
    fn a_value_legal_on_another_machine_survives_loading_under_this_one() {
        let bell_decay_max = FmEngine::machine_params(FmMachine::Bell)
            .into_iter()
            .find(|p| p.id == fp("decay"))
            .expect("Bell declares decay")
            .max;
        let kick_decay_max = FmEngine::machine_params(FmMachine::Kick)
            .into_iter()
            .find(|p| p.id == fp("decay"))
            .expect("Kick declares decay")
            .max;
        assert!(
            bell_decay_max > kick_decay_max,
            "fixture assumption: Bell's decay range must exceed Kick's"
        );

        let mut eng = FmEngine::kick();
        let mut initial = HashMap::new();
        initial.insert("decay".to_string(), bell_decay_max);
        eng.set_initial_params(&initial);
        eng.activate(44100.0, 512);

        assert_eq!(
            eng.bank.get(fp("decay")),
            bell_decay_max,
            "a Kick-constructed engine truncated a Bell-legal decay — the bank \
             was narrowed to the active machine, destroying the value on load"
        );
    }

    #[test]
    fn machine_round_trip_preserves_every_param() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);

        let doc = FmEngine::build_doc(FmMachine::Kick);
        let mut expected: Vec<(u32, f64)> = Vec::new();
        for p in doc.params.iter().filter(|p| p.id != fp("machine")) {
            let v = p.min + (p.max - p.min) * 0.73;
            eng.bank.set(p.id, v);
            expected.push((p.id, v));
        }

        for target in [FmMachine::Bell, FmMachine::Bass, FmMachine::Kick] {
            eng.bank.set(fp("machine"), target.value() as f64);
            eng.poll_machine_param();
            assert_eq!(eng.machine, target, "switch should apply while silent");
        }

        for (pid, want) in expected {
            assert_eq!(eng.bank.get(pid), want, "param {pid} changed across a round trip");
        }
    }

    #[test]
    fn union_doc_has_no_duplicate_ids() {
        for m in FmMachine::ALL {
            let doc = FmEngine::build_doc(m);
            let mut ids: Vec<u32> = doc.params.iter().map(|p| p.id).collect();
            let before = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), before, "duplicate param id in {m:?}'s union doc");
        }
    }

    /// MM-C8 assertion part 3, brought forward because this is the engine with
    /// the wide conflict set. The union merge keeps the *first declarer's*
    /// name/unit/stepped for a shared id and silently drops the rest, so a
    /// disagreement would show a param under the wrong unit with no diagnostic.
    #[test]
    fn shared_param_ids_agree_on_name_unit_and_stepped() {
        let mut seen: HashMap<u32, (String, ParamUnit, bool)> = HashMap::new();
        for m in FmMachine::ALL {
            for p in FmEngine::machine_params(m) {
                let key = (p.name.as_str().to_string(), p.unit, p.stepped);
                match seen.get(&p.id) {
                    Some(first) => assert_eq!(
                        *first, key,
                        "param {} ({}) disagrees across machines: {first:?} vs {key:?} — \
                         the union merge would silently keep the first",
                        p.id,
                        p.name.as_str()
                    ),
                    None => {
                        seen.insert(p.id, key);
                    }
                }
            }
        }
    }

    #[test]
    fn machine_param_is_stepped_over_the_machine_count() {
        let doc = FmEngine::build_doc(FmMachine::Kick);
        let m = doc.params.iter().find(|p| p.id == fp("machine")).expect("machine param");
        assert!(m.stepped);
        assert_eq!((m.min, m.max), (0.0, 2.0));
    }

    #[test]
    fn every_variant_flags_machine_as_identity() {
        for v in FmEngine::machine_variants() {
            let (_, o) = v
                .overlays
                .iter()
                .find(|(pid, _)| *pid == fp("machine"))
                .unwrap_or_else(|| panic!("variant {} has no machine overlay", v.name));
            assert!(o.identity, "variant {} does not flag machine as identity", v.name);
        }
    }

    /// MM-C5 resolves which variant a surface draws from the cap-doc default
    /// of the identity param, so that a view assembled with no live state
    /// still shows the machine the node was built on. That only works while
    /// the identity param's default *is* the active machine's value.
    #[test]
    fn the_machine_params_cap_doc_default_is_the_active_machine() {
        for m in FmMachine::ALL {
            let doc = FmEngine::build_doc(m);
            let p = doc
                .params
                .iter()
                .find(|p| p.id == fp("machine"))
                .expect("union doc declares machine");
            assert_eq!(
                p.default,
                m.value() as f64,
                "{m:?}'s cap-doc must name {m:?} as the machine in force"
            );
        }
    }

    /// The base `Rule` fields are the active machine's, so a consumer that
    /// ignores `variants` renders what the variant-aware path renders
    /// (ADR-041 decision 3). If these drift, `assemble` and a client that
    /// reads `param_pages` directly disagree about the same node.
    #[test]
    fn base_param_pages_equal_the_active_variants_pages() {
        for m in FmMachine::ALL {
            let rule = FmEngine::new(m).to_rule(0, &[]);
            let v = rule
                .variants
                .iter()
                .find(|v| v.value == m.value())
                .expect("a variant per machine");
            assert_eq!(
                rule.param_pages.to_vec(),
                v.pages.to_vec(),
                "{m:?}'s base pages differ from its own variant's"
            );
            assert_eq!(rule.page_groups.to_vec(), v.page_groups.to_vec());
        }
    }

    /// **This is #47 (BUG-037).** Before MM-C4 the single machine-invariant
    /// page set named `ratio`/`index`/`attack` for FmKick, which declares none
    /// of them; `drive`/`attack` for FmBell; `feedback` for FmBass — the last
    /// of which is node 27 in the shipped instrument, drawing a dead control.
    #[test]
    fn every_variant_page_ref_resolves_in_that_variants_params() {
        for m in FmMachine::ALL {
            let mut declared: Vec<u32> =
                FmEngine::machine_params(m).iter().map(|p| p.id).collect();
            declared.push(fp("machine"));
            // Same for the seven `lfo_*` params (MM-C9): one LFO per node, not
            // per machine, so `machine_params` excludes them too while the MOD
            // page carries them on every variant.
            declared.extend(LFO_PAGE_ORDER); // union-level, MM-C6 pages it
            for (pid, page_ref) in FmEngine::machine_page_refs(m) {
                assert!(
                    declared.contains(&pid),
                    "{m:?}'s {} page slot {} names param {pid}, which {m:?} does not declare",
                    page_ref.page, page_ref.slot
                );
            }
        }
    }

    /// The other half of #47: every param a machine declares must be reachable
    /// from some page. `punch` (FmKick) and `tune` (all three) were on no page
    /// at all, so no surface could edit them.
    #[test]
    fn every_declared_param_appears_on_some_page() {
        for m in FmMachine::ALL {
            let paged: Vec<u32> = FmEngine::machine_page_refs(m).iter().map(|(id, _)| *id).collect();
            for p in FmEngine::machine_params(m) {
                assert!(
                    paged.contains(&p.id),
                    "{m:?} declares {} but no page references it — unreachable from any surface",
                    p.name.as_str()
                );
            }
        }
    }

    /// A param keeps the same encoder across every machine that has it, so
    /// switching machine does not shuffle controls under the performer.
    #[test]
    fn shared_params_keep_the_same_slot_across_machines() {
        let mut slot_of: HashMap<u32, (String, u8)> = HashMap::new();
        for m in FmMachine::ALL {
            for (pid, r) in FmEngine::machine_page_refs(m) {
                let here = (r.page.as_ref().to_string(), r.slot);
                match slot_of.get(&pid) {
                    Some(first) => assert_eq!(
                        *first, here,
                        "param {pid} sits at {first:?} on one machine and {here:?} on {m:?}"
                    ),
                    None => {
                        slot_of.insert(pid, here);
                    }
                }
            }
        }
    }

    #[test]
    fn switch_while_silent_is_immediate() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        assert!(!eng.active);
        eng.bank.set(fp("machine"), FmMachine::Bass.value() as f64);
        eng.poll_machine_param();
        assert_eq!(eng.machine, FmMachine::Bass, "nothing to declick");
        assert!(eng.switch_fade.is_none());
    }

    #[test]
    fn switch_while_sounding_fades_out_before_swapping() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let _ = run_fm(&mut eng, &[make_note_on(36)]);
        assert!(eng.active, "voice should be sounding");

        eng.bank.set(fp("machine"), FmMachine::Bell.value() as f64);
        eng.poll_machine_param();
        assert_eq!(eng.machine, FmMachine::Kick, "must fade first, not swap instantly");

        let out = run_fm(&mut eng, &[]);
        assert_eq!(eng.machine, FmMachine::Bell, "swap after the fade");
        assert!(!eng.active, "voice state resets on switch");

        // Envelope, not per-sample slope: FM at a high modulation index has
        // legitimate near-Nyquist content, so adjacent samples can differ by
        // more than the peak and a step bound says nothing. What a fade must
        // show is a *declining* envelope followed by exact silence.
        let fade = eng.fade_len() as usize;
        assert!(fade * 2 < out.len(), "fixture: the fade must fit inside a block");
        let rms = |w: &[f32]| (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();

        let first = rms(&out[..fade / 2]);
        let second = rms(&out[fade / 2..fade]);
        assert!(first > 0.001, "fixture: the fading voice must be audible");
        // 0.6, not merely `second < first`. A hard cut at the fade boundary —
        // no ramp at all — still yields a ratio of ~0.96 here, because the
        // kick's own amp decay falls a few percent over 5 ms; the real ramp
        // gives ~0.36. Bare monotonicity passes the mutant.
        assert!(
            second < first * 0.6,
            "envelope declined only {:.3}x across the fade ({first:.4} then \
             {second:.4}) — that is the voice's own decay, not a ramp",
            second / first
        );
        assert!(
            out[fade..].iter().all(|s| *s == 0.0),
            "everything after the fade must be exactly silent, ready for the swap"
        );
    }

    /// The cancel path (MM §0 D1). Copied into this engine from AnalogEngine
    /// and initially shipped unguarded here — a mutation replacing the whole
    /// branch with `switch_fade = None` passed the entire suite. ~35 lines of
    /// the trickiest logic in the commit, unguarded in its second copy.
    #[test]
    fn cancelling_a_switch_ramps_back_instead_of_snapping() {
        let mut eng = FmEngine::kick();
        // 192 kHz makes the 5 ms fade 960 samples — longer than the 512-sample
        // block, which is the only condition under which a cancel can land
        // mid-ramp at all.
        eng.activate(192_000.0, 512);
        let _ = run_fm(&mut eng, &[make_note_on(36)]);

        eng.bank.set(fp("machine"), FmMachine::Bell.value() as f64);
        eng.poll_machine_param();
        assert_eq!(
            eng.switch_fade.expect("fade armed").target,
            Some(FmMachine::Bell)
        );

        eng.bank.set(fp("machine"), FmMachine::Kick.value() as f64);
        eng.poll_machine_param();
        let cancelling = eng.switch_fade.expect("a cancel must still ramp");
        assert_eq!(
            cancelling.target, None,
            "a cancelled switch ramps back to unity and swaps nothing"
        );
        assert_eq!(eng.machine, FmMachine::Kick, "no swap happened");
    }

    /// The retarget path. A mutation taking a fresh full-length fade here also
    /// passed the whole suite before this test existed.
    #[test]
    fn retargeting_mid_fade_keeps_the_gain_it_reached() {
        let mut eng = FmEngine::kick();
        eng.activate(192_000.0, 512);
        let _ = run_fm(&mut eng, &[make_note_on(36)]);

        eng.bank.set(fp("machine"), FmMachine::Bell.value() as f64);
        eng.poll_machine_param();
        let _ = run_fm(&mut eng, &[]); // burn part of the fade
        let mid = eng.switch_fade.expect("still fading").remaining;
        assert!(mid < eng.fade_len(), "fixture: some fade must have elapsed");

        eng.bank.set(fp("machine"), FmMachine::Bass.value() as f64);
        eng.poll_machine_param();
        let after = eng.switch_fade.expect("still fading");
        assert_eq!(after.target, Some(FmMachine::Bass));
        assert!(
            after.remaining <= mid,
            "retarget restarted the fade ({} > {mid}), stepping the gain back \
             to unity",
            after.remaining
        );
    }

    /// Two params of one machine on one slot means one silently covers the
    /// other. MM-C0's duplicate-slot `debug_assert` lives in composite
    /// assembly, which no engine unit test reaches, and MM-C8 part 2 checks
    /// *overlay* id uniqueness — a different thing. This commit hand-maintains
    /// a 7-column table across three machines, so the guard belongs next to it.
    #[test]
    fn no_machine_puts_two_params_on_one_slot() {
        for m in FmMachine::ALL {
            let mut seen: Vec<(String, u8)> = Vec::new();
            for (pid, r) in FmEngine::machine_page_refs(m) {
                let key = (r.page.as_ref().to_string(), r.slot);
                assert!(
                    !seen.contains(&key),
                    "{m:?} puts a second param ({pid}) on {} slot {} — one \
                     would silently cover the other",
                    r.page,
                    r.slot
                );
                seen.push(key);
            }
        }
    }

    #[test]
    fn a_param_lock_on_machine_does_not_switch() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        eng.set_node_id(27);
        let lock = TimedEvent {
            sample_offset: 0,
            event: Event::ParamLock(ParamLockEvent {
                node_id: 27,
                param_id: fp("machine"),
                value: FmMachine::Bass.value() as f64,
            }),
        };
        for _ in 0..3 {
            let _ = run_fm(&mut eng, std::slice::from_ref(&lock));
            assert_eq!(eng.machine, FmMachine::Kick, "a p-lock must never switch machines");
        }
        assert!(eng.switch_fade.is_none(), "and must not even arm a fade");
    }

    /// See `AnalogEngine`'s copy — same claim, same reasoning, and the same
    /// note that MM-C9 will legitimately break it.
    #[test]
    fn chunked_render_is_identical_to_one_unchunked_call() {
        for m in FmMachine::ALL {
            let mut whole = FmEngine::new(m);
            let mut chunked = FmEngine::new(m);
            whole.activate(44100.0, 512);
            chunked.activate(44100.0, 512);
            whole.retrigger(60, 1.0);
            chunked.retrigger(60, 1.0);

            const END: usize = 500;
            match m {
                FmMachine::Kick => whole.process_kick(0, END),
                FmMachine::Bell => whole.process_bell(0, END),
                FmMachine::Bass => whole.process_bass(0, END),
            }
            for (lo, hi) in sub_blocks(0, END) {
                match m {
                    FmMachine::Kick => chunked.process_kick(lo, hi),
                    FmMachine::Bell => chunked.process_bell(lo, hi),
                    FmMachine::Bass => chunked.process_bass(lo, hi),
                }
            }

            assert_eq!(
                whole.render_l[..END],
                chunked.render_l[..END],
                "{m:?}: chunking changed the output — a `process_*` carries \
                 per-span state the chunking broke. Find it; do not \
                 re-fingerprint."
            );
            assert_eq!(
                whole.active, chunked.active,
                "{m:?}: voice liveness must not depend on the chunking"
            );
        }
    }

    /// MM §0 D3 — see `AnalogEngine`'s copy. `lfo_dest` stores a one-based
    /// index into this table, so a reorder re-points every saved patch.
    #[test]
    fn the_lfo_dest_table_is_append_only() {
        assert_eq!(
            FmEngine::LFO_DESTS,
            &[
                fp("tune"), fp("decay"), fp("ratio"), fp("index"),
                fp("feedback"), fp("drive"), fp("punch"), fp("attack"),
            ],
            "APPEND ONLY"
        );
    }

    /// The id table and the name table are two lists that must stay in step —
    /// there is no stable way to derive one from the other in a `const`
    /// initialiser, so this is what stops a drift from silently mislabelling
    /// an encoder.
    #[test]
    fn the_dest_ids_and_names_correspond() {
        // #179: and every per-machine table must draw from this union, since
        // `LFO_DESTS.len()` is the width the bank persists `lfo_dest` at. A
        // machine list longer than the union would truncate on load.
        for names in [FM_KICK_DEST_NAMES, FM_BELL_DEST_NAMES, FM_BASS_DEST_NAMES] {
            assert!(
                names.len() <= FmEngine::LFO_DESTS.len(),
                "a machine cannot offer more dests than the bank can store"
            );
            for n in names {
                assert!(
                    FM_DEST_NAMES.contains(n),
                    "{n} is offered by a machine but absent from the union envelope"
                );
            }
        }
        assert_eq!(FM_DEST_NAMES.len(), FmEngine::LFO_DESTS.len());
        for (name, id) in FM_DEST_NAMES.iter().zip(FmEngine::LFO_DESTS) {
            assert_eq!(fp(name), *id, "`{name}` does not hash to its table entry");
        }
        // M2: each per-machine NAME table has a mirror ID table, pinned
        // entry-for-entry, so the audio-thread zero-cost lookup can never
        // point at a different param than the names advertise.
        for (names, ids) in [
            (FM_KICK_DEST_NAMES, FM_KICK_DEST_IDS),
            (FM_BELL_DEST_NAMES, FM_BELL_DEST_IDS),
            (FM_BASS_DEST_NAMES, FM_BASS_DEST_IDS),
        ] {
            assert_eq!(names.len(), ids.len(), "name and id tables must mirror");
            for (name, id) in names.iter().zip(ids) {
                assert_eq!(fp(name), *id, "`{name}` does not hash to its id table entry");
            }
        }
    }

    /// MM §0 D4: the cap-doc carries the `lfo_dest` labels, so a surface can
    /// name the destinations without knowing anything about LFOs. Static, not
    /// dynamic — ADR-042 amendment 5 ruled out `Dynamic` because it panics on
    /// clone, and the cap-doc path clones.
    #[test]
    fn lfo_dest_labels_reach_the_cap_doc_and_survive_a_clone() {
        // #179: per-machine labels — see AnalogEngine's twin for the contract.
        for (eng, names) in [
            (FmEngine::kick(), FM_KICK_DEST_NAMES),
            (FmEngine::bell(), FM_BELL_DEST_NAMES),
            (FmEngine::bass(), FM_BASS_DEST_NAMES),
        ] {
            let doc = eng.capability_document();
            let d = doc
                .params
                .iter()
                .find(|p| p.id == fp("lfo_dest"))
                .expect("lfo_dest is declared");
            let display = d.display.as_ref().expect("labels are declared");
            assert_eq!(display.format(0.0), "off");
            assert_eq!(display.format(1.0), names[0]);
            assert_eq!(display.format(names.len() as f64), *names.last().unwrap());
            assert_eq!(
                display.format(names.len() as f64 + 1.0),
                "",
                "past this machine's table is a gap, not another choice"
            );
            assert_eq!(display.parse("off"), Some(0.0));
            assert_eq!(display.parse(names[0]), Some(1.0));

            let labels = d.value_labels().expect("a stepped param with a display");
            assert_eq!(labels.len(), FmEngine::LFO_DESTS.len() + 1);
            for (i, want) in names.iter().enumerate() {
                assert_eq!(labels[i + 1].as_deref(), Some(*want));
            }
            for slot in labels.iter().skip(names.len() + 1) {
                assert_eq!(slot.as_deref(), None, "a gap must not be drawable");
            }

            // The whole doc is cloned on the mainline cap-doc path; `Dynamic`
            // would panic here.
            let _ = doc.clone();
        }
    }

    /// #179, FM half — see `AnalogEngine::every_machines_dests_are_exactly_its_params`.
    #[test]
    fn every_machines_dests_are_exactly_its_params() {
        for machine in FmMachine::ALL {
            let mut read: Vec<String> = FmEngine::machine_params(machine)
                .iter()
                .map(|p| p.name.to_string())
                .collect();
            let mut offered: Vec<String> = FmEngine::dest_names(machine)
                .iter()
                .map(|n| n.to_string())
                .collect();
            read.sort();
            offered.sort();
            // m1: the sorted comparison alone has a blind spot — a duplicate
            // mirrored in BOTH lists compares equal. Distinct-count each side.
            assert!(
                read.windows(2).all(|w| w[0] != w[1]),
                "{machine:?}: machine_params declares a duplicate param: {read:?}"
            );
            assert!(
                offered.windows(2).all(|w| w[0] != w[1]),
                "{machine:?}: dest table offers a duplicate: {offered:?}"
            );
            assert_eq!(
                offered, read,
                "{machine:?}: every destination offered must be a param this \
                 machine reads, and every param it reads must be offered"
            );
        }
    }

    /// **APPEND ONLY** — `lfo_dest` persists a one-based index into the active
    /// machine's table (#179).
    #[test]
    fn the_per_machine_dest_tables_are_append_only() {
        assert_eq!(FM_KICK_DEST_NAMES, &["tune", "punch", "decay", "feedback", "drive"]);
        assert_eq!(FM_BELL_DEST_NAMES, &["tune", "ratio", "index", "decay", "feedback"]);
        assert_eq!(FM_BASS_DEST_NAMES, &["tune", "ratio", "index", "attack", "decay", "drive"]);
    }

    /// BUG-069, FM half — see `AnalogEngine::the_lfo_range_tables_match_machine_params`.
    #[test]
    fn the_lfo_range_tables_match_machine_params() {
        for machine in FmMachine::ALL {
            let params = FmEngine::machine_params(machine);
            let ranges = FmEngine::dest_ranges(machine);
            assert_eq!(
                params.len(),
                ranges.len(),
                "{machine:?}: every machine param needs an LFO range"
            );
            for (id, lo, hi) in ranges {
                let p = params
                    .iter()
                    .find(|p| p.id == *id)
                    .unwrap_or_else(|| panic!("{machine:?}: range id {id} is not a machine param"));
                assert_eq!(*lo, p.min as f32, "{machine:?}: {} range min drifts", p.name);
                assert_eq!(*hi, p.max as f32, "{machine:?}: {} range max drifts", p.name);
            }
        }
    }

    /// Union-wide in the bank, per-machine on the encoder (#179) — the pair
    /// that makes a per-machine index safe to persist.
    #[test]
    fn the_dest_range_is_union_wide_in_the_bank_and_per_machine_on_the_encoder() {
        for machine in FmMachine::ALL {
            let doc = FmEngine::new(machine).capability_document();
            let d = doc
                .params
                .iter()
                .find(|p| p.id == fp("lfo_dest"))
                .expect("lfo_dest is declared");
            assert_eq!(d.max, FmEngine::LFO_DESTS.len() as f64);

            let overlay = FmEngine::machine_overlays(machine)
                .into_iter()
                .find(|(id, _)| *id == fp("lfo_dest"))
                .map(|(_, o)| o)
                .expect("the overlay narrows the encoder");
            assert_eq!(overlay.max, FmEngine::dest_names(machine).len() as f64);
            assert!(!overlay.identity, "`lfo_dest` is a setting, not identity");
        }
    }

    /// ADR-041 amendment 2026-08-02 (M1), FM half — see
    /// `AnalogEngine::each_variant_carries_that_machines_dest_labels`.
    #[test]
    fn each_variant_carries_that_machines_dest_labels() {
        for machine in FmMachine::ALL {
            let variants = FmEngine::machine_variants();
            let v = variants
                .iter()
                .find(|v| v.value == machine.value())
                .expect("every machine has a variant");
            let (_, labels) = v
                .param_labels
                .iter()
                .find(|(id, _)| *id == fp("lfo_dest"))
                .unwrap_or_else(|| {
                    panic!("{machine:?}: the variant must declare `lfo_dest` labels")
                });
            let want: Vec<Option<String>> = FmEngine::dest_param_labels(machine)
                .iter()
                .map(|o| o.as_ref().map(|s| s.to_string()))
                .collect();
            let got: Vec<Option<String>> = labels
                .iter()
                .map(|o| o.as_ref().map(|s| s.to_string()))
                .collect();
            assert_eq!(
                got, want,
                "{machine:?}: the variant's dest labels must be this machine's"
            );
        }
    }

    /// Every dest index modulates exactly the param it names, on every
    /// machine (#179 extends session #5's audit to the per-machine tables).
    #[test]
    fn every_dest_index_modulates_exactly_the_param_it_names() {
        use crate::engine_dsp::LFO_SUB_BLOCK;
        let mut failures: Vec<String> = Vec::new();
        for machine in FmMachine::ALL {
            let observed: Vec<u32> = FmEngine::union_params(machine)
                .iter()
                .map(|p| p.id)
                .collect();
            let name_of = |id: u32| -> String {
                FmEngine::union_params(machine)
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.to_string())
                    .unwrap_or_else(|| format!("id:{id}"))
            };

            for (i, expected_name) in FmEngine::dest_names(machine).iter().enumerate() {
                let mut eng = FmEngine::new(machine);
                eng.activate(44100.0, 512);
                for (name, v) in [
                    ("lfo_dest", (i + 1) as f64),
                    ("lfo_depth", 1.0),
                    ("lfo_speed", 4.0),
                    ("lfo_shape", 0.0),
                    ("lfo_mode", 1.0),
                ] {
                    eng.bank.set(fp(name), v);
                }
                eng.retrigger(60, 1.0);

                let base: Vec<f32> = observed.iter().map(|id| eng.raw_param(*id)).collect();
                let mut moved = vec![false; observed.len()];
                for _ in 0..200 {
                    eng.update_lfo(LFO_SUB_BLOCK);
                    for (k, id) in observed.iter().enumerate() {
                        if (eng.get_param(*id) - base[k]).abs() > 1e-6 {
                            moved[k] = true;
                        }
                    }
                }
                let actually_moved: Vec<String> = observed
                    .iter()
                    .zip(&moved)
                    .filter(|(_, m)| **m)
                    .map(|(id, _)| name_of(*id))
                    .collect();
                if actually_moved != vec![expected_name.to_string()] {
                    failures.push(format!(
                        "  {machine:?} lfo_dest={} expected [{}] but moved {:?}",
                        i + 1,
                        expected_name,
                        actually_moved
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "dest index -> modulated param mismatch:\n{}",
            failures.join("\n")
        );
    }

    /// MM-C11: the first real `LfoShape` in the tree — the hint has existed
    /// since ADR-032 and nothing ever emitted it (§2.6.5's known gap).
    #[test]
    fn lfo_shape_declares_the_lfo_shape_affordance() {
        let rule = FmEngine::bass().to_rule(0, &[]);
        assert!(
            rule.affordances.iter().any(|(pid, hint)| *pid == fp("lfo_shape")
                && matches!(hint, AffordanceHint::LfoShape)),
            "lfo_shape must carry the LfoShape affordance"
        );
    }

    #[test]
    fn the_dest_table_excludes_lfo_params_and_machine() {
        for id in FmEngine::LFO_DESTS {
            assert!(!LFO_PAGE_ORDER.contains(id), "an lfo_* param is a dest");
            assert_ne!(*id, fp("machine"), "`machine` is a dest");
        }
    }

    /// What keeps the four ADR-035 baselines valid after MM-C9.
    #[test]
    fn depth_zero_is_bit_identical_to_no_lfo() {
        use crate::engine_dsp::LFO_SUB_BLOCK;
        let mut eng = FmEngine::bass();
        eng.activate(44100.0, 512);
        eng.bank.set(fp("lfo_dest"), 3.0);
        eng.bank.set(fp("lfo_speed"), 8.0);
        let base = eng.raw_param(fp("ratio"));
        for _ in 0..40 {
            eng.update_lfo(LFO_SUB_BLOCK);
            assert_eq!(eng.get_param(fp("ratio")), base);
        }
    }

    /// A non-integral `lfo_dest` reads as off rather than truncating to a
    /// destination — the FM half of the guard `AnalogEngine`'s
    /// `dest_zero_and_out_of_range_are_both_off` pins. P-locks bypass the
    /// bank's clamp, so this is the path that can carry such a value.
    #[test]
    fn a_fractional_lfo_dest_is_off_not_truncated() {
        let mut eng = FmEngine::bass();
        eng.activate(44100.0, 512);
        eng.node_locks.push((fp("lfo_dest"), 1.9));
        assert_eq!(eng.lfo_dest_id(), None, "1.9 is not a destination");
        eng.node_locks.clear();
        eng.node_locks.push((fp("lfo_dest"), 2.0));
        assert_eq!(
            eng.lfo_dest_id(),
            Some(fp("ratio")),
            "an integral index still resolves"
        );
    }

    /// The MOD page is `LFO_PAGE_ORDER` at slots 0..6 on every machine. Both
    /// engines build from that same constant, which is what makes a
    /// performer's muscle memory carry from an analog track to an FM one —
    /// asserted against the constant rather than across engines, since a test
    /// comparing two private functions could only reach one of them.
    #[test]
    fn every_machine_carries_the_shared_mod_page() {
        for m in FmMachine::ALL {
            let mod_page: Vec<(u32, u8)> = FmEngine::machine_page_refs(m)
                .into_iter()
                .filter(|(_, r)| r.page.as_ref() == "MOD")
                .map(|(id, r)| (id, r.slot))
                .collect();
            let want: Vec<(u32, u8)> = LFO_PAGE_ORDER
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i as u8))
                .collect();
            assert_eq!(mod_page, want, "{m:?}'s MOD page");
        }
    }

    #[test]
    fn fm_kick_produces_audio_on_note_on() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_fm(&mut eng, &[make_note_on(36)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "FM kick should produce audio");
    }

    #[test]
    fn fm_kick_feedback_zero_vs_nonzero_timbre_differs() {
        let mut eng_nofb = FmEngine::kick();
        eng_nofb.activate(44100.0, 512);
        let cmds_nofb = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("feedback") as i64, arg1: 0.0 }];
        run_fm_cmds(&mut eng_nofb, &[], &cmds_nofb);
        let out_nofb = run_fm(&mut eng_nofb, &[make_note_on(36)]);

        let mut eng_fb = FmEngine::kick();
        eng_fb.activate(44100.0, 512);
        let cmds_fb = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("feedback") as i64, arg1: 0.8 }];
        run_fm_cmds(&mut eng_fb, &[], &cmds_fb);
        let out_fb = run_fm(&mut eng_fb, &[make_note_on(36)]);

        let differ = out_nofb.iter().zip(&out_fb).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "feedback=0 vs feedback=0.8 should produce different timbres");
    }

    #[test]
    fn fm_bell_decays_longer_than_kick_at_same_decay_param() {
        // Bell default decay = 2.0s; kick default decay = 0.5s.
        // After 8 blocks (≈93ms), bell should still have more energy.
        let mut bell = FmEngine::bell();
        bell.activate(44100.0, 512);
        let _ = run_fm(&mut bell, &[make_note_on(60)]);
        for _ in 0..8 { run_fm(&mut bell, &[]); }
        let bell_energy: f32 = run_fm(&mut bell, &[]).iter().map(|&x| x*x).sum();

        let mut kick = FmEngine::kick();
        kick.activate(44100.0, 512);
        let _ = run_fm(&mut kick, &[make_note_on(36)]);
        for _ in 0..8 { run_fm(&mut kick, &[]); }
        let kick_energy: f32 = run_fm(&mut kick, &[]).iter().map(|&x| x*x).sum();

        assert!(bell_energy > kick_energy,
            "bell should have more energy after 8 blocks: bell={bell_energy:.6} kick={kick_energy:.6}");
    }

    #[test]
    fn fm_bell_noninteger_ratio_differs_from_integer_ratio() {
        let mut eng_int = FmEngine::bell();
        eng_int.activate(44100.0, 512);
        let cmd_int = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("ratio") as i64, arg1: 2.0 }];
        run_fm_cmds(&mut eng_int, &[], &cmd_int);
        let out_int = run_fm(&mut eng_int, &[make_note_on(60)]);

        let mut eng_frac = FmEngine::bell();
        eng_frac.activate(44100.0, 512);
        let cmd_frac = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("ratio") as i64, arg1: 3.5 }];
        run_fm_cmds(&mut eng_frac, &[], &cmd_frac);
        let out_frac = run_fm(&mut eng_frac, &[make_note_on(60)]);

        let differ = out_int.iter().zip(&out_frac).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "ratio=2 vs ratio=3.5 should produce different spectra");
    }

    #[test]
    fn fm_bass_attack_param_changes_onset_slope() {
        // Short attack → louder at sample 5; long attack → quieter.
        let mut eng_fast = FmEngine::bass();
        eng_fast.activate(44100.0, 512);
        let cmds_fast = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("attack") as i64, arg1: 0.001 }];
        run_fm_cmds(&mut eng_fast, &[], &cmds_fast);
        let out_fast = run_fm(&mut eng_fast, &[make_note_on(36)]);

        let mut eng_slow = FmEngine::bass();
        eng_slow.activate(44100.0, 512);
        let cmds_slow = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("attack") as i64, arg1: 0.3 }];
        run_fm_cmds(&mut eng_slow, &[], &cmds_slow);
        let out_slow = run_fm(&mut eng_slow, &[make_note_on(36)]);

        // Fast attack → louder early onset
        assert!(out_fast[5].abs() > out_slow[5].abs(),
            "fast attack should be louder at onset: fast={:.4} slow={:.4}",
            out_fast[5].abs(), out_slow[5].abs());
    }

    #[test]
    fn fm_bass_drive_increases_rms_output() {
        let mut eng_no_drive = FmEngine::bass();
        eng_no_drive.activate(44100.0, 512);
        let out_no_drive = run_fm(&mut eng_no_drive, &[make_note_on(36)]);

        let mut eng_drive = FmEngine::bass();
        eng_drive.activate(44100.0, 512);
        let cmds = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fp("drive") as i64, arg1: 1.0 }];
        run_fm_cmds(&mut eng_drive, &[], &cmds);
        let out_drive = run_fm(&mut eng_drive, &[make_note_on(36)]);

        assert!(rms(&out_drive) > rms(&out_no_drive),
            "drive=1.0 should increase RMS: {:.4} vs {:.4}", rms(&out_drive), rms(&out_no_drive));
    }

    #[test]
    fn fm_param_lock_ratio_overrides_base_ratio() {
        let node_id = 55u32;
        let mut eng = FmEngine::bell();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        let out_base = run_fm(&mut eng, &[make_note_on(60)]);

        let lock_event = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: fp("ratio"), value: 7.0,
        }));
        let out_locked = run_fm(&mut eng, &[lock_event, make_note_on(60)]);

        let differ = out_base.iter().zip(&out_locked).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "param lock ratio=7.0 should change timbre vs base ratio=3.5");
    }

    #[test]
    fn fm_portability_check() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        assert!(!eng.ports().is_empty());
    }

    #[test]
    fn fm_engine_set_initial_params_applied() {
        let mut eng = FmEngine::bass();
        eng.set_node_id(1);
        eng.set_initial_params(&[("index".to_string(), 4.0)].into_iter().collect());
        eng.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k.ends_with("/index"));
        assert!(entry.is_some(), "published_state should contain /index");
        if let paraclete_node_api::StateBusValue::Float(v) = entry.unwrap().1 {
            assert!((v - 4.0).abs() < 1e-9, "index should be 4.0, got {v}");
        } else {
            panic!("index entry should be Float");
        }
    }

    // ── W1 Commit 0: CMD_TRIGGER + velocity plumbing ─────────────────────────

    #[test]
    fn cmd_trigger_produces_audio() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let cmd = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 1.0 };
        let out = run_fm_cmds(&mut eng, &[], &[cmd]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "CMD_TRIGGER should produce audio");
    }

    #[test]
    fn cmd_trigger_negative_note_uses_default() {
        let mut eng = FmEngine::kick();
        eng.activate(44100.0, 512);
        let cmd = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: -1, arg1: 1.0 };
        let out = run_fm_cmds(&mut eng, &[], &[cmd]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5),
            "CMD_TRIGGER with arg0<0 should use the default/last note and produce audio");
    }

    #[test]
    fn velocity_scales_output_level() {
        let mut eng_hi = FmEngine::kick();
        eng_hi.activate(44100.0, 512);
        let cmd_hi = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 1.0 };
        let out_hi = run_fm_cmds(&mut eng_hi, &[], &[cmd_hi]);
        let peak_hi = out_hi.iter().fold(0.0f32, |m, &x| m.max(x.abs()));

        let mut eng_lo = FmEngine::kick();
        eng_lo.activate(44100.0, 512);
        let cmd_lo = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 0.25 };
        let out_lo = run_fm_cmds(&mut eng_lo, &[], &[cmd_lo]);
        let peak_lo = out_lo.iter().fold(0.0f32, |m, &x| m.max(x.abs()));

        assert!(peak_hi > peak_lo,
            "higher velocity should produce a louder peak: hi={peak_hi:.4} lo={peak_lo:.4}");
        let ratio = peak_hi / peak_lo.max(1e-9);
        assert!(ratio > 2.0,
            "velocity ratio (1.0 vs 0.25) should roughly scale peak amplitude, got ratio={ratio:.2}");
    }

    #[test]
    fn note_on_mid_block_starts_at_its_sample_offset() {
        // BUG-013 regression (see AnalogEngine's twin test).
        let mut eng = FmEngine::bass();
        eng.activate(44100.0, 512);
        let mut ev = make_note_on(36);
        ev.sample_offset = 100;
        let out = run_fm(&mut eng, &[ev]);
        assert!(out[..100].iter().all(|&s| s == 0.0),
            "pre-offset span must be silent");
        assert!(out[100..].iter().any(|&s| s.abs() > 1e-6),
            "voice sounds from its offset");
    }


    #[test]
    fn param_lock_does_not_bleed_into_bank() {
        // BUG-015 regression: a per-step ParamLock is a per-cycle override,
        // never a bank write — the next unlocked step reads the original.
        let mut eng = FmEngine::kick();
        eng.set_node_id(9);
        eng.activate(44100.0, 512);
        let default_decay = eng.bank.get(fp("decay"));
        assert!(default_decay > 0.4, "kick decay default sanity");

        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id:  9,
            param_id: fp("decay"),
            value:    0.01,
        }));
        run_fm(&mut eng, &[lock, make_note_on(36)]);
        assert_eq!(eng.bank.get(fp("decay")), default_decay,
            "lock must not mutate the bank");
        run_fm(&mut eng, &[make_note_on(36)]);
        assert!(eng.node_locks.is_empty(),
            "an unlocked trigger retires the previous step's set (#169)");
        assert_eq!(eng.bank.get(fp("decay")), default_decay);
    }

    /// #169 (BUG-063), FM half. `drive` is re-read per render chunk
    /// (`process_kick`), so under the pre-#169 per-cycle clear the lock
    /// reverted to the bank ~11 ms in and only the trigger block was driven.
    /// See `AnalogEngine::consume_pending_locks` for the mechanism.
    ///
    /// `drive` and not `decay`: a `decay` lock short enough to be obvious
    /// ends the note *inside* the trigger block, leaving both tails silent
    /// and the assertion vacuously true — a mutant confirmed exactly that.
    #[test]
    fn a_p_lock_shapes_the_whole_note_not_just_its_first_block() {
        let rms = |v: &[f32]| (v.iter().map(|&x| x * x).sum::<f32>() / v.len() as f32).sqrt();

        let tail = |lock: bool| -> f32 {
            let mut eng = FmEngine::kick();
            eng.set_node_id(9);
            eng.activate(44100.0, 512);
            let mut events = Vec::new();
            if lock {
                events.push(TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
                    node_id: 9, param_id: fp("drive"), value: 1.0,
                })));
            }
            events.push(make_note_on(36));
            run_fm(&mut eng, &events);
            // Blocks 2..=6, well past the ~11 ms the old behaviour survived.
            let mut tail = Vec::new();
            for _ in 0..5 { tail.extend(run_fm(&mut eng, &[])); }
            rms(&tail)
        };

        let driven = tail(true);
        let clean = tail(false);
        assert!(driven > clean * 2.0,
            "a drive=1.0 lock must still be driving the note after the \
             trigger block: locked tail rms={driven:.5}, unlocked={clean:.5}");
    }

    /// State-level twin: the locked value persists with no fresh `ParamLock`.
    #[test]
    fn a_lock_survives_blocks_that_carry_no_events() {
        let mut eng = FmEngine::kick();
        eng.set_node_id(9);
        eng.activate(44100.0, 512);

        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id: 9, param_id: fp("decay"), value: 0.01,
        }));
        run_fm(&mut eng, &[lock, make_note_on(36)]);

        for block in 2..=8 {
            run_fm(&mut eng, &[]);
            assert!((eng.raw_param(fp("decay")) - 0.01).abs() < 1e-6,
                "block {block}: the lock must outlive the cycle it arrived in");
        }
    }

    /// #175 (BUG-066), FM half. `tune` was read only in `retrigger()`, so
    /// `lfo_dest = tune` was a per-note sample-and-hold rather than a sweep.
    /// See `AnalogEngine::lfo_tune_semitones` for why it is a delta on the
    /// latched base and not a per-span re-read. All three machines drive
    /// pitch through `swept_hz`, so one is enough to pin the mechanism.
    #[test]
    fn the_lfo_sweeps_pitch_within_one_note() {
        let mut eng = FmEngine::bass();
        eng.activate(44100.0, 512);
        for (name, v) in [
            ("lfo_dest", 1.0),  // tune
            ("lfo_depth", 1.0),
            ("lfo_speed", 4.0),
            ("lfo_shape", 0.0), // Tri
            ("lfo_mode", 1.0),  // Trig
        ] {
            eng.bank.set(fp(name), v);
        }
        eng.retrigger(60, 1.0);

        let mut seen: Vec<f32> = Vec::new();
        for _ in 0..8 {
            seen.push(eng.swept_hz());
            eng.render_span(0, 512);
        }
        let lo = seen.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = seen.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            hi > lo * 1.05,
            "pitch must move within the note, not freeze at retrigger: {seen:?}"
        );
    }

    /// A re-activate kills the voice, so it must kill the voice's locks.
    #[test]
    fn activate_retires_a_lock_in_flight() {
        let mut eng = FmEngine::kick();
        eng.set_node_id(9);
        eng.activate(44100.0, 512);

        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id: 9, param_id: fp("decay"), value: 0.01,
        }));
        run_fm(&mut eng, &[lock, make_note_on(36)]);
        assert!(!eng.node_locks.is_empty());

        eng.activate(44100.0, 512);
        assert!(eng.node_locks.is_empty(), "a rebuild must not carry a lock over");
        assert!(!eng.locks_pending);
    }
}
