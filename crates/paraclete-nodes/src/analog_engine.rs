use std::collections::HashMap;

use std::borrow::Cow;

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
    soft_clip, sub_blocks, svf_lp_sample, xorshift,
};

fn ap(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

// ── Machine variant ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnalogMachine { Kick, Snare, HiHat }

impl AnalogMachine {
    /// Declaration order, and therefore the `machine` param's value order.
    /// **Append-only** — a saved project stores the numeric value, so
    /// reordering this silently re-points every stored `machine` at a
    /// different engine.
    pub const ALL: [AnalogMachine; 3] = [
        AnalogMachine::Kick,
        AnalogMachine::Snare,
        AnalogMachine::HiHat,
    ];

    pub fn value(self) -> u32 {
        match self {
            AnalogMachine::Kick => 0,
            AnalogMachine::Snare => 1,
            AnalogMachine::HiHat => 2,
        }
    }

    /// Out-of-range values clamp to the last machine rather than panicking —
    /// this reads a `f64` bank slot that a malformed project could carry.
    pub fn from_value(v: u32) -> Self {
        *Self::ALL.get(v as usize).unwrap_or(&AnalogMachine::HiHat)
    }

    /// Capability-document name, shown as the engine name on a surface.
    pub fn doc_name(self) -> &'static str {
        match self {
            AnalogMachine::Kick => "AnalogKick",
            AnalogMachine::Snare => "AnalogSnare",
            AnalogMachine::HiHat => "AnalogHiHat",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AnalogMachine::Kick => "Kick",
            AnalogMachine::Snare => "Snare",
            AnalogMachine::HiHat => "HiHat",
        }
    }
}

/// MM §0 D1: a machine switch fades the voice to silence over this long,
/// then swaps and resets voice state. There is no matching fade-*in* —
/// ADR-041 decision 4 resets envelopes on switch, so the new machine starts
/// silent and its next trigger begins from zero anyway.
const MACHINE_SWITCH_FADE_SECS: f32 = 0.005;

/// An in-flight gain ramp around a machine switch.
///
/// There is no fade-*in* after a swap: ADR-041 decision 4 resets voice state,
/// and `render_span` early-returns while inactive, so the node emits exact
/// zeros until the next trigger — which starts from zero anyway. There is
/// provably nothing to fade in.
///
/// The ramp is still bidirectional, for a different reason: a switch that is
/// cancelled or retargeted part-way must return to unity *continuously*.
/// Jumping straight back is the same click the fade exists to prevent.
#[derive(Clone, Copy, Debug)]
struct SwitchFade {
    /// Samples of ramp left to run.
    remaining: u32,
    /// `Some` = fading out toward this machine; `None` = a cancelled switch
    /// ramping back to unity, which swaps nothing.
    target: Option<AnalogMachine>,
}

// ── AnalogEngine ──────────────────────────────────────────────────────────────

/// Analog drum voice synthesizer with three machine variants.
/// Event interface identical to Sampler — same graph wiring works.
/// Ports: events_in (0, Event), audio_out_l (1, Mono), audio_out_r (2, Mono).
pub struct AnalogEngine {
    machine:     AnalogMachine,
    bank:        ParameterBank,
    sample_rate: f32,

    osc_phase:   f32,
    pitch_env:   AdState,
    amp_env:     AdState,
    body_env:    AdState,
    noise_env:   AdState,
    noise_state: u32,
    hihat_noise: u32,
    svf_low:     f32,
    svf_band:    f32,
    current_hz:  f32,
    active:      bool,
    node_id:     u32,
    node_locks:  Vec<(u32, f64)>,
    /// #169 (BUG-063): set when a `ParamLock` arrives, consumed by the
    /// `retrigger()` it belongs to. See `consume_pending_locks`.
    locks_pending: bool,
    /// Note of the last retrigger — used as the CMD_TRIGGER default note (arg0 < 0).
    last_note:   u8,
    /// Linear output-level multiplier derived from trigger velocity (0.0..=1.0).
    /// 1.0 = unity gain (full velocity, matches pre-W1 output level).
    velocity_level: f32,

    /// In-flight gain ramp for a machine switch (MM §0 D1).
    switch_fade: Option<SwitchFade>,

    /// MM-C9: one LFO per hosting node (ADR-042 decision 1), ticked once per
    /// 64-sample sub-block in MM-C7's loop.
    lfo: LfoHost,

    render_l:    Vec<f32>,
    render_r:    Vec<f32>,

    pending_initial_params: HashMap<String, f64>,

    ports: [PortDescriptor; 3],
}

/// The dest table as **names**, in the same order as `AnalogEngine::LFO_DESTS`.
///
/// Two lists rather than one because `ParamDescriptor::id_for_name` is a
/// `const fn` over a `&str` but there is no stable way to map an array of
/// names to an array of ids in a `const` initialiser. A test asserts they
/// correspond entry-for-entry, so a drift fails loudly rather than mislabelling
/// an encoder.
/// #179: no longer the offered list, and there is no union label adapter any
/// more — the cap-doc carries the ACTIVE machine's labels
/// (`AnalogEngine::dest_labels`), because a machine-invariant list named
/// destinations the machine could not reach. This survives as the union
/// *envelope*: the set every per-machine table must draw from, which is what
/// keeps `LFO_DESTS.len()` — the width `lfo_dest` persists at — honest.
/// Referenced by the append-only tests.
#[allow(dead_code)]
static ANALOG_DEST_NAMES: &[&str] = &["tune", "tone", "decay", "punch", "drive", "snap", "noise", "open"];


// ── Per-machine destination tables (#179) ────────────────────────────────────
//
// **Each list is APPEND ONLY**, for the same reason `AnalogMachine::ALL` is:
// `lfo_dest` persists a one-based index into the *active machine's* list, so
// inserting or reordering an entry re-points every saved patch on that machine
// at a different param. `the_per_machine_dest_tables_are_append_only` pins all
// three heads so a reorder fails loudly rather than silently.
//
// A machine's list must name exactly the params that machine reads. The union
// table above stayed machine-invariant through MM, which meant three to five of
// its eight entries did nothing on any given machine — on a HiHat, five of
// eight — and selecting one was indistinguishable from a broken LFO. That cost
// most of session #5's LFO round. `every_machines_dests_are_exactly_its_params`
// asserts the correspondence in both directions, so neither an inert entry nor
// a missing one can be introduced.
//
// Declared as names rather than derived by filtering `ANALOG_DEST_NAMES`
// against `machine_params`, because a derived list is append-only only by
// accident: adding `punch` to HiHat would insert it *before* `open` in union
// order and shift the index of a param that was already there. Written out,
// the append-only contract is something you can see and a test can pin.
static KICK_DEST_NAMES:  &[&str] = &["tune", "punch", "decay", "drive", "tone"];
static SNARE_DEST_NAMES: &[&str] = &["tune", "snap", "noise", "decay", "tone"];
static HIHAT_DEST_NAMES: &[&str] = &["tone", "decay", "open"];

static KICK_DEST_LABELS:  LfoDestLabels = LfoDestLabels(KICK_DEST_NAMES);
static SNARE_DEST_LABELS: LfoDestLabels = LfoDestLabels(SNARE_DEST_NAMES);
static HIHAT_DEST_LABELS: LfoDestLabels = LfoDestLabels(HIHAT_DEST_NAMES);

impl AnalogEngine {
    pub const PORT_EVENTS_IN:   u32 = 0;
    pub const PORT_AUDIO_OUT_L: u32 = 1;
    pub const PORT_AUDIO_OUT_R: u32 = 2;

    pub fn new(machine: AnalogMachine) -> Self {
        let doc = Self::build_doc(machine);
        Self {
            machine,
            bank:        ParameterBank::from_capability_document(&doc),
            sample_rate: 44100.0,
            osc_phase:   0.0,
            pitch_env:   AdState::new(),
            amp_env:     AdState::new(),
            body_env:    AdState::new(),
            noise_env:   AdState::new(),
            noise_state: 1,
            hihat_noise: 1,
            svf_low:     0.0,
            svf_band:    0.0,
            current_hz:  65.41, // C2
            active:      false,
            node_id:     0,
            node_locks:  Vec::new(),
            locks_pending: false,
            last_note:   36, // C2 — matches current_hz's initial value
            velocity_level: 1.0,
            switch_fade: None,
            lfo:            LfoHost::new(),
            render_l:    Vec::new(),
            render_r:    Vec::new(),
            pending_initial_params: HashMap::new(),
            ports: Self::default_ports(),
        }
    }

    pub fn kick()  -> Self { Self::new(AnalogMachine::Kick)  }
    pub fn snare() -> Self { Self::new(AnalogMachine::Snare) }
    pub fn hihat() -> Self { Self::new(AnalogMachine::HiHat) }

    /// **APPEND ONLY.** `lfo_dest` stores a one-based index into this table,
    /// so inserting or reordering an entry silently re-points every saved
    /// patch at a different destination — the same contract as
    /// `AnalogMachine::ALL`, and the thing that makes storing the index as
    /// stable as storing a name-hash id (see `lfo_dest_param`).
    ///
    /// `lfo_*` params are absent by construction (no self-modulation, ADR-042
    /// review m14) and so is `machine` — modulating an identity param would be
    /// per-step machine switching by the back door (ADR-041 §0 A4).
    ///
    /// Machine-invariant: the table is the union across machines, so a dest
    /// keeps meaning the same param when the machine changes. A machine that
    /// does not read its destination simply hears nothing, which is the same
    /// thing that happens to any inert param.
    const LFO_DESTS: &'static [u32] = &[
        ParamDescriptor::id_for_name("tune"),
        ParamDescriptor::id_for_name("tone"),
        ParamDescriptor::id_for_name("decay"),
        ParamDescriptor::id_for_name("punch"),
        ParamDescriptor::id_for_name("drive"),
        ParamDescriptor::id_for_name("snap"),
        ParamDescriptor::id_for_name("noise"),
        ParamDescriptor::id_for_name("open"),
    ];

    /// Record a `ParamLock` for the trigger it precedes (#169).
    ///
    /// The first lock after a trigger *replaces* the set rather than adding to
    /// it, so a step's locks are exactly the locks that arrived for that step.
    /// Locks for one step always arrive as a contiguous run — the executor
    /// sorts `ParamLock` ahead of `Midi2` at equal offsets — so "have I seen a
    /// lock since the last retrigger?" is enough to tell a new run from a
    /// continuation, with no per-step bookkeeping on the audio thread.
    fn push_lock(&mut self, param_id: u32, value: f64) {
        if !self.locks_pending {
            self.node_locks.clear();
            self.locks_pending = true;
        }
        self.node_locks.push((param_id, value));
    }

    /// Hand the pending lock set to the note now starting, or end the previous
    /// note's set if this step carried none (#169).
    ///
    /// A p-lock owns its **note**, not the audio cycle it arrived in. Clearing
    /// per cycle (the pre-#169 behaviour) meant a lock survived ~11 ms at
    /// 44.1 kHz/512, so params latched at trigger time (`tune`, velocity) held
    /// it while params re-read per render span (`open`, `decay`, `tone`, the
    /// whole `lfo_*` block) reverted to the bank one cycle in — inaudible.
    ///
    /// The per-cycle clear's stated concern, "locks from one step must not
    /// bleed into steps that carry no lock", is still met, and by the exact
    /// event that defines the boundary: the next trigger.
    fn consume_pending_locks(&mut self) {
        if self.locks_pending {
            self.locks_pending = false;
        } else {
            self.node_locks.clear();
        }
    }

    /// Bank/lock value, **before** the LFO. Used for the `lfo_*` params
    /// themselves, so an LFO can never modulate its own controls even if a
    /// dest table were mis-declared.
    fn raw_param(&self, param_id: u32) -> f32 {
        for &(id, val) in &self.node_locks {
            if id == param_id { return val as f32; }
        }
        self.bank.get(param_id) as f32
    }

    /// Parameter read honoring per-cycle ParamLock overrides **and** the LFO.
    ///
    /// ADR-042 amendment 1: the base is the `get_param()` result, i.e. the
    /// p-locked value when a lock is in force, so the LFO breathes on top of a
    /// locked step rather than replacing it — locks never defeat the LFO nor
    /// vice versa. The bank itself is untouched, so p-locks, the state bus and
    /// (later) kits all still see the base, and `CMD_BUMP_PARAM` reads do not
    /// feed back on themselves.
    fn get_param(&self, param_id: u32) -> f32 {
        self.lfo.apply(param_id, self.raw_param(param_id))
    }

    /// The LFO's current contribution to `tune`, in semitones (#175).
    ///
    /// `tune` is the one destination that is not read per render span:
    /// `retrigger()` latches it into `current_hz`, and that is deliberate —
    /// a p-lock on `tune` must set the note's pitch outright, not wobble it,
    /// and pitch has to survive for the note the way velocity does. The cost
    /// was that `lfo_dest = tune` sampled the LFO once at the trigger instant
    /// and held it: a per-note sample-and-hold, not a sweep. In `Free` that
    /// read as a wandering per-hit pitch; in `Trig`, because `retrigger()`
    /// read `tune` *before* resetting the LFO phase, every hit after the
    /// first sampled the same drift and the pattern came out identical.
    ///
    /// So the LFO is applied here as a **delta on top of the latched base**
    /// rather than by making `tune` a per-span read: `retrigger()` latches
    /// the LFO-free value (`raw_param`) and the render multiplies by this.
    /// Both properties hold at once — the lock owns the note's pitch, the
    /// LFO sweeps within it — and this is how `punch` already bends pitch.
    ///
    /// Taken as a difference rather than read raw so `apply`'s clamp to the
    /// dest range still governs: the swept pitch cannot leave the range
    /// `tune` itself declares.
    fn lfo_tune_semitones(&self) -> f32 {
        let base = self.raw_param(ap("tune"));
        self.lfo.apply(ap("tune"), base) - base
    }

    /// `current_hz` with the LFO's `tune` sweep applied (#175). 1.0 semitone
    /// of delta is one semitone of pitch, matching `tune`'s own unit.
    fn swept_hz(&self) -> f32 {
        let semis = self.lfo_tune_semitones();
        if semis == 0.0 {
            return self.current_hz;
        }
        self.current_hz * 2.0f32.powf(semis / 12.0)
    }

    /// The `lfo_*` params as the block wants them, read raw.
    fn lfo_settings(&self) -> LfoSettings {
        LfoSettings {
            shape: LfoShape::from_value(self.raw_param(ap("lfo_shape"))),
            mode: LfoMode::from_value(self.raw_param(ap("lfo_mode"))),
            speed_hz: self.raw_param(ap("lfo_speed")),
            start_phase: self.raw_param(ap("lfo_start_phase")),
            fade: self.raw_param(ap("lfo_fade")),
        }
    }

    /// The destination names offered on `machine` (#179), in index order.
    fn dest_names(machine: AnalogMachine) -> &'static [&'static str] {
        match machine {
            AnalogMachine::Kick => KICK_DEST_NAMES,
            AnalogMachine::Snare => SNARE_DEST_NAMES,
            AnalogMachine::HiHat => HIHAT_DEST_NAMES,
        }
    }

    fn dest_labels(machine: AnalogMachine) -> &'static LfoDestLabels {
        match machine {
            AnalogMachine::Kick => &KICK_DEST_LABELS,
            AnalogMachine::Snare => &SNARE_DEST_LABELS,
            AnalogMachine::HiHat => &HIHAT_DEST_LABELS,
        }
    }

    /// Resolve `lfo_dest` to a param id, or `None` for off.
    ///
    /// One-based against the **active machine's** table (#179): value 1 is
    /// `dest_names(machine)[0]`. An out-of-range index reads as off rather
    /// than clamping to a neighbour — a malformed value, or one belonging to
    /// a machine with a longer list, must not quietly modulate something.
    fn lfo_dest_id(&self) -> Option<u32> {
        let v = self.raw_param(ap("lfo_dest"));
        if !v.is_finite() || v < 1.0 {
            return None;
        }
        Self::dest_names(self.machine)
            .get(v as usize - 1)
            .map(|n| ap(n))
    }

    /// Advance the LFO one sub-block and latch what it modulates.
    fn update_lfo(&mut self, samples: usize) {
        let dest = self.lfo_dest_id();
        // Only meaningful when there IS a destination; `update` ignores the
        // range when `dest` is `None`.
        let range = dest
            .and_then(|d| self.bank.range(d))
            .map(|(lo, hi)| (lo as f32, hi as f32))
            .unwrap_or((0.0, 1.0));
        let depth = self.raw_param(ap("lfo_depth"));
        let settings = self.lfo_settings();
        let sr = self.sample_rate;
        self.lfo.update(settings, dest, range, depth, sr, samples);
    }

    /// The params one machine actually reads. **Not** what the bank stores —
    /// see `union_params`.
    fn machine_params(machine: AnalogMachine) -> Vec<ParamDescriptor> {
        match machine {
            AnalogMachine::Kick => vec![
                ParamDescriptor { id: ap("tune"),  name: "tune".into(),  min: -24.0, max: 24.0,   default: 0.0,   stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: ap("punch"), name: "punch".into(), min: 0.0,   max: 1.0,    default: 0.7,   stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: ap("decay"), name: "decay".into(), min: 0.01,  max: 2.0,    default: 0.5,   stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: ap("drive"), name: "drive".into(), min: 0.0,   max: 1.0,    default: 0.0,   stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: ap("tone"),  name: "tone".into(),  min: 200.0, max: 8000.0, default: 4000.0, stepped: false, unit: ParamUnit::Hz,        display: None },
            ],
            AnalogMachine::Snare => vec![
                ParamDescriptor { id: ap("tune"),  name: "tune".into(),  min: -24.0, max: 24.0,    default: 0.0,  stepped: false, unit: ParamUnit::Semitones, display: None },
                ParamDescriptor { id: ap("snap"),  name: "snap".into(),  min: 0.005, max: 0.3,     default: 0.05, stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: ap("noise"), name: "noise".into(), min: 0.0,   max: 1.0,     default: 0.5,  stepped: false, unit: ParamUnit::Generic,   display: None },
                ParamDescriptor { id: ap("decay"), name: "decay".into(), min: 0.01,  max: 2.0,     default: 0.3,  stepped: false, unit: ParamUnit::Seconds,   display: None },
                ParamDescriptor { id: ap("tone"),  name: "tone".into(),  min: 200.0, max: 8000.0,  default: 2000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
            ],
            AnalogMachine::HiHat => vec![
                ParamDescriptor { id: ap("tone"),  name: "tone".into(),  min: 1000.0, max: 18000.0, default: 8000.0, stepped: false, unit: ParamUnit::Hz,      display: None },
                ParamDescriptor { id: ap("decay"), name: "decay".into(), min: 0.01,   max: 1.0,     default: 0.08,   stepped: false, unit: ParamUnit::Seconds, display: None },
                ParamDescriptor { id: ap("open"),  name: "open".into(),  min: 0.0,    max: 1.0,     default: 0.0,    stepped: false, unit: ParamUnit::Generic, display: None },
            ],
        }
    }

    /// The bank's parameter set: **every machine's params merged at the widest
    /// envelope**, plus `machine` itself (ADR-041 §0 A1).
    ///
    /// `active` picks each param's *default* — and nothing else. Ranges are the
    /// union unconditionally, for the lifetime of the node. Narrowing them to
    /// the active machine is the phase's one unrecoverable mistake: writes
    /// clamp to the bank's range (`parameter.rs:73,78,97`), and `deserialize()`
    /// runs after `activate()` through the same clamping `set()`, so a narrowed
    /// bank truncates every value belonging to a machine that is not currently
    /// selected — on **load** — and persists that on the next save.
    ///
    /// Per-machine ranges live in `MachineVariant::overlays` and are a surface
    /// concern; this engine never sees them.
    fn union_params(active: AnalogMachine) -> Vec<ParamDescriptor> {
        let mut out: Vec<ParamDescriptor> = vec![ParamDescriptor {
            id: ap("machine"),
            name: "machine".into(),
            min: 0.0,
            max: (AnalogMachine::ALL.len() - 1) as f64,
            default: active.value() as f64,
            stepped: true,
            unit: ParamUnit::Generic,
            display: None,
        }];

        // MM-C9: the seven `lfo_*` params are machine-invariant — one LFO per
        // node, not per machine — so they join the union once rather than
        // through the per-machine merge below.
        // #179: the WIDTH stays the union — narrowing the bank to the active
        // machine is the mistake this function's header warns about, and it
        // would truncate a `lfo_dest` belonging to a machine with a longer
        // list on load. The LABELS are the active machine's, so only its own
        // destinations are named; `machine_overlays` narrows what the encoder
        // can reach. Indices between the two are gaps by construction.
        out.extend(lfo_params(
            Self::LFO_DESTS.len(),
            Some(Self::dest_labels(active)),
        ));

        for m in AnalogMachine::ALL {
            for p in Self::machine_params(m) {
                match out.iter_mut().find(|q| q.id == p.id) {
                    Some(q) => {
                        q.min = q.min.min(p.min);
                        q.max = q.max.max(p.max);
                        // The active machine's default wins; otherwise the
                        // first declarer's stands, so a param the active
                        // machine does not read still has a sane starting
                        // value for when it is selected.
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

    fn build_doc(machine: AnalogMachine) -> CapabilityDocument {
        CapabilityDocument {
            // The active machine's name — a surface shows what is selected.
            // This is the only other place `machine` is read here, and it
            // touches no range.
            name: machine.doc_name().into(),
            vendor: "Paraclete".into(),
            version: (0, 6, 0),
            ports: Self::default_ports().to_vec(),
            params: Self::union_params(machine),
            extensions: vec!["paraclete.instrument".into()],
            view: None,
        }
    }

    /// Has the `machine` param been moved? Called at the block boundary, so a
    /// switch never lands mid-sub-block (ADR-041 decision 4).
    ///
    /// Reads the **bank**, not `get_param` — a `ParamLock` on `machine` must
    /// never switch machines mid-step. Decision 6 forbids p-locking it and
    /// MM-C6 rejects it surface-side; this is the engine-side belt to that
    /// brace, because the sequencer stores opaque `(node_id, param_id)` locks
    /// and cannot know it is holding an identity param.
    fn poll_machine_param(&mut self) {
        let target = AnalogMachine::from_value(self.bank.get(ap("machine")).max(0.0) as u32);
        let total = self.fade_len();

        if target == self.machine {
            // Moved back before the fade finished. Ramping straight to unity
            // would step the gain by however far the fade had already got —
            // the very artefact this exists to prevent — so ramp back up from
            // wherever it is.
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
            // Nothing sounding, nothing to declick.
            self.apply_machine_switch(target);
            self.switch_fade = None;
            return;
        }

        // Retarget mid-fade keeps the gain it has already reached; taking a
        // fresh full-length fade would jump back to unity.
        let remaining = match self.switch_fade {
            Some(f) if f.target.is_some() => f.remaining.min(total),
            // A cancel-ramp in flight is at gain (total - remaining)/total;
            // converting back to a fade-out preserves that level.
            Some(f) => total.saturating_sub(f.remaining).max(1),
            None => total,
        };
        self.switch_fade = Some(SwitchFade {
            remaining,
            target: Some(target),
        });
    }

    fn fade_len(&self) -> u32 {
        (MACHINE_SWITCH_FADE_SECS * self.sample_rate).max(1.0) as u32
    }

    /// Ramp the rendered block, and once a fade-out reaches silence, swap.
    ///
    /// A `target: None` ramp is a cancelled switch on its way back to unity —
    /// it changes no machine, it just restores gain continuously.
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
            self.switch_fade = Some(SwitchFade {
                remaining: left,
                target: fade.target,
            });
        }
    }

    /// ADR-041 decision 4: voice state resets on switch.
    ///
    /// **The bank is not rebuilt.** Rebuilding is `activate()`'s job and it
    /// resets every slot to defaults — the same cross-machine data loss the
    /// union bank exists to prevent, by a different route (MM §3.5).
    /// #179: `lfo_dest` is deliberately **not** translated across a switch.
    ///
    /// The index keeps its value and its meaning follows the machine, so a
    /// dest the new machine does not offer reads as off (`lfo_dest_id`
    /// bounds-checks against the active table) and comes back unchanged on
    /// switching away and back. Remapping by param name was tried and is
    /// worse: a destination the other machine lacks has nowhere to go, so it
    /// collapses to off and a Kick -> HiHat -> Kick round trip loses it —
    /// breaking the losslessness MM §6.2 guarantees and
    /// `machine_round_trip_preserves_every_param` pins. Leaving the number
    /// alone costs a re-pointed LFO on a live switch and nothing else.
    fn apply_machine_switch(&mut self, target: AnalogMachine) {
        self.machine = target;
        self.osc_phase = 0.0;
        self.pitch_env = AdState::new();
        self.amp_env = AdState::new();
        self.body_env = AdState::new();
        self.noise_env = AdState::new();
        self.svf_low = 0.0;
        self.svf_band = 0.0;
        self.active = false;
    }

    fn retrigger(&mut self, note: u8, velocity: f32) {
        // Before any param read: this note's lock set is now final.
        self.consume_pending_locks();
        // #175: `raw_param`, not `get_param` — the LFO is applied per render
        // span by `swept_hz`, so reading it here too would double-count it and
        // re-freeze the sweep into the note. The p-lock still lands: a lock on
        // `tune` sets this note's pitch, and the sweep rides on top.
        let tune = self.raw_param(ap("tune"));
        self.current_hz = note_to_hz(note, tune);
        self.last_note = note;
        self.velocity_level = velocity.clamp(0.0, 1.0);
        self.pitch_env.trigger();
        self.amp_env.trigger();
        self.body_env.trigger();
        self.noise_env.trigger();
        self.osc_phase = 0.0;
        self.active    = true;
        self.lfo.trigger(self.lfo_settings());
    }

    /// Render `[start, end)` with the current voice state, dispatched by
    /// machine. A no-op span (or inactive voice) leaves the zeroed buffer.
    ///
    /// MM-C7: the span is chunked into `LFO_SUB_BLOCK` pieces **relative to
    /// `start`**, so a 100-sample span renders as 64 + 36 wherever it sits in
    /// the block. `process_*` re-reads its params per chunk, which is what a
    /// later commit's LFO needs and, with nothing modulating, changes no
    /// sample: the same constant params give the same coefficients.
    ///
    /// The voice is **not** cut short when `active` goes false mid-span, and
    /// `if !self.active { break; }` is not the free optimisation it looks
    /// like. The skipped samples really would be silence — the buffers are
    /// zeroed at the top of every block and an idle `AdState` returns 0.0 —
    /// but `process_snare` and `process_hihat` advance an xorshift LFSR once
    /// per sample (`self.noise_state`, `self.hihat_noise`). Skipping samples
    /// skips those advances, so **every later note gets a different noise
    /// sequence**.
    ///
    /// Measured, not reasoned: with the break in place the first hihat note is
    /// bit-identical and `analog_machines` drifts from the *second* one
    /// onward. Neither `kick_reverb_clean` nor `fm_machines` notices — no
    /// noise in either voice — so the only thing standing between this and a
    /// silent regression is that one baseline.
    fn render_span(&mut self, start: usize, end: usize) {
        if start >= end || !self.active {
            return;
        }
        for (lo, hi) in sub_blocks(start, end) {
            // MM-C9: the LFO advances once per sub-block, before the machine
            // reads its params — that is the whole reason MM-C7 built this
            // loop, since a `process_*` reads every param once up front.
            self.update_lfo(hi - lo);
            match self.machine {
                AnalogMachine::Kick  => self.process_kick(lo, hi),
                AnalogMachine::Snare => self.process_snare(lo, hi),
                AnalogMachine::HiHat => self.process_hihat(lo, hi),
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
        let punch    = self.get_param(ap("punch"));
        let decay_s  = self.get_param(ap("decay"));
        let drive    = self.get_param(ap("drive"));
        let tone_hz  = self.get_param(ap("tone"));
        let sr = self.sample_rate;

        let pitch_attack_inc  = 1.0 / (0.002 * sr);
        let pitch_decay_coeff = 0.001f32.powf(1.0 / (0.08 * sr));
        let amp_attack_inc    = 1.0 / (0.001 * sr);
        let amp_decay_coeff   = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let f_svf = (std::f32::consts::PI * tone_hz / sr).sin().clamp(0.0, 0.99);
        // #175: hoisted with the other per-chunk param reads, so the sweep
        // advances once per LFO sub-block exactly like every other dest.
        let swept_hz = self.swept_hz();

        for i in start..end {
            let pitch_val = self.pitch_env.tick(pitch_attack_inc, pitch_decay_coeff);
            let freq = swept_hz * 2.0f32.powf(pitch_val * punch * 24.0 / 12.0);
            let phase_inc = (freq / sr).clamp(0.0, 0.5);
            self.osc_phase = (self.osc_phase + phase_inc).fract();
            let osc = (self.osc_phase * std::f32::consts::TAU).sin();
            let amp = self.amp_env.tick(amp_attack_inc, amp_decay_coeff);
            let sig = svf_lp_sample(osc * amp, f_svf, 0.7, &mut self.svf_low, &mut self.svf_band);
            let out = soft_clip(sig * (1.0 + drive * 9.0));
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }

    fn process_snare(&mut self, start: usize, end: usize) {
        let snap_s   = self.get_param(ap("snap"));
        let noise_lvl= self.get_param(ap("noise"));
        let decay_s  = self.get_param(ap("decay"));
        let tone_hz  = self.get_param(ap("tone"));
        let sr = self.sample_rate;

        let body_attack_inc   = 1.0 / (0.001 * sr);
        let body_decay_coeff  = 0.001f32.powf(1.0 / (snap_s * sr).max(1.0));
        let noise_attack_inc  = 1.0 / (0.001 * sr);
        let noise_decay_coeff = 0.001f32.powf(1.0 / (decay_s * sr).max(1.0));
        let f_svf = (std::f32::consts::PI * tone_hz / sr).sin().clamp(0.0, 0.99);
        let swept_hz = self.swept_hz(); // #175

        for i in start..end {
            let body_amp = self.body_env.tick(body_attack_inc, body_decay_coeff);
            self.osc_phase = (self.osc_phase + swept_hz / sr).fract();
            let body = (self.osc_phase * std::f32::consts::TAU).sin() * body_amp;

            let noise_raw = xorshift(&mut self.noise_state);
            let noise_amp = self.noise_env.tick(noise_attack_inc, noise_decay_coeff);
            // Bandpass SVF: use band output
            self.svf_low  += f_svf * self.svf_band;
            self.svf_band += f_svf * (noise_raw - self.svf_low - 1.0 * self.svf_band);
            let noise_out = self.svf_band * noise_amp * noise_lvl;

            let out = soft_clip(body + noise_out);
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.body_env.is_idle() || !self.noise_env.is_idle();
    }

    fn process_hihat(&mut self, start: usize, end: usize) {
        let tone_hz = self.get_param(ap("tone"));
        let decay_s = self.get_param(ap("decay"));
        let open    = self.get_param(ap("open"));
        let sr = self.sample_rate;

        let effective_decay = decay_s * (1.0 + open * 7.0);
        let amp_attack_inc  = 1.0 / (0.0005 * sr);
        let amp_decay_coeff = 0.001f32.powf(1.0 / (effective_decay * sr).max(1.0));
        let f_svf = (std::f32::consts::PI * tone_hz / sr).sin().clamp(0.0, 0.99);

        for i in start..end {
            let noise_raw = xorshift(&mut self.hihat_noise);
            self.svf_low  += f_svf * self.svf_band;
            self.svf_band += f_svf * (noise_raw - self.svf_low - 0.5 * self.svf_band);
            let hp_out = noise_raw - self.svf_low;
            let amp = self.amp_env.tick(amp_attack_inc, amp_decay_coeff);
            let out = hp_out * amp;
            self.render_l[i] = out;
            self.render_r[i] = out;
        }
        self.active = !self.amp_env.is_idle();
    }
}

impl AnalogEngine {
    /// One machine's page placements. `tune` is deliberately absent for HiHat,
    /// which does not declare it — the unconditional `tune` at SRC slot 0 in
    /// the pre-MM code is half of #47 (BUG-037), and per-machine pages are how
    /// MM-C4 closes that class.
    fn machine_page_refs(machine: AnalogMachine) -> Vec<(u32, PageRef)> {
        // MM-C6 item 2 / ADR-041 amendment 2: machine-select lives on the
        // TRIG page. Declared by the engine, not synthesized by one surface,
        // so every surface inherits it through the machinery MM-C5 built —
        // and so `machine` stops being a declared param that no page reaches.
        // Slot 0 of TRIG on every machine: a shared param never moves.
        let mut refs = vec![
            (ap("machine"), PageRef { page: Cow::Borrowed("TRIG"), slot: 0 }),
            (ap("decay"), PageRef { page: Cow::Borrowed("AMP"), slot: 0 })];
        // MM-C9: the LFO is one full encoder page (ADR-042 decision 1) and is
        // machine-invariant, so every variant carries the same MOD block.
        // Placement lands here rather than in MM-C11 because a declared param
        // that no page reaches is exactly what MM-C8b's assertion refuses.
        for (i, id) in LFO_PAGE_ORDER.iter().enumerate() {
            refs.push((*id, PageRef { page: Cow::Borrowed("MOD"), slot: i as u8 }));
        }
        match machine {
            AnalogMachine::Kick => {
                refs.push((ap("tune"),  PageRef { page: Cow::Borrowed("SRC"), slot: 0 }));
                refs.push((ap("tone"),  PageRef { page: Cow::Borrowed("SRC"), slot: 1 }));
                refs.push((ap("punch"), PageRef { page: Cow::Borrowed("SRC"), slot: 2 }));
                refs.push((ap("drive"), PageRef { page: Cow::Borrowed("SRC"), slot: 3 }));
            }
            AnalogMachine::Snare => {
                refs.push((ap("tune"),  PageRef { page: Cow::Borrowed("SRC"), slot: 0 }));
                refs.push((ap("tone"),  PageRef { page: Cow::Borrowed("SRC"), slot: 1 }));
                refs.push((ap("snap"),  PageRef { page: Cow::Borrowed("SRC"), slot: 2 }));
                refs.push((ap("noise"), PageRef { page: Cow::Borrowed("SRC"), slot: 3 }));
            }
            AnalogMachine::HiHat => {
                refs.push((ap("tone"), PageRef { page: Cow::Borrowed("SRC"), slot: 1 }));
                refs.push((ap("open"), PageRef { page: Cow::Borrowed("SRC"), slot: 2 }));
            }
        }
        refs
    }

    /// This machine's per-param ranges, for a surface to display and clamp
    /// against. The bank stores the union and is never narrowed to these.
    fn machine_overlays(machine: AnalogMachine) -> Vec<(u32, ParamOverlay)> {
        let mut out: Vec<(u32, ParamOverlay)> = vec![(
            ap("machine"),
            ParamOverlay {
                min: 0.0,
                max: (AnalogMachine::ALL.len() - 1) as f64,
                default: machine.value() as f64,
                // Repeated in every variant on purpose: miss one and p-lock
                // rejection stops working for that machine alone. MM-C8
                // asserts the flag is consistent across variants.
                identity: true,
            },
        )];
        // #179: the dest encoder reaches only this machine's destinations.
        // The bank keeps the union width so nothing truncates on load; this is
        // the surface-facing narrowing, and it is what stops a performer
        // selecting a destination the machine does not read.
        out.push((
            ap("lfo_dest"),
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
                ParamOverlay {
                    min: p.min,
                    max: p.max,
                    default: p.default,
                    identity: false,
                },
            ));
        }
        out
    }

    fn machine_variants() -> Vec<MachineVariant> {
        AnalogMachine::ALL
            .iter()
            .map(|&m| MachineVariant {
                value: m.value(),
                name: Cow::Borrowed(m.doc_name()),
                page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC"), Cow::Borrowed("AMP"), Cow::Borrowed("MOD")]),
                pages: Cow::Owned(Self::machine_page_refs(m)),
                overlays: Cow::Owned(Self::machine_overlays(m)),
            })
            .collect()
    }
}

impl ViewPlugin for AnalogEngine {
    fn to_rule(&self, _node_id: u64, _sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule {
        let display_name = self.machine.display_name();
        let (decay_id, tone_id) = (ap("decay"), ap("tone"));

        // Base fields stay the ACTIVE machine's, so a consumer that ignores
        // `variants` renders exactly what it did before (ADR-041 decision 3).
        // MM-C5 teaches composite assembly to prefer the variant.
        let page_refs = Self::machine_page_refs(self.machine);

        let affordances = vec![
            (decay_id, AffordanceHint::EnvelopeCurve { group_idx: 0 }),
            (tone_id,  AffordanceHint::FilterShape),
            // MM-C11: the first real `LfoShape` declaration in the tree,
            // closing the known ADR-032 §2.6.5 gap — the hint existed and
            // nothing had ever emitted it.
            (ap("lfo_shape"), AffordanceHint::LfoShape),
        ];

        let env = EnvelopeGroup {
            env_type: Cow::Borrowed("AD"),
            label: Cow::Borrowed("Amp Envelope"),
            param_ids: [ap("decay"), 0, 0, 0],
        };

        Rule {
            name: Cow::Borrowed(display_name),
            page_groups: Cow::Owned(vec![Cow::Borrowed("TRIG"), Cow::Borrowed("SRC"), Cow::Borrowed("AMP"), Cow::Borrowed("MOD")]),
            param_pages: Cow::Owned(page_refs),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Owned(affordances),
            envelopes: Cow::Owned(vec![env]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Owned(Self::machine_variants()),
        }
    }
}

impl Node for AnalogEngine {
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
    /// The bank is the whole of this node's persistable state — everything
    /// else (envelopes, oscillator phase, the switch fade) is transient DSP
    /// that `activate()` resets. Until #154 this node inherited the trait
    /// default and every sound edit was lost on save.
    ///
    /// `machine` is a bank slot, so the selected machine round-trips with the
    /// rest. It lands on the first `process()`: `activate()` leaves
    /// `active == false`, so `poll_machine_param` snaps rather than fading —
    /// there is nothing sounding to declick at load time.
    ///
    /// The **union** bank is what makes this lossless. `deserialize` clamps
    /// each value to its slot range (`bank.set`), and the range is the union
    /// across machines and is never narrowed (MM §3.4) — so a value belonging
    /// to a machine that is not currently selected survives the round trip
    /// instead of being truncated to the active machine's ceiling.
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
        // ADR-042 decision 7: surfaces draw a live LFO position from this.
        buf.push((
            format!("/node/{}/state/lfo_phase", self.node_id),
            StateBusValue::Float(self.lfo.phase() as f64),
        ));
    }

    fn activate(&mut self, sample_rate: f32, block_size: usize) {
        self.sample_rate = sample_rate;
        let doc = Self::build_doc(self.machine);
        self.bank        = ParameterBank::from_capability_document(&doc);
        // BUG-008 fix: consume the pending map so a re-activate (dynamic
        // topology rebuild, P9 C4) cannot overwrite deserialized state.
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, value);
            }
        }
        self.render_l    = vec![0.0; block_size];
        self.render_r    = vec![0.0; block_size];
        self.osc_phase   = 0.0;
        self.svf_low     = 0.0;
        self.svf_band    = 0.0;
        self.pitch_env   = AdState::new();
        self.amp_env     = AdState::new();
        self.body_env    = AdState::new();
        self.noise_env   = AdState::new();
        self.noise_state = 1;
        self.hihat_noise = 1;
        self.current_hz  = 65.41;
        self.active      = false;
        self.last_note   = 36;
        self.velocity_level = 1.0;
        self.switch_fade = None;
        // #169: locks outlive a block now, so a re-activate must retire them —
        // otherwise a lock survives the topology rebuild that killed its note.
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

        // #169: `node_locks` deliberately survives the block boundary — a lock
        // belongs to its note, and a note outlives the cycle it starts in. Only
        // the "a lock arrived for the next trigger" flag is per-cycle; the set
        // itself is retired by `consume_pending_locks` at the next retrigger.
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
        // (sample_offset, priority), ParamLock before NoteOn at equal
        // offsets). A NoteOn mid-block splits the render at its offset
        // (BUG-013): the span before it plays the old voice state, the
        // retrigger applies, and rendering resumes — voice starts are
        // sample-accurate instead of quantized to the block boundary, which
        // is what makes micro-timing and swing audible in audio.
        let mut cursor = 0usize;
        for timed in input.events {
            match timed.event {
                Event::ParamLock(ref pl) if pl.node_id == self.node_id => {
                    self.push_lock(pl.param_id, pl.value);
                }
                Event::Midi2(ref ump) => {
                    if let UmpMessage::ChannelVoice2(cv2) = ump {
                        match cv2 {
                            ChannelVoice2::NoteOn(n) => {
                                let off = timed.sample_offset as usize;
                                if off > cursor {
                                    self.render_span(cursor, off);
                                    cursor = off;
                                }
                                let velocity = n.velocity() as f32 / 65535.0;
                                self.retrigger(u8::from(n.note_number()), velocity);
                                output.emit_debug(off as u32, DebugEventKind::VoiceTrigger, u8::from(n.note_number()) as i64, velocity as f64);
                            }
                            ChannelVoice2::NoteOff(_) => {}
                            _ => {}
                        }
                    }
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

    fn run_engine(eng: &mut AnalogEngine, events: &[TimedEvent]) -> Vec<f32> {
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

    fn run_engine_cmds(eng: &mut AnalogEngine, events: &[TimedEvent], cmds: &[NodeCommand]) -> Vec<f32> {
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

    // ── BUG-022: sequenced note vs live-trigger pitch unity ──────────────────
    #[test]
    fn seq_note_36_and_default_trigger_fire_same_pitch() {
        // A sequenced NoteOn at the engine's reference note (36, what the
        // instrument file's default_note now emits) must land on the same
        // frequency as a bare CMD_TRIGGER (arg0 < 0 → last_note default).
        let mut via_note = AnalogEngine::kick();
        via_note.activate(44100.0, 512);
        run_engine(&mut via_note, &[make_note_on(36)]);

        let mut via_trigger = AnalogEngine::kick();
        via_trigger.activate(44100.0, 512);
        run_engine_cmds(&mut via_trigger, &[], &[NodeCommand {
            target_id: 0, type_id: CMD_TRIGGER, arg0: -1, arg1: 0.0,
        }]);

        assert!(
            (via_note.current_hz - via_trigger.current_hz).abs() < 1e-3,
            "sequenced 36 ({} Hz) and default trigger ({} Hz) must match",
            via_note.current_hz, via_trigger.current_hz
        );
    }

    // ── BUG-023: fast-retrigger ducking measurement harness ─────────────────
    /// Render `blocks` blocks and return the peak |sample| observed.
    fn render_peak(eng: &mut AnalogEngine, blocks: usize, trigger_first: bool) -> f32 {
        let mut peak = 0.0f32;
        for b in 0..blocks {
            let cmds = if b == 0 && trigger_first {
                vec![NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: -1, arg1: 0.0 }]
            } else {
                vec![]
            };
            let out = run_engine_cmds(eng, &[], &cmds);
            for s in out {
                peak = peak.max(s.abs());
            }
        }
        peak
    }

    /// Render `blocks` blocks; return (rms, peak) over the span.
    fn render_energy(eng: &mut AnalogEngine, blocks: usize) -> (f32, f32) {
        let mut sum_sq = 0.0f64;
        let mut n = 0usize;
        let mut peak = 0.0f32;
        for _ in 0..blocks {
            let out = run_engine_cmds(eng, &[], &[]);
            for s in out {
                sum_sq += (s as f64) * (s as f64);
                n += 1;
                peak = peak.max(s.abs());
            }
        }
        (((sum_sq / n.max(1) as f64) as f32).sqrt(), peak)
    }

    fn trigger_now(eng: &mut AnalogEngine) {
        run_engine_cmds(eng, &[], &[NodeCommand {
            target_id: 0, type_id: CMD_TRIGGER, arg0: -1, arg1: 0.0,
        }]);
    }

    /// BUG-023 exploratory probe — `cargo test -p paraclete-nodes probe_rehit -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_rehit_energy_vs_gap() {
        let mut fresh = AnalogEngine::kick();
        fresh.activate(44100.0, 512);
        trigger_now(&mut fresh);
        let (rms_ref, peak_ref) = render_energy(&mut fresh, 9);

        for gap_blocks in [1usize, 2, 4, 8, 17, 43, 86] {
            let mut eng = AnalogEngine::kick();
            eng.activate(44100.0, 512);
            trigger_now(&mut eng);
            for _ in 0..gap_blocks { let _ = run_engine_cmds(&mut eng, &[], &[]); }
            trigger_now(&mut eng);
            let (rms, peak) = render_energy(&mut eng, 9);
            println!(
                "gap {:>3} blocks ({:>4} ms): rehit rms {:.4} ({:>5.1}% of ref) peak {:.3} ({:>5.1}% of ref)",
                gap_blocks, gap_blocks * 512 * 1000 / 44100,
                rms, 100.0 * rms / rms_ref, peak, 100.0 * peak / peak_ref
            );
        }
    }

    #[test]
    fn fast_retrigger_is_not_ducked() {
        // s2.md F5: "strong first hit, quiet if I hit it right after, strong
        // again if I wait." 8 blocks @ 512/44.1k ≈ 93 ms between hits.
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);

        let first = render_peak(&mut eng, 8, true);
        let fast_rehit = render_peak(&mut eng, 8, true);
        // Let everything decay (~1 s), then a rested hit for reference.
        render_peak(&mut eng, 86, false);
        let rested = render_peak(&mut eng, 8, true);

        assert!(first > 0.05, "first hit must be audible (peak {first})");
        assert!(
            fast_rehit >= rested * 0.8,
            "fast re-hit peak {fast_rehit} vs rested {rested} — engine ducks on retrigger (BUG-023)"
        );
    }

    #[test]
    fn analog_kick_produces_audio_on_note_on() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[make_note_on(36)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "kick should produce audio");
    }

    #[test]
    fn analog_kick_is_silent_before_any_note_on() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[]);
        assert!(out.iter().all(|&s| s == 0.0), "no NoteOn → silence");
    }

    #[test]
    fn analog_kick_punch_zero_has_no_pitch_drop() {
        let mut eng_punch = AnalogEngine::kick();
        eng_punch.activate(44100.0, 512);
        let cmds = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("punch") as i64, arg1: 1.0 }];
        run_engine_cmds(&mut eng_punch, &[], &cmds);
        let out_punch = run_engine(&mut eng_punch, &[make_note_on(36)]);

        let mut eng_flat = AnalogEngine::kick();
        eng_flat.activate(44100.0, 512);
        let cmds_flat = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("punch") as i64, arg1: 0.0 }];
        run_engine_cmds(&mut eng_flat, &[], &cmds_flat);
        let out_flat = run_engine(&mut eng_flat, &[make_note_on(36)]);

        // Both produce audio; waveforms differ due to pitch sweep.
        let differ = out_punch.iter().zip(&out_flat).any(|(a, b)| (a - b).abs() > 1e-5);
        assert!(differ, "punch=1 vs punch=0 should produce different waveforms");
    }

    #[test]
    fn analog_snare_body_and_noise_both_present() {
        let mut eng = AnalogEngine::snare();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[make_note_on(48)]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "snare should produce audio");
    }

    #[test]
    fn analog_snare_noise_zero_silences_noise_path() {
        let mut eng = AnalogEngine::snare();
        eng.activate(44100.0, 512);
        let cmds = [
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("noise") as i64, arg1: 0.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("snap")  as i64, arg1: 0.3 },
        ];
        run_engine_cmds(&mut eng, &[], &cmds);
        // With noise=0, only body oscillator remains (low amplitude after snap decay).
        // Just verify it doesn't crash and produces some output.
        let out = run_engine(&mut eng, &[make_note_on(48)]);
        // Some audio expected from body oscillator.
        assert!(out.iter().any(|&s| s.abs() > 0.0), "snare with noise=0 should still have body");
    }

    /// MM-C7 is a pure restructure: `render_span` now calls `process_*` once
    /// per 64-sample chunk instead of once per span. This asserts that
    /// directly — one un-chunked call against the chunked sequence, from
    /// identical state — rather than inferring it from the ADR-035 baselines.
    /// The baselines prove it through the whole graph; this proves it per
    /// machine, and says *which* machine broke when one does.
    ///
    /// Chosen span is 500, not 512: it is not a multiple of 64, so the last
    /// chunk is a short one (7 x 64 + 52). A span that divided evenly would
    /// not exercise the `.min(end)` clamp at all.
    ///
    /// **This test is expected to fail at MM-C9, and that is not a
    /// regression.** Once an LFO ticks per sub-block, a chunked render
    /// legitimately differs from an un-chunked one — that is the entire point
    /// of the structure. Update it then, deliberately; do not weaken it now.
    #[test]
    fn chunked_render_is_identical_to_one_unchunked_call() {
        for m in AnalogMachine::ALL {
            let mut whole = AnalogEngine::new(m);
            let mut chunked = AnalogEngine::new(m);
            whole.activate(44100.0, 512);
            chunked.activate(44100.0, 512);
            whole.retrigger(60, 1.0);
            chunked.retrigger(60, 1.0);

            const END: usize = 500;
            match m {
                AnalogMachine::Kick => whole.process_kick(0, END),
                AnalogMachine::Snare => whole.process_snare(0, END),
                AnalogMachine::HiHat => whole.process_hihat(0, END),
            }
            for (lo, hi) in sub_blocks(0, END) {
                match m {
                    AnalogMachine::Kick => chunked.process_kick(lo, hi),
                    AnalogMachine::Snare => chunked.process_snare(lo, hi),
                    AnalogMachine::HiHat => chunked.process_hihat(lo, hi),
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

    /// The span really is cut, so the test above is comparing two different
    /// call sequences rather than two identical ones.
    #[test]
    fn a_five_hundred_sample_span_is_eight_chunks() {
        assert_eq!(sub_blocks(0, 500).count(), 8);
    }

    // ── MM-C9: the LFO, hosted ───────────────────────────────────────────

    use crate::engine_dsp::LFO_SUB_BLOCK;

    fn set_lfo(eng: &mut AnalogEngine, pairs: &[(&str, f64)]) {
        for (name, v) in pairs {
            eng.bank.set(ap(name), *v);
        }
    }

    // ── #175 (BUG-066): `tune` sweeps within a note, not per note ─────────

    /// The reported symptom: `lfo_dest = tune` gave *"high high, low low"* in
    /// `Free` and *"four on four identical kicks"* in `Trig`. Both were the
    /// same mechanism — `tune` had exactly one runtime read, in `retrigger()`,
    /// so the LFO was sampled at the trigger instant and held for the note.
    ///
    /// The discriminating property is **within-note** movement: a
    /// sample-and-hold and a sweep both differ from a depth-0 control, and an
    /// earlier attempt to tell them apart by rendering at two `lfo_speed`
    /// values was not valid evidence (both differ under either hypothesis).
    #[test]
    fn the_lfo_sweeps_pitch_within_one_note() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        set_lfo(
            &mut eng,
            &[
                ("lfo_dest", 1.0),  // tune
                ("lfo_depth", 1.0),
                ("lfo_speed", 4.0),
                ("lfo_shape", 0.0), // Tri: sweeps the full -1..+1
                ("lfo_mode", 1.0),  // Trig: phase reset from the note
            ],
        );
        eng.retrigger(60, 1.0);

        // Sample the swept pitch across the note, driving the LFO the way the
        // render does. `render_span` ticks it once per sub-block.
        let mut seen: Vec<f32> = Vec::new();
        for _ in 0..8 {
            seen.push(eng.swept_hz());
            eng.render_span(0, 512);
        }

        let lo = seen.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = seen.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            hi > lo * 1.05,
            "pitch must move within the note, not be frozen at retrigger: \
             saw {seen:?}"
        );
    }

    /// The half of the old behaviour that was correct and must stay: a p-lock
    /// on `tune` sets *this note's* pitch. The sweep rides on top of it rather
    /// than replacing it, so the locked note is transposed and still sweeps.
    #[test]
    fn a_tune_lock_still_sets_the_notes_pitch_under_a_sweep() {
        let pitch = |lock: Option<f64>, dest: f64| -> f32 {
            let mut eng = AnalogEngine::kick();
            eng.activate(44100.0, 512);
            eng.set_node_id(20);
            set_lfo(
                &mut eng,
                &[
                    ("lfo_dest", dest),
                    ("lfo_depth", 1.0),
                    ("lfo_speed", 4.0),
                    ("lfo_shape", 0.0),
                    ("lfo_mode", 1.0),
                ],
            );
            let mut events = Vec::new();
            if let Some(v) = lock {
                events.push(TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
                    node_id: 20, param_id: ap("tune"), value: v,
                })));
            }
            events.push(make_note_on(60));
            run_engine(&mut eng, &events);
            eng.current_hz
        };

        // With the LFO off, a +12 lock is an octave up.
        let plain = pitch(None, 0.0);
        let locked = pitch(Some(12.0), 0.0);
        assert!((locked / plain - 2.0).abs() < 0.01,
            "a `tune` lock of +12 must be an octave: {plain} -> {locked}");

        // With the LFO on `tune`, the latched base is still the locked value —
        // `retrigger` reads `raw_param`, so the sweep cannot be baked in.
        let locked_swept = pitch(Some(12.0), 1.0);
        assert_eq!(locked_swept, locked,
            "the latched pitch must be the lock alone; the sweep is applied \
             per render span on top of it");
    }

    /// A `tune` sweep must not disturb a note whose LFO points elsewhere, and
    /// must be exactly inert when the LFO is off — `swept_hz` short-circuits
    /// on a zero delta, and that has to be the same value, not merely close.
    #[test]
    fn swept_hz_is_exactly_current_hz_when_tune_is_not_the_dest() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        for dest in [0.0, 2.0, 3.0, 5.0] {
            set_lfo(
                &mut eng,
                &[("lfo_dest", dest), ("lfo_depth", 1.0), ("lfo_speed", 4.0)],
            );
            eng.retrigger(60, 1.0);
            for _ in 0..4 {
                assert_eq!(eng.swept_hz(), eng.current_hz,
                    "dest {dest} must leave pitch alone");
                eng.render_span(0, 512);
            }
        }
    }

    /// The dest table is the stability guarantee behind storing an *index*
    /// rather than a name-hash id, so a reorder has to fail loudly (MM §0 D3).
    #[test]
    fn the_lfo_dest_table_is_append_only() {
        assert_eq!(
            AnalogEngine::LFO_DESTS,
            &[
                ap("tune"), ap("tone"), ap("decay"), ap("punch"),
                ap("drive"), ap("snap"), ap("noise"), ap("open"),
            ],
            "APPEND ONLY — `lfo_dest` stores a one-based index into this, so \
             reordering silently re-points every saved patch"
        );
    }

    /// No self-modulation (ADR-042 review m14) and no modulating identity
    /// (ADR-041 §0 A4 — that would be per-step machine switching by the back
    /// door).
    /// The id table and the name table are two lists that must stay in step —
    /// there is no stable way to derive one from the other in a `const`
    /// initialiser, so this is what stops a drift from silently mislabelling
    /// an encoder.
    #[test]
    fn the_dest_ids_and_names_correspond() {
        // #179: and every per-machine table must draw from this union, since
        // `LFO_DESTS.len()` is the width the bank persists `lfo_dest` at. A
        // machine list longer than the union would truncate on load.
        for names in [KICK_DEST_NAMES, SNARE_DEST_NAMES, HIHAT_DEST_NAMES] {
            assert!(
                names.len() <= AnalogEngine::LFO_DESTS.len(),
                "a machine cannot offer more dests than the bank can store"
            );
            for n in names {
                assert!(
                    ANALOG_DEST_NAMES.contains(n),
                    "{n} is offered by a machine but absent from the union envelope"
                );
            }
        }
        assert_eq!(ANALOG_DEST_NAMES.len(), AnalogEngine::LFO_DESTS.len());
        for (name, id) in ANALOG_DEST_NAMES.iter().zip(AnalogEngine::LFO_DESTS) {
            assert_eq!(ap(name), *id, "`{name}` does not hash to its table entry");
        }
    }

    /// MM §0 D4: the cap-doc carries the `lfo_dest` labels, so a surface can
    /// name the destinations without knowing anything about LFOs. Static, not
    /// dynamic — ADR-042 amendment 5 ruled out `Dynamic` because it panics on
    /// clone, and the cap-doc path clones.
    #[test]
    fn lfo_dest_labels_reach_the_cap_doc_and_survive_a_clone() {
        // #179: the labels are the ACTIVE machine's, so each machine names
        // exactly its own destinations. The descriptor's `max` stays the union
        // width — the bank must never truncate a value belonging to a machine
        // with a longer list — so indices between a machine's count and the
        // union width are gaps, and a gap must read as EMPTY rather than
        // "off": five decoy "off" entries on a HiHat is the drawing bug the
        // labels exist to prevent.
        for (eng, names) in [
            (AnalogEngine::kick(), KICK_DEST_NAMES),
            (AnalogEngine::snare(), SNARE_DEST_NAMES),
            (AnalogEngine::hihat(), HIHAT_DEST_NAMES),
        ] {
            let doc = eng.capability_document();
            let d = doc
                .params
                .iter()
                .find(|p| p.id == ap("lfo_dest"))
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
            assert_eq!(display.format(999.0), "");
            assert_eq!(display.parse("off"), Some(0.0));
            assert_eq!(display.parse(names[0]), Some(1.0));

            // And the value-indexed view a client actually reads: named
            // entries up to the machine's count, `None` past it.
            let labels = d.value_labels().expect("a stepped param with a display");
            assert_eq!(labels.len(), AnalogEngine::LFO_DESTS.len() + 1);
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

    /// MM-C11: the first real `LfoShape` in the tree — the hint has existed
    /// since ADR-032 and nothing ever emitted it (§2.6.5's known gap).
    #[test]
    fn lfo_shape_declares_the_lfo_shape_affordance() {
        let rule = AnalogEngine::kick().to_rule(0, &[]);
        assert!(
            rule.affordances.iter().any(|(pid, hint)| *pid == ap("lfo_shape")
                && matches!(hint, AffordanceHint::LfoShape)),
            "lfo_shape must carry the LfoShape affordance"
        );
    }

    #[test]
    fn the_dest_table_excludes_lfo_params_and_machine() {
        for id in AnalogEngine::LFO_DESTS {
            assert!(!LFO_PAGE_ORDER.contains(id), "an lfo_* param is a dest");
            assert_ne!(*id, ap("machine"), "`machine` is a dest");
        }
    }

    /// `lfo_dest = 0` is off, and so is an index past the end — a malformed
    /// value must not quietly modulate a neighbour.
    #[test]
    fn dest_zero_and_out_of_range_are_both_off() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        assert_eq!(eng.lfo_dest_id(), None, "default is off");
        set_lfo(&mut eng, &[("lfo_dest", 1.0)]);
        assert_eq!(eng.lfo_dest_id(), Some(ap("tune")), "one-based: 1 is the first entry");
        set_lfo(&mut eng, &[("lfo_dest", KICK_DEST_NAMES.len() as f64)]);
        assert_eq!(eng.lfo_dest_id(), Some(ap("tone")), "and 5 is Kick's last");
        // #179: past THIS machine's table but inside the bank's union width.
        // Reachable by switching from a machine with a longer list, and it
        // must read as off rather than pointing at a param this machine does
        // not have.
        set_lfo(&mut eng, &[("lfo_dest", KICK_DEST_NAMES.len() as f64 + 1.0)]);
        assert_eq!(eng.lfo_dest_id(), None, "past Kick's table is off");
        // The bank clamps writes to the descriptor's 0..N, so a *bank* value
        // can never be out of range — defence in depth, and worth knowing.
        set_lfo(&mut eng, &[("lfo_dest", 99.0)]);
        assert_eq!(
            eng.lfo_dest_id(),
            None,
            "a bank write past the end clamps to the UNION width, which is \
             still past this machine's table — so, off"
        );
        // A p-lock bypasses the bank entirely (that is the point of
        // `node_locks`), so this is the path that can actually carry a bad
        // index — along with a malformed project. It must read as off rather
        // than modulating a neighbour.
        eng.node_locks.push((ap("lfo_dest"), 99.0));
        assert_eq!(eng.lfo_dest_id(), None, "an out-of-range lock is off, not clamped");
        eng.node_locks.clear();
        eng.node_locks.push((ap("lfo_dest"), f64::NAN));
        assert_eq!(eng.lfo_dest_id(), None, "and so is a non-finite one");
    }

    /// Depth 0 must be *exactly* the unmodulated read — this is what makes
    /// the four ADR-035 baselines still valid after MM-C9.
    #[test]
    fn depth_zero_is_bit_identical_to_no_lfo() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        set_lfo(&mut eng, &[("lfo_dest", 5.0), ("lfo_depth", 0.0), ("lfo_speed", 8.0)]); // #179: `tone` is 5 on Kick
        let base = eng.raw_param(ap("tone"));
        for _ in 0..40 {
            eng.update_lfo(LFO_SUB_BLOCK);
            assert_eq!(eng.get_param(ap("tone")), base);
        }
    }

    /// The LFO rides on `get_param`, so it moves the value the machine reads
    /// while leaving the bank alone (ADR-042 decision 3).
    #[test]
    fn the_lfo_moves_the_read_value_but_never_the_bank() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        // tone: 200..8000 on Kick's overlay, union 200..18000.
        set_lfo(
            &mut eng,
            // #179: `tone` is index 5 on Kick's own dest table.
            &[("lfo_dest", 5.0), ("lfo_depth", 1.0), ("lfo_speed", 4.0),
              ("lfo_shape", 1.0), ("lfo_mode", 1.0), ("tone", 4000.0)],
        );
        eng.retrigger(60, 1.0);

        let bank_before = eng.bank.get(ap("tone"));
        let mut seen: Vec<f32> = Vec::new();
        for _ in 0..200 {
            eng.update_lfo(LFO_SUB_BLOCK);
            seen.push(eng.get_param(ap("tone")));
        }
        assert!(
            seen.iter().any(|v| (*v - 4000.0).abs() > 1.0),
            "the LFO must actually move the read value"
        );
        assert_eq!(
            eng.bank.get(ap("tone")),
            bank_before,
            "and must never write the bank — p-locks, the state bus and kits \
             all read the base"
        );
        let (lo, hi) = eng.bank.range(ap("tone")).unwrap();
        for v in &seen {
            assert!(
                *v as f64 >= lo - 1e-3 && *v as f64 <= hi + 1e-3,
                "modulated value {v} left the declared range {lo}..{hi}"
            );
        }
    }

    /// Only the **destination** is modulated. Nothing else on the node moves,
    /// and in particular nothing else gets the destination's clamp — an LFO on
    /// `tone` (200..8000) must not squeeze `decay` (0.01..2.0) into that range.
    ///
    /// A mutant dropping the `param_id == dest` check survived the whole suite
    /// until this existed: every other test reads the destination, so applying
    /// the offset everywhere looked identical.
    #[test]
    fn only_the_destination_param_is_modulated() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        set_lfo(
            &mut eng,
            // #179: `tone` is index 5 on Kick's own dest table.
            &[("lfo_dest", 5.0), ("lfo_depth", 1.0), ("lfo_speed", 4.0),
              ("lfo_shape", 2.0), ("lfo_mode", 1.0), ("tone", 4000.0),
              ("decay", 0.5), ("drive", 0.25)],
        );
        eng.retrigger(60, 1.0);

        let mut tone_moved = false;
        for _ in 0..200 {
            eng.update_lfo(LFO_SUB_BLOCK);
            if (eng.get_param(ap("tone")) - 4000.0).abs() > 1.0 {
                tone_moved = true;
            }
            assert_eq!(
                eng.get_param(ap("decay")),
                0.5,
                "`decay` is not the destination and must not move — nor be \
                 clamped into `tone`'s 200..8000 range"
            );
            assert_eq!(eng.get_param(ap("drive")), 0.25, "nor `drive`");
        }
        assert!(tone_moved, "the destination really was being modulated");
    }

    /// #179: **no dest index is inert on any machine.**
    ///
    /// The inverse of what this test used to assert. Through MM the dest table
    /// was machine-invariant (one `LFO_DESTS` per node) while `machine_params`
    /// was per-machine, so on a Kick dests 6/7/8 pointed at params
    /// `process_kick` never reads, on a Snare 4/5/8, and on a HiHat five of
    /// eight did nothing. That earlier test pinned the matrix as a record of
    /// the defect, not a specification; this replaces it with the property
    /// that made it a defect.
    ///
    /// Both directions, because either alone is satisfiable by a mistake: a
    /// dest naming a param the machine does not read is the original bug, and
    /// a param the machine reads with no dest is a destination silently
    /// dropped from the offer.
    #[test]
    fn every_machines_dests_are_exactly_its_params() {
        for machine in AnalogMachine::ALL {
            let mut read: Vec<String> = AnalogEngine::machine_params(machine)
                .iter()
                .map(|p| p.name.to_string())
                .collect();
            let mut offered: Vec<String> = AnalogEngine::dest_names(machine)
                .iter()
                .map(|n| n.to_string())
                .collect();
            read.sort();
            offered.sort();
            assert_eq!(
                offered, read,
                "{machine:?}: every destination offered must be a param this \
                 machine reads, and every param it reads must be offered"
            );
        }
    }

    /// `lfo_dest` persists a one-based index into the **active machine's**
    /// table, so each table is append-only for exactly the reason
    /// `AnalogMachine::ALL` is: inserting or reordering an entry re-points
    /// every saved patch on that machine at a different param (#179, and the
    /// same contract MM §0 D3 gave the union table).
    #[test]
    fn the_per_machine_dest_tables_are_append_only() {
        assert_eq!(KICK_DEST_NAMES, &["tune", "punch", "decay", "drive", "tone"]);
        assert_eq!(SNARE_DEST_NAMES, &["tune", "snap", "noise", "decay", "tone"]);
        assert_eq!(HIHAT_DEST_NAMES, &["tone", "decay", "open"]);
    }

    /// The bank's `lfo_dest` range stays the **union** width while the
    /// encoder's overlay narrows to the machine (#179).
    ///
    /// This is the pair that makes per-machine tables safe to persist. Narrow
    /// the bank instead and a Kick patch's `lfo_dest = 5` truncates to 3 the
    /// moment it is loaded into a node sitting on HiHat — on load, silently,
    /// and then saved back that way. It is the same trap `union_params`'
    /// header describes for every other param.
    #[test]
    fn the_dest_range_is_union_wide_in_the_bank_and_per_machine_on_the_encoder() {
        for machine in AnalogMachine::ALL {
            let doc = AnalogEngine::new(machine).capability_document();
            let d = doc
                .params
                .iter()
                .find(|p| p.id == ap("lfo_dest"))
                .expect("lfo_dest is declared");
            assert_eq!(
                d.max,
                AnalogEngine::LFO_DESTS.len() as f64,
                "{machine:?}: the bank must hold any machine's index"
            );

            let overlay = AnalogEngine::machine_overlays(machine)
                .into_iter()
                .find(|(id, _)| *id == ap("lfo_dest"))
                .map(|(_, o)| o)
                .expect("the overlay narrows the encoder");
            assert_eq!(
                overlay.max,
                AnalogEngine::dest_names(machine).len() as f64,
                "{machine:?}: the encoder must reach only its own dests"
            );
            assert!(!overlay.identity, "`lfo_dest` is a setting, not identity");
        }
    }

    /// A value belonging to a machine with a longer list survives a switch
    /// away and back (#179).
    ///
    /// Translating `lfo_dest` by param name on a switch was tried and is
    /// worse: a destination the other machine lacks has nowhere to go, so it
    /// collapses to off and the round trip loses it — breaking MM §6.2, which
    /// `machine_round_trip_preserves_every_param` pins. Leaving the number
    /// alone means the LFO goes quiet while the other machine is selected and
    /// comes back exactly as it was.
    #[test]
    fn a_dest_that_the_other_machine_lacks_survives_a_round_trip() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        // 2 is `punch` on Kick; HiHat has no `punch` at any index.
        set_lfo(&mut eng, &[("lfo_dest", 2.0)]);
        assert_eq!(eng.lfo_dest_id(), Some(ap("punch")));

        eng.apply_machine_switch(AnalogMachine::HiHat);
        assert_eq!(eng.bank.get(ap("lfo_dest")), 2.0, "the stored index is untouched");
        assert_eq!(
            eng.lfo_dest_id(),
            Some(ap("decay")),
            "on HiHat index 2 is `decay` — the number keeps its value and its \
             meaning follows the machine"
        );

        eng.apply_machine_switch(AnalogMachine::Kick);
        assert_eq!(eng.lfo_dest_id(), Some(ap("punch")), "and comes back intact");
    }

    /// The other half: an index past the new machine's table is off, not a
    /// neighbour. Kick offers 5 destinations, HiHat 3.
    #[test]
    fn a_dest_past_the_new_machines_table_reads_as_off() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        set_lfo(&mut eng, &[("lfo_dest", 5.0)]);
        assert_eq!(eng.lfo_dest_id(), Some(ap("tone")), "5 is `tone` on Kick");

        eng.apply_machine_switch(AnalogMachine::HiHat);
        assert_eq!(
            eng.lfo_dest_id(),
            None,
            "HiHat has only 3 dests, so 5 modulates nothing rather than \
             landing on whatever sits nearby"
        );

        eng.apply_machine_switch(AnalogMachine::Kick);
        assert_eq!(eng.lfo_dest_id(), Some(ap("tone")));
    }

    /// Session #5 audit: walk **every** one-based `lfo_dest` index and record
    /// which of the node's params actually move. Exactly one must, and it
    /// must be the one `ANALOG_DEST_NAMES` claims sits at that index.
    ///
    /// `only_the_destination_param_is_modulated` above proves the isolation
    /// property, but only for dest 2 against two bystanders — an index that
    /// selected the wrong param, or a table whose names had drifted from
    /// `LFO_DESTS`, would pass it. This is the exhaustive form: 8 indices x
    /// every observable param, so "the LFO is wired to something other than
    /// what the panel says" cannot hide.
    ///
    /// **This is the parameter-*read* layer**, not the audible one. It proves
    /// `lfo_dest = n` offsets the param the table names when read through
    /// `get_param`. It says nothing about whether the active machine ever
    /// reads that param — on a Kick this test reports dest 6 moving `snap`,
    /// while `some_dest_indices_are_inert_on_each_machine` above records that
    /// `snap` is inert on a Kick. Both are true: the union bank holds `snap`,
    /// and `process_kick` never reads it. Read the two together.
    ///
    /// Prints the full matrix on failure so the actual wiring is visible
    /// rather than just the first mismatch.
    #[test]
    fn every_dest_index_modulates_exactly_the_param_it_names() {
        // Every param a caller can observe on this node, derived from the bank
        // itself rather than listed by hand — a hand-written list silently
        // stops being exhaustive the moment a param is added, which is exactly
        // the failure this test exists to prevent. Includes `machine` and the
        // seven `lfo_*` controls, so an LFO that reached its own controls
        // would be caught.
        //
        // #179: per machine now. The dest table used to be machine-invariant,
        // so this walked one table against one machine and said nothing about
        // the other two — where three to five of the eight entries modulated
        // a param that machine does not read.
        let observed = |m: AnalogMachine| -> Vec<u32> {
            AnalogEngine::union_params(m).iter().map(|p| p.id).collect()
        };
        let name_of = |id: u32| -> String {
            AnalogEngine::union_params(AnalogMachine::Kick)
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.to_string())
                .unwrap_or_else(|| format!("id:{id}"))
        };

        let mut failures: Vec<String> = Vec::new();

        for machine in AnalogMachine::ALL {
            let observed = observed(machine);
            let names = AnalogEngine::dest_names(machine);
            assert!(
                names.iter().all(|n| observed.contains(&ap(n))),
                "sanity: every {machine:?} dest must be observable in the bank"
            );

            for (i, expected_name) in names.iter().enumerate() {
                let idx = (i + 1) as f64;
                let mut eng = AnalogEngine::new(machine);
                eng.activate(44100.0, 512);
                set_lfo(
                    &mut eng,
                    &[
                        ("lfo_dest", idx),
                        ("lfo_depth", 1.0),
                        ("lfo_speed", 4.0),
                        ("lfo_shape", 0.0), // Tri: sweeps the full -1..+1
                        ("lfo_mode", 1.0),  // Trig: deterministic phase
                    ],
                );
                eng.retrigger(60, 1.0);

                // Unmodulated baselines, read through `raw_param` so the LFO
                // is excluded by construction rather than by timing.
                let base: Vec<f32> = observed.iter().map(|id| eng.raw_param(*id)).collect();
                let mut moved = vec![false; observed.len()];

                // 200 x 64 samples / 44100 = 0.29 s; at 4 Hz that is 1.16
                // cycles, so a Tri covers its whole excursion with margin.
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

    /// ADR-042 amendment 1: the base is the `get_param()` result, so a
    /// p-locked step and the LFO **compose** — the lock is not defeated by the
    /// LFO, and the LFO does not start from the bank while a lock is in force.
    #[test]
    fn an_lfo_breathes_on_top_of_a_p_locked_step() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        set_lfo(
            &mut eng,
            // #179: `tone` is index 5 on Kick's own dest table.
            &[("lfo_dest", 5.0), ("lfo_depth", 0.5), ("lfo_speed", 4.0),
              ("lfo_shape", 2.0), ("lfo_mode", 1.0), ("tone", 4000.0)],
        );
        // Square wave, so the offset is a constant +/- within a half cycle and
        // the arithmetic is checkable rather than approximate.
        eng.retrigger(60, 1.0);
        eng.update_lfo(LFO_SUB_BLOCK);
        let unlocked = eng.get_param(ap("tone"));
        let offset = unlocked - 4000.0;
        assert!(offset.abs() > 1.0, "the LFO is doing something");

        // Now the same instant, with a p-lock in force on the same param.
        eng.node_locks.push((ap("tone"), 1000.0));
        let locked = eng.get_param(ap("tone"));
        assert!(
            (locked - (1000.0 + offset)).abs() < 1e-3,
            "expected lock + offset ({}), got {locked} — a lock-only result \
             would be 1000 and an offset-from-bank result {unlocked}",
            1000.0 + offset
        );
    }

    /// The seven params are on the MOD page, in one block, identically on
    /// every machine — MM-C11 draws it, but the placement has to exist now or
    /// MM-C8b's assertion refuses the declaration.
    #[test]
    fn every_machine_carries_the_same_mod_page() {
        let mut first: Option<Vec<(u32, u8)>> = None;
        for m in AnalogMachine::ALL {
            let mod_page: Vec<(u32, u8)> = AnalogEngine::machine_page_refs(m)
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
            match &first {
                None => first = Some(mod_page),
                Some(f) => assert_eq!(*f, mod_page, "{m:?}'s MOD page differs"),
            }
        }
    }

    #[test]
    fn analog_hihat_open_extends_decay() {
        let mut eng_closed = AnalogEngine::hihat();
        eng_closed.activate(44100.0, 512);
        let cmds_c = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("open") as i64, arg1: 0.0 }];
        run_engine_cmds(&mut eng_closed, &[], &cmds_c);
        let _t0 = run_engine(&mut eng_closed, &[make_note_on(60)]);
        // Let it decay for 5 blocks
        for _ in 0..5 { run_engine(&mut eng_closed, &[]); }
        let tail_closed: f32 = run_engine(&mut eng_closed, &[]).iter().map(|&x| x.abs()).sum();

        let mut eng_open = AnalogEngine::hihat();
        eng_open.activate(44100.0, 512);
        let cmds_o = [NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("open") as i64, arg1: 1.0 }];
        run_engine_cmds(&mut eng_open, &[], &cmds_o);
        let _t0o = run_engine(&mut eng_open, &[make_note_on(60)]);
        for _ in 0..5 { run_engine(&mut eng_open, &[]); }
        let tail_open: f32 = run_engine(&mut eng_open, &[]).iter().map(|&x| x.abs()).sum();

        assert!(tail_open > tail_closed,
            "open hihat should have longer decay tail: open={tail_open:.4}, closed={tail_closed:.4}");
    }

    #[test]
    fn analog_hihat_closed_short_decay() {
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        let _ = run_engine(&mut eng, &[make_note_on(60)]);
        // After many blocks, closed hihat should be silent.
        for _ in 0..40 { run_engine(&mut eng, &[]); }
        let tail: f32 = run_engine(&mut eng, &[]).iter().map(|&x| x.abs()).sum();
        assert!(tail < 1e-4, "closed hihat should be silent after decay, got {tail:.6}");
    }

    #[test]
    fn analog_bump_param_decay_changes_output_length() {
        // Shorter decay → sample exhausts sooner → fewer non-zero frames after N blocks.
        let mut eng_long = AnalogEngine::kick();
        eng_long.activate(44100.0, 512);
        let _ = run_engine(&mut eng_long, &[make_note_on(36)]);
        for _ in 0..4 { run_engine(&mut eng_long, &[]); }
        let long_energy: f32 = run_engine(&mut eng_long, &[]).iter().map(|&x| x * x).sum();

        let mut eng_short = AnalogEngine::kick();
        eng_short.activate(44100.0, 512);
        let bump = NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: ap("decay") as i64, arg1: 0.02 };
        run_engine_cmds(&mut eng_short, &[], &[bump]);
        let _ = run_engine(&mut eng_short, &[make_note_on(36)]);
        for _ in 0..4 { run_engine(&mut eng_short, &[]); }
        let short_energy: f32 = run_engine(&mut eng_short, &[]).iter().map(|&x| x * x).sum();

        assert!(long_energy > short_energy,
            "long decay should have more energy after same time: {long_energy:.6} vs {short_energy:.6}");
    }

    #[test]
    fn analog_param_lock_drive_overrides_base_drive() {
        let node_id = 42u32;
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);
        let _ = run_engine(&mut eng, &[make_note_on(36)]);

        // With no param lock: drive=0 (default)
        let out_no_lock = run_engine(&mut eng, &[make_note_on(36)]);

        // With param lock: drive=1.0 (max)
        let lock_event = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("drive"), value: 1.0,
        }));
        let out_lock = run_engine(&mut eng, &[lock_event, make_note_on(36)]);

        let rms = |v: &[f32]| (v.iter().map(|&x| x*x).sum::<f32>() / v.len() as f32).sqrt();
        // Drive=1.0 should produce different (typically louder/more saturated) output.
        let differ = (rms(&out_lock) - rms(&out_no_lock)).abs() > 1e-5;
        assert!(differ, "param lock drive=1.0 should change output vs drive=0");
    }

    #[test]
    fn analog_param_lock_does_not_bleed_to_next_cycle() {
        let node_id = 42u32;
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        // Cycle 1: param lock drive=1.0 — must not permanently mutate the bank.
        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("drive"), value: 1.0,
        }));
        let _ = run_engine(&mut eng, &[lock, make_note_on(36)]);

        // Bank drive should still be 0.0 (the default) — the lock goes to node_locks,
        // not to the bank, so the base value is unchanged for subsequent cycles.
        assert!((eng.bank.get(ap("drive")) - 0.0).abs() < 1e-9,
            "bank drive should stay 0.0 after a locked cycle; got {:.4}", eng.bank.get(ap("drive")));

        // Cycle 2: no lock — drive=0.0 (base), output should differ from locked drive=1.0.
        let out_base = run_engine(&mut eng, &[make_note_on(36)]);
        let out_locked = {
            let mut e2 = AnalogEngine::kick();
            e2.activate(44100.0, 512);
            e2.set_node_id(node_id);
            let lock2 = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
                node_id, param_id: ap("drive"), value: 1.0,
            }));
            run_engine(&mut e2, &[lock2, make_note_on(36)])
        };
        let rms = |v: &[f32]| (v.iter().map(|&x| x*x).sum::<f32>() / v.len() as f32).sqrt();
        assert!((rms(&out_base) - rms(&out_locked)).abs() > 1e-4,
            "cycle 2 (no lock) should differ from locked drive=1.0; base={:.4} locked={:.4}",
            rms(&out_base), rms(&out_locked));
    }

    // ── #169 (BUG-063): a p-lock owns its note, not one audio cycle ────────
    //
    // Every test below fails against the pre-#169 `node_locks.clear()` at the
    // top of `process()`. The two that assert on audio are the ones that
    // matter — `analog_param_lock_changes_output` above already passed on the
    // broken code, because it only ever looked at the block the trigger
    // arrived in, which is exactly the one block the old behaviour got right.

    /// The reported symptom: an `open` lock on a HiHat step "sounds identical".
    ///
    /// `open` is re-read per render span (it feeds `effective_decay`), so
    /// under the per-cycle clear it reverted to the bank ~11 ms in and the
    /// locked hit was the closed hat's decay shape at a slightly higher peak.
    /// Blocks 2.. are therefore where the bug lives; block 1 never showed it.
    #[test]
    fn a_p_lock_shapes_the_whole_note_not_just_its_first_block() {
        let node_id = 22u32;
        let rms = |v: &[f32]| (v.iter().map(|&x| x * x).sum::<f32>() / v.len() as f32).sqrt();

        // Tail energy after the trigger block, with and without an `open` lock.
        let tail = |lock: bool| -> f32 {
            let mut eng = AnalogEngine::hihat();
            eng.activate(44100.0, 512);
            eng.set_node_id(node_id);
            let mut events = Vec::new();
            if lock {
                events.push(TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
                    node_id, param_id: ap("open"), value: 1.0,
                })));
            }
            events.push(make_note_on(42));
            run_engine(&mut eng, &events);
            // Blocks 2..=6 — roughly 12..70 ms past the onset.
            let mut tail = Vec::new();
            for _ in 0..5 {
                tail.extend(run_engine(&mut eng, &[]));
            }
            rms(&tail)
        };

        let open = tail(true);
        let closed = tail(false);
        assert!(
            open > closed * 3.0,
            "an `open`=1.0 lock must still be holding the envelope open after \
             the trigger block: locked tail rms={open:.5}, unlocked={closed:.5}"
        );
    }

    /// The locked value must persist without a fresh `ParamLock` each cycle.
    /// State-level twin of the audio test above — it names the mechanism, so a
    /// future refactor that reintroduces per-cycle clearing fails here first.
    #[test]
    fn a_lock_survives_blocks_that_carry_no_events() {
        let node_id = 22u32;
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("open"), value: 1.0,
        }));
        run_engine(&mut eng, &[lock, make_note_on(42)]);

        for block in 2..=8 {
            run_engine(&mut eng, &[]);
            assert!(
                (eng.raw_param(ap("open")) - 1.0).abs() < 1e-6,
                "block {block}: the lock must outlive the cycle it arrived in, \
                 got open={:.4}",
                eng.raw_param(ap("open"))
            );
        }
        assert_eq!(eng.bank.get(ap("open")), 0.0, "and never touch the bank");
    }

    /// The other half of the contract, and the one the per-cycle clear was
    /// written to protect: the *next* trigger bounds a lock. A step carrying
    /// no lock must sound exactly like the never-locked engine.
    #[test]
    fn an_unlocked_trigger_ends_the_previous_step_s_lock() {
        let node_id = 22u32;
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("open"), value: 1.0,
        }));
        run_engine(&mut eng, &[lock, make_note_on(42)]);
        assert!(!eng.node_locks.is_empty(), "locked step holds its lock");

        // Next step, no lock.
        let rms = |v: &[f32]| (v.iter().map(|&x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let mut after = run_engine(&mut eng, &[make_note_on(42)]);
        assert!(eng.node_locks.is_empty(),
            "an unlocked trigger must retire the previous step's set");
        assert_eq!(eng.raw_param(ap("open")), eng.bank.get(ap("open")) as f32,
            "and `open` must read the bank again");
        for _ in 0..5 { after.extend(run_engine(&mut eng, &[])); }

        // A never-locked engine driven the same way.
        let mut clean = AnalogEngine::hihat();
        clean.activate(44100.0, 512);
        clean.set_node_id(node_id);
        run_engine(&mut clean, &[make_note_on(42)]);
        let mut clean_after = run_engine(&mut clean, &[make_note_on(42)]);
        for _ in 0..5 { clean_after.extend(run_engine(&mut clean, &[])); }

        // Not sample-exact: the SVF carries state across the retrigger, so the
        // first few samples still remember the louder open hit. The envelope —
        // the thing the lock was shaping — must be back to closed.
        let (got, want) = (rms(&after), rms(&clean_after));
        assert!((got - want).abs() < want * 0.02,
            "the unlocked step must decay like the never-locked engine: \
             got rms={got:.5}, want={want:.5}");
    }

    /// A second lock run replaces the first rather than appending to it —
    /// otherwise `node_locks` grows without bound across a pattern and
    /// `raw_param`'s first-match scan keeps returning the oldest value.
    #[test]
    fn a_new_lock_run_replaces_the_previous_set() {
        let node_id = 22u32;
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        for value in [0.25_f64, 0.75, 0.5] {
            let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
                node_id, param_id: ap("open"), value,
            }));
            run_engine(&mut eng, &[lock, make_note_on(42)]);
            assert_eq!(eng.node_locks.len(), 1,
                "one lock per step, not an accumulating list");
            assert!((eng.raw_param(ap("open")) as f64 - value).abs() < 1e-6,
                "the newest lock wins; got {}", eng.raw_param(ap("open")));
        }
    }

    /// Two triggers in one block, the second locked: the lock must attach to
    /// the trigger it followed, not to both. This is the case the
    /// "have I seen a lock since the last retrigger?" flag exists to get right.
    #[test]
    fn a_mid_block_lock_attaches_only_to_the_trigger_it_precedes() {
        let node_id = 22u32;
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        // A locked step first, so there is a set in flight to be retired.
        let pre = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("open"), value: 1.0,
        }));
        run_engine(&mut eng, &[pre, make_note_on(42)]);

        // Then a block holding an unlocked trigger followed by a locked one.
        let mut unlocked = make_note_on(42);
        unlocked.sample_offset = 0;
        let mut lock = TimedEvent::new(256, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("tone"), value: 0.9,
        }));
        lock.sample_offset = 256;
        let mut locked = make_note_on(42);
        locked.sample_offset = 256;
        run_engine(&mut eng, &[unlocked, lock, locked]);

        assert_eq!(eng.node_locks.len(), 1,
            "the first trigger retired `open`; the second owns only `tone`");
        assert!((eng.raw_param(ap("tone")) - 0.9).abs() < 1e-6);
        assert_eq!(eng.raw_param(ap("open")), eng.bank.get(ap("open")) as f32,
            "`open` must be back to the bank — its step is over");
    }

    /// A re-activate (dynamic topology rebuild) kills the voice, so it must
    /// kill the voice's locks too — they no longer outlive anything.
    #[test]
    fn activate_retires_a_lock_in_flight() {
        let node_id = 22u32;
        let mut eng = AnalogEngine::hihat();
        eng.activate(44100.0, 512);
        eng.set_node_id(node_id);

        let lock = TimedEvent::new(0, Event::ParamLock(ParamLockEvent {
            node_id, param_id: ap("open"), value: 1.0,
        }));
        run_engine(&mut eng, &[lock, make_note_on(42)]);
        assert!(!eng.node_locks.is_empty());

        eng.activate(44100.0, 512);
        assert!(eng.node_locks.is_empty(), "a rebuild must not carry a lock over");
        assert!(!eng.locks_pending);
    }

    // ── MM-C3: union bank + machine identity ──────────────────────────────

    /// **The phase's load-bearing invariant** (MM §3.4). The bank stores the
    /// widest envelope across every machine and is never narrowed to the
    /// active one — writes clamp to the bank's range, so narrowing truncates
    /// storage rather than display, and `deserialize()` runs after
    /// `activate()` through that same clamping `set()`.
    ///
    /// Asserted against the variants' own overlays rather than literals, so it
    /// cannot drift from the declarations it is checking.
    #[test]
    fn union_bank_covers_every_variant_overlay() {
        for constructed in AnalogMachine::ALL {
            let doc = AnalogEngine::build_doc(constructed);
            for variant in AnalogEngine::machine_variants() {
                for (pid, overlay) in variant.overlays.iter() {
                    let slot = doc
                        .params
                        .iter()
                        .find(|p| p.id == *pid)
                        .unwrap_or_else(|| {
                            panic!("variant {} declares param {pid} the union doc lacks", variant.name)
                        });
                    assert!(
                        slot.min <= overlay.min && slot.max >= overlay.max,
                        "bank range [{}, {}] for param {pid} does not cover {}'s \
                         overlay [{}, {}] (engine constructed as {constructed:?}) — \
                         narrowing the bank truncates stored values on load",
                        slot.min, slot.max, variant.name, overlay.min, overlay.max
                    );
                }
            }
        }
    }

    /// The load path that exists today: `initial_params` from the instrument
    /// file, replayed through the clamping `bank.set()` inside `activate()`.
    ///
    /// HiHat's `tone` reaches 18 kHz; Kick's stops at 8 kHz. A Kick-constructed
    /// engine must still store 18 kHz, because the value belongs to a machine
    /// the user may select later. This is the corruption path from MM §3.4.
    ///
    /// It was written against `set_initial_params` because #154 meant a
    /// project save could not exercise it at all. That is no longer true —
    /// `a_saved_value_belonging_to_an_unselected_machine_survives_the_load`
    /// now covers the same invariant through the real save/load path. This
    /// one stays: `initial_params` is a second, independent route into the
    /// same clamping `set()`, and it runs inside `activate()` rather than
    /// after it.
    #[test]
    fn a_value_legal_on_another_machine_survives_loading_under_this_one() {
        let hihat_tone_max = AnalogEngine::machine_params(AnalogMachine::HiHat)
            .into_iter()
            .find(|p| p.id == ap("tone"))
            .expect("HiHat declares tone")
            .max;
        let kick_tone_max = AnalogEngine::machine_params(AnalogMachine::Kick)
            .into_iter()
            .find(|p| p.id == ap("tone"))
            .expect("Kick declares tone")
            .max;
        assert!(
            hihat_tone_max > kick_tone_max,
            "fixture assumption: HiHat's tone range must exceed Kick's"
        );

        let mut eng = AnalogEngine::kick();
        let mut initial = HashMap::new();
        initial.insert("tone".to_string(), hihat_tone_max);
        eng.set_initial_params(&initial);
        eng.activate(44100.0, 512);

        assert_eq!(
            eng.bank.get(ap("tone")),
            hihat_tone_max,
            "a Kick-constructed engine truncated a HiHat-legal tone to Kick's \
             ceiling — the bank was narrowed to the active machine, which \
             destroys the value on load"
        );
    }

    /// Switching away and back must lose nothing, for every param of every
    /// machine. Probe values are derived from the declared ranges, not written
    /// as literals — a failing derived test cannot be quietly retuned.
    #[test]
    fn machine_round_trip_preserves_every_param() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);

        // Park every param at a distinctive point inside the union range.
        let doc = AnalogEngine::build_doc(AnalogMachine::Kick);
        let mut expected: Vec<(u32, f64)> = Vec::new();
        for p in doc.params.iter().filter(|p| p.id != ap("machine")) {
            let v = p.min + (p.max - p.min) * 0.73;
            eng.bank.set(p.id, v);
            expected.push((p.id, v));
        }

        for target in [AnalogMachine::Snare, AnalogMachine::HiHat, AnalogMachine::Kick] {
            eng.bank.set(ap("machine"), target.value() as f64);
            eng.poll_machine_param();
            // Inactive voice, so the switch is immediate — no fade to run out.
            assert_eq!(eng.machine, target, "switch should apply while silent");
        }

        for (pid, want) in expected {
            assert_eq!(
                eng.bank.get(pid),
                want,
                "param {pid} changed across a machine round trip"
            );
        }
    }

    // ── #154: project persistence ─────────────────────────────────────────

    /// `AnalogMachine::ALL` is declared append-only in prose (`ALL`'s doc
    /// comment) and nothing enforced it. #154 made `machine` a persisted
    /// value, so the numeric index now lives in project files: inserting a
    /// machine anywhere but the end boots every previously-saved project on
    /// the wrong voice, and `from_value` clamps rather than erroring, so
    /// nothing complains. Extend the array; never reorder it.
    #[test]
    fn the_machine_table_is_append_only() {
        assert_eq!(
            AnalogMachine::ALL,
            [
                AnalogMachine::Kick,
                AnalogMachine::Snare,
                AnalogMachine::HiHat
            ]
        );
        // `value()` is a separate `match`, so the two can drift apart.
        for (i, m) in AnalogMachine::ALL.iter().enumerate() {
            assert_eq!(m.value(), i as u32, "{m:?} does not sit at its own value");
            assert_eq!(AnalogMachine::from_value(i as u32), *m);
        }
    }

    /// The corruption path MM §3.4 was actually about, now that it exists.
    /// `a_value_legal_on_another_machine_survives_loading_under_this_one`
    /// had to go through `set_initial_params` because this node had no
    /// `serialize`/`deserialize` at all; #154 added them, so the real
    /// save → `activate()` → `deserialize()` sequence can be asserted.
    ///
    /// HiHat's `tone` reaches 18 kHz, Kick's stops at 8 kHz. Save a
    /// HiHat-legal tone while the engine is on Kick and it must come back
    /// intact — narrowing the bank to the active machine would truncate it
    /// on **load**, silently, and the next save would persist the truncation.
    #[test]
    fn a_saved_value_belonging_to_an_unselected_machine_survives_the_load() {
        let hihat_tone_max = AnalogEngine::machine_params(AnalogMachine::HiHat)
            .into_iter()
            .find(|p| p.id == ap("tone"))
            .expect("HiHat declares tone")
            .max;

        let mut saved = AnalogEngine::kick();
        saved.activate(44100.0, 512);
        saved.bank.set(ap("tone"), hihat_tone_max);
        let bytes = saved.serialize();
        assert!(!bytes.is_empty(), "the trait default returns an empty Vec");

        let mut loaded = AnalogEngine::kick();
        loaded.activate(44100.0, 512); // resets the bank to defaults
        loaded.deserialize(&bytes);

        assert_eq!(
            loaded.bank.get(ap("tone")),
            hihat_tone_max,
            "a HiHat-legal tone was truncated to Kick's ceiling on load"
        );
    }

    /// `machine` is a bank slot, so the selected machine is part of what a
    /// project saves. It lands on the first `process()`: `activate()` leaves
    /// the voice inactive, so `poll_machine_param` snaps with no fade.
    #[test]
    fn the_selected_machine_is_part_of_what_a_project_saves() {
        let mut saved = AnalogEngine::kick();
        saved.activate(44100.0, 512);
        saved.bank.set(ap("machine"), AnalogMachine::HiHat.value() as f64);
        let bytes = saved.serialize();

        let mut loaded = AnalogEngine::kick();
        loaded.activate(44100.0, 512);
        loaded.deserialize(&bytes);
        assert_eq!(
            loaded.bank.get(ap("machine")),
            AnalogMachine::HiHat.value() as f64
        );

        loaded.poll_machine_param();
        assert_eq!(
            loaded.machine,
            AnalogMachine::HiHat,
            "a loaded project must come up on the machine it was saved on"
        );
    }

    /// Every param, not just the two above — a hand-written serializer that
    /// forgot a slot would pass a spot check.
    #[test]
    fn every_bank_param_survives_a_save_and_load() {
        let mut saved = AnalogEngine::kick();
        saved.activate(44100.0, 512);

        // Distinctive point inside each union range, derived rather than
        // written as literals; skip `machine`, which is stepped.
        let doc = AnalogEngine::build_doc(AnalogMachine::Kick);
        let mut expected: Vec<(u32, f64)> = Vec::new();
        for p in doc.params.iter().filter(|p| p.id != ap("machine")) {
            let v = p.min + (p.max - p.min) * 0.31;
            saved.bank.set(p.id, v);
            expected.push((p.id, v));
        }

        let mut loaded = AnalogEngine::kick();
        loaded.activate(44100.0, 512);
        loaded.deserialize(&saved.serialize());

        for (pid, want) in expected {
            assert_eq!(loaded.bank.get(pid), want, "param {pid} was lost on load");
        }
    }

    #[test]
    fn union_doc_has_no_duplicate_ids() {
        for m in AnalogMachine::ALL {
            let doc = AnalogEngine::build_doc(m);
            let mut ids: Vec<u32> = doc.params.iter().map(|p| p.id).collect();
            let before = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), before, "duplicate param id in {m:?}'s union doc");
        }
    }

    /// The union is the same set whatever the engine was constructed as —
    /// only defaults differ. A machine-dependent param *set* would mean the
    /// bank changes shape on switch.
    #[test]
    fn union_param_set_is_machine_independent_but_defaults_are_not() {
        let ids = |m| {
            let mut v: Vec<u32> = AnalogEngine::build_doc(m).params.iter().map(|p| p.id).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(ids(AnalogMachine::Kick), ids(AnalogMachine::HiHat));

        let default_of = |m, pid| {
            AnalogEngine::build_doc(m)
                .params
                .iter()
                .find(|p| p.id == pid)
                .unwrap()
                .default
        };
        // Kick's tone default is 4000, HiHat's 8000 — the active machine picks.
        assert_ne!(
            default_of(AnalogMachine::Kick, ap("tone")),
            default_of(AnalogMachine::HiHat, ap("tone"))
        );
        assert_eq!(default_of(AnalogMachine::Kick, ap("machine")), 0.0);
        assert_eq!(default_of(AnalogMachine::HiHat, ap("machine")), 2.0);
    }

    #[test]
    fn machine_param_is_stepped_over_the_machine_count() {
        let doc = AnalogEngine::build_doc(AnalogMachine::Kick);
        let m = doc.params.iter().find(|p| p.id == ap("machine")).expect("machine param");
        assert!(m.stepped);
        assert_eq!((m.min, m.max), (0.0, 2.0));
    }

    /// Every variant must flag `machine` as identity — miss one and p-lock
    /// rejection silently stops working for that machine alone.
    #[test]
    fn every_variant_flags_machine_as_identity() {
        for v in AnalogEngine::machine_variants() {
            let (_, o) = v
                .overlays
                .iter()
                .find(|(pid, _)| *pid == ap("machine"))
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
        for m in AnalogMachine::ALL {
            let doc = AnalogEngine::build_doc(m);
            let p = doc
                .params
                .iter()
                .find(|p| p.id == ap("machine"))
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
        for m in AnalogMachine::ALL {
            let rule = AnalogEngine::new(m).to_rule(0, &[]);
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

    /// A variant's pages may only name params that variant declares — the
    /// BUG-037 class. HiHat has no `tune`, so no HiHat page may reference it.
    #[test]
    fn every_variant_page_ref_resolves_in_that_variants_params() {
        for m in AnalogMachine::ALL {
            let mut declared: Vec<u32> =
                AnalogEngine::machine_params(m).iter().map(|p| p.id).collect();
            // `machine` is on the union doc but is not a machine-specific
            // param, so `machine_params` excludes it. MM-C6 puts machine-select
            // on the TRIG page (ADR-041 §0 A2); without this it would look like
            // an unresolvable ref the moment that lands.
            declared.push(ap("machine"));
            // Same for the seven `lfo_*` params (MM-C9): one LFO per node, not
            // per machine, so `machine_params` excludes them too while the MOD
            // page carries them on every variant.
            declared.extend(LFO_PAGE_ORDER);
            for (pid, page_ref) in AnalogEngine::machine_page_refs(m) {
                assert!(
                    declared.contains(&pid),
                    "{m:?}'s {} page slot {} names param {pid}, which {m:?} does not declare",
                    page_ref.page, page_ref.slot
                );
            }
        }
    }

    /// A machine switch while a voice is sounding ramps to silence rather than
    /// cutting — the declick from MM §0 D1.
    ///
    /// Asserts the *samples*, not just the state machine: a hard mute would
    /// satisfy "swapped after a block" while producing exactly the click the
    /// fade exists to prevent.
    #[test]
    fn switch_while_sounding_fades_out_before_swapping() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        trigger_now(&mut eng);
        assert!(eng.active, "voice should be sounding");

        eng.bank.set(ap("machine"), AnalogMachine::Snare.value() as f64);
        eng.poll_machine_param();
        assert_eq!(
            eng.machine,
            AnalogMachine::Kick,
            "a sounding voice must fade first, not swap instantly"
        );
        assert!(eng.switch_fade.is_some());

        // The fade is shorter than one 512-sample block at 44.1 kHz (5 ms =
        // 220 samples), so one render completes it.
        let out = run_engine(&mut eng, &[]);
        assert_eq!(eng.machine, AnalogMachine::Snare, "swap after the fade");
        assert!(eng.switch_fade.is_none());
        assert!(!eng.active, "voice state resets on switch");

        // The ramp really ramped: no single-sample step anywhere near the size
        // of the signal it was attenuating.
        let peak = out.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!(peak > 0.01, "fixture: the fading voice must be audible");
        let max_step = out
            .windows(2)
            .fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()));
        assert!(
            max_step < peak * 0.5,
            "a {max_step:.4} step against a {peak:.4} peak is a cut, not a fade"
        );
        assert!(
            out[out.len() - 1].abs() < 1e-6,
            "the block must end silent, ready for the swap"
        );
    }

    /// M1 from MM-C3's review: cancelling a switch part-way used to drop the
    /// fade outright, snapping the gain from wherever it had reached back to
    /// unity — the same click, in the other direction. Reachable whenever the
    /// fade spans more than one block, which the tunable constant invites.
    #[test]
    fn cancelling_a_switch_ramps_back_instead_of_snapping() {
        let mut eng = AnalogEngine::kick();
        // 192 kHz makes the 5 ms fade 960 samples — longer than the 512-sample
        // block, which is exactly the condition that makes this reachable.
        eng.activate(192_000.0, 512);
        trigger_now(&mut eng);

        eng.bank.set(ap("machine"), AnalogMachine::Snare.value() as f64);
        eng.poll_machine_param();
        let fading = eng.switch_fade.expect("fade armed");
        assert_eq!(fading.target, Some(AnalogMachine::Snare));

        // Move it back before the fade completes.
        eng.bank.set(ap("machine"), AnalogMachine::Kick.value() as f64);
        eng.poll_machine_param();
        let cancelling = eng.switch_fade.expect("a cancel must still ramp");
        assert_eq!(
            cancelling.target, None,
            "a cancelled switch ramps back to unity and swaps nothing"
        );
        assert_eq!(
            eng.machine,
            AnalogMachine::Kick,
            "no swap happened, so the machine is unchanged"
        );
    }

    /// Retargeting mid-fade must continue from the gain already reached, not
    /// restart at unity.
    #[test]
    fn retargeting_mid_fade_keeps_the_gain_it_reached() {
        let mut eng = AnalogEngine::kick();
        // See above: the fade must outlast one block for a retarget to land
        // mid-ramp at all.
        eng.activate(192_000.0, 512);
        trigger_now(&mut eng);

        eng.bank.set(ap("machine"), AnalogMachine::Snare.value() as f64);
        eng.poll_machine_param();
        // Burn some of the fade.
        let _ = run_engine(&mut eng, &[]);
        let mid = eng.switch_fade.expect("still fading").remaining;
        assert!(mid < eng.fade_len(), "fixture: some fade must have elapsed");

        eng.bank.set(ap("machine"), AnalogMachine::HiHat.value() as f64);
        eng.poll_machine_param();
        let after = eng.switch_fade.expect("still fading");
        assert_eq!(after.target, Some(AnalogMachine::HiHat));
        assert!(
            after.remaining <= mid,
            "retarget restarted the fade ({} > {mid}), stepping the gain back to unity",
            after.remaining
        );
    }

    #[test]
    fn switch_while_silent_is_immediate() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        assert!(!eng.active);

        eng.bank.set(ap("machine"), AnalogMachine::HiHat.value() as f64);
        eng.poll_machine_param();
        assert_eq!(eng.machine, AnalogMachine::HiHat, "nothing to declick");
        assert!(eng.switch_fade.is_none());
    }

    /// A `ParamLock` on `machine` must not switch machines: `poll_machine_param`
    /// reads the bank, not `get_param`. ADR-041 decision 6 forbids p-locking it
    /// and MM-C6 rejects it surface-side; this is the engine-side guard,
    /// because the sequencer cannot know it is holding an identity param.
    #[test]
    fn a_param_lock_on_machine_does_not_switch() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        eng.set_node_id(20);

        // Fed as the executor actually delivers it — a ParamLock event through
        // `process()` — not by poking `node_locks` and calling poll directly.
        // `process()` polls before it clears and refills `node_locks`, so the
        // lock only exists on the *second* block; a one-block test would pass
        // against an implementation that reads `get_param` (design-process
        // learning 4).
        let lock = TimedEvent {
            sample_offset: 0,
            event: Event::ParamLock(ParamLockEvent {
                node_id: 20,
                param_id: ap("machine"),
                value: AnalogMachine::HiHat.value() as f64,
            }),
        };
        for _ in 0..3 {
            let _ = run_engine(&mut eng, std::slice::from_ref(&lock));
            assert_eq!(
                eng.machine,
                AnalogMachine::Kick,
                "a p-lock must never reach the machine switch"
            );
        }
        assert!(eng.switch_fade.is_none(), "and must not even arm a fade");
    }

    #[test]
    fn analog_portability_check() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        assert!(!eng.ports().is_empty());
    }

    #[test]
    fn analog_engine_published_state_contains_decay() {
        let mut eng = AnalogEngine::kick();
        eng.set_node_id(10);
        eng.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let decay_entry = buf.iter().find(|(k, _)| k.ends_with("/decay"));
        assert!(decay_entry.is_some(), "published_state should contain a /decay entry");
        assert!(matches!(decay_entry.unwrap().1, paraclete_node_api::StateBusValue::Float(_)),
            "decay entry should be StateBusValue::Float");
    }

    #[test]
    fn analog_engine_set_initial_params_applied() {
        let mut eng = AnalogEngine::kick();
        eng.set_node_id(1);
        eng.set_initial_params(&[("decay".to_string(), 0.9)].into_iter().collect());
        eng.activate(44100.0, 256);
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k.ends_with("/decay"));
        assert!(entry.is_some(), "published_state should contain /decay");
        if let paraclete_node_api::StateBusValue::Float(v) = entry.unwrap().1 {
            assert!((v - 0.9).abs() < 1e-9, "decay should be 0.9, got {v}");
        } else {
            panic!("decay entry should be Float");
        }
    }

    #[test]
    fn reactivate_does_not_reapply_initial_params() {
        // BUG-008: pending_initial_params must be consumed on first activate()
        // so a rebuild re-activate leaves the bank at defaults for deserialize()
        // to overlay (kick decay default = 0.5).
        let mut eng = AnalogEngine::kick();
        eng.set_node_id(1);
        eng.set_initial_params(&[("decay".to_string(), 0.9)].into_iter().collect());
        eng.activate(44100.0, 256);
        eng.activate(44100.0, 256); // dynamic-topology rebuild path
        let mut buf: Vec<(String, paraclete_node_api::StateBusValue)> = Vec::new();
        eng.published_state(&mut buf);
        let entry = buf.iter().find(|(k, _)| k.ends_with("/decay")).expect("/decay published");
        if let paraclete_node_api::StateBusValue::Float(v) = entry.1 {
            assert!((v - 0.5).abs() < 1e-9,
                "re-activate must not re-apply initial params (expected default 0.5, got {v})");
        } else {
            panic!("decay entry should be Float");
        }
    }

    #[test]
    fn set_initial_params_unknown_key_ignored() {
        let mut eng = AnalogEngine::snare();
        eng.set_initial_params(&[("nonexistent_param".to_string(), 99.0)].into_iter().collect());
        eng.activate(44100.0, 256); // must not panic
    }

    // ── W1 Commit 0: CMD_TRIGGER + velocity plumbing ─────────────────────────

    #[test]
    fn cmd_trigger_produces_audio() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let cmd = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 1.0 };
        let out = run_engine_cmds(&mut eng, &[], &[cmd]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5), "CMD_TRIGGER should produce audio");
    }

    #[test]
    fn cmd_trigger_negative_note_uses_default() {
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let cmd = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: -1, arg1: 1.0 };
        let out = run_engine_cmds(&mut eng, &[], &[cmd]);
        assert!(out.iter().any(|&s| s.abs() > 1e-5),
            "CMD_TRIGGER with arg0<0 should use the default/last note and produce audio");
    }

    #[test]
    fn velocity_scales_output_level() {
        let mut eng_hi = AnalogEngine::kick();
        eng_hi.activate(44100.0, 512);
        let cmd_hi = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 1.0 };
        let out_hi = run_engine_cmds(&mut eng_hi, &[], &[cmd_hi]);
        let peak_hi = out_hi.iter().fold(0.0f32, |m, &x| m.max(x.abs()));

        let mut eng_lo = AnalogEngine::kick();
        eng_lo.activate(44100.0, 512);
        let cmd_lo = NodeCommand { target_id: 0, type_id: CMD_TRIGGER, arg0: 36, arg1: 0.25 };
        let out_lo = run_engine_cmds(&mut eng_lo, &[], &[cmd_lo]);
        let peak_lo = out_lo.iter().fold(0.0f32, |m, &x| m.max(x.abs()));

        assert!(peak_hi > peak_lo,
            "higher velocity should produce a louder peak: hi={peak_hi:.4} lo={peak_lo:.4}");
        let ratio = peak_hi / peak_lo.max(1e-9);
        assert!(ratio > 2.0,
            "velocity ratio (1.0 vs 0.25) should roughly scale peak amplitude, got ratio={ratio:.2}");
    }

    #[test]
    fn note_on_mid_block_starts_at_its_sample_offset() {
        // BUG-013 regression: a NoteOn at offset 100 leaves samples 0..100
        // silent and sounds from 100 on — voice starts are sample-accurate,
        // not quantized to the block boundary.
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let mut ev = make_note_on(36);
        ev.sample_offset = 100;
        let out = run_engine(&mut eng, &[ev]);
        assert!(out[..100].iter().all(|&s| s == 0.0),
            "pre-offset span must be silent");
        assert!(out[100..].iter().any(|&s| s.abs() > 1e-6),
            "voice sounds from its offset");
    }


    #[test]
    fn two_notes_in_one_block_keep_their_own_velocities() {
        // Review finding (BUG-013 fix): velocity is baked per span — a
        // quiet second note must not rescale the loud first note's span.
        fn note_with_vel(offset: u32, vel: u16) -> TimedEvent {
            let mut msg = NoteOn::<[u32; 4]>::new();
            msg.set_group(u4::new(0));
            msg.set_channel(u4::new(0));
            msg.set_note_number(u7::new(36));
            msg.set_velocity(vel);
            TimedEvent::new(offset, Event::Midi2(UmpMessage::from(ChannelVoice2::from(msg))))
        }

        // Reference: a lone full-velocity hit; peak of its first 400 samples.
        let mut solo = AnalogEngine::kick();
        solo.activate(44100.0, 512);
        let solo_out = run_engine(&mut solo, &[note_with_vel(0, 65535)]);
        let solo_peak = solo_out[..400].iter().fold(0.0f32, |m, &s| m.max(s.abs()));

        // Same hit followed by a near-silent hit at offset 400.
        let mut eng = AnalogEngine::kick();
        eng.activate(44100.0, 512);
        let out = run_engine(&mut eng, &[note_with_vel(0, 65535), note_with_vel(400, 655)]);
        let first_peak = out[..400].iter().fold(0.0f32, |m, &s| m.max(s.abs()));

        assert!(first_peak > solo_peak * 0.9,
            "first note's span keeps its own velocity (solo {solo_peak}, got {first_peak})");
    }

}
