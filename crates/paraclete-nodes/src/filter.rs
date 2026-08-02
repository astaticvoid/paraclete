// SPDX-License-Identifier: GPL-3.0-or-later
//! FilterNode — Chamberlin state-variable filter (SVF).
//!
//! Parameters:
//!   cutoff   (id=0) — 20–20000 Hz, default 1000
//!   resonance   (id=1) — 0.1–4.0, default 0.7
//!   filter_type (id=2) — 0=LP, 1=HP, 2=BP, 3=Notch, default 0

use std::borrow::Cow;
use std::collections::HashMap;

use crate::engine_dsp::{
    lfo_params, sub_blocks, LfoDestLabels, LfoHost, LfoMode, LfoSettings, LfoShape,
    LFO_PAGE_ORDER,
};

use paraclete_node_api::{
    AffordanceHint, CapabilityDocument, Node, PageRef, ParameterBank, ParamDescriptor, ParamUnit,
    PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, Rule, StateBusValue,
    ViewPlugin,
};

/// This node's params are numbered, not name-hashed (it predates the
/// convention), but the `lfo_*` block must use canonical ids so it matches
/// every other host.
fn fid(name: &str) -> u32 { ParamDescriptor::id_for_name(name) }

/// MM-C10 dest names, in `FilterNode::DESTS` order. **APPEND ONLY.**
static FILTER_DEST_NAMES: &[&str] = &["cutoff", "resonance"];
static FILTER_DEST_LABELS: LfoDestLabels = LfoDestLabels(FILTER_DEST_NAMES);

const PARAM_CUTOFF:      u32 = 0;
const PARAM_RESONANCE:   u32 = 1;
const PARAM_FILTER_TYPE: u32 = 2;

pub struct FilterNode {
    ports: [PortDescriptor; 2],
    node_id: u32,
    bank: ParameterBank,
    pending_initial_params: HashMap<String, f64>,
    // SVF state (stereo)
    low_l:   f32,
    band_l:  f32,
    low_r:   f32,
    band_r:  f32,
    f_coeff: f32,
    q_coeff: f32,
    sr:      f32,
    /// MM-C10: one LFO per hosting node (ADR-042 decision 6's rollout order).
    lfo: LfoHost,
    /// The `(cutoff, resonance)` `update_coefficients` was last run for.
    ///
    /// Was a comparison against the **bank** (`process` held `prev_cutoff`/
    /// `prev_res` across `handle_commands`), which an LFO makes wrong every
    /// sub-block: the bank never moves, so the cache never refreshes and the
    /// modulation is silently inaudible. Keyed on the *effective* value now —
    /// post-lock, post-LFO — so it refreshes exactly when the filter it
    /// describes has actually changed.
    coeff_for: (f32, f32),
}

impl FilterNode {
    pub const PORT_AUDIO_IN:  u32 = 0;
    pub const PORT_AUDIO_OUT: u32 = 1;

    pub fn new() -> Self {
        Self {
            ports: [
                PortDescriptor {
                    id: Self::PORT_AUDIO_IN,
                    name: "audio_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    id: Self::PORT_AUDIO_OUT,
                    name: "audio_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Audio,
                },
            ],
            node_id: 0,
            bank: ParameterBank::empty(),
            pending_initial_params: HashMap::new(),
            low_l:   0.0,
            band_l:  0.0,
            low_r:   0.0,
            band_r:  0.0,
            f_coeff: 0.0,
            q_coeff: 0.0,
            sr:      44100.0,
            lfo:     LfoHost::new(),
            // Deliberately not a plausible pair, so the first
            // `refresh_coefficients` always runs.
            coeff_for: (f32::NAN, f32::NAN),
        }
    }

    fn default_doc() -> CapabilityDocument {
        CapabilityDocument {
            name: "FilterNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 4, 0),
            ports: vec![
                PortDescriptor { id: 0, name: "audio_in".into(),  direction: PortDirection::Input,  port_type: PortType::Audio },
                PortDescriptor { id: 1, name: "audio_out".into(), direction: PortDirection::Output, port_type: PortType::Audio },
            ],
            params: vec![
                ParamDescriptor { id: PARAM_CUTOFF,      name: "cutoff".into(),   min: 20.0,  max: 20000.0, default: 1000.0, stepped: false, in_kit: true,  unit: ParamUnit::Hz,      display: None },
                ParamDescriptor { id: PARAM_RESONANCE,   name: "resonance".into(),   min: 0.1,   max: 4.0,     default: 0.7,    stepped: false, in_kit: true, unit: ParamUnit::Generic, display: None },
                ParamDescriptor { id: PARAM_FILTER_TYPE, name: "filter_type".into(), min: 0.0,   max: 3.0,     default: 0.0,    stepped: true,  in_kit: false, unit: ParamUnit::Generic, display: None },
            ]
            .into_iter()
            .chain(lfo_params(Self::DESTS.len(), Some(&FILTER_DEST_LABELS)))
            .collect(),
            extensions: vec!["paraclete.effect".into()],
    view: None,
        }
    }

    /// **APPEND ONLY** — `lfo_dest` stores a one-based index into this. See
    /// `AnalogEngine::LFO_DESTS` for the full contract.
    ///
    /// `filter_type` is excluded: it is a stepped selector over filter shapes,
    /// not a continuous setting, so it is machine-class in the sense ADR-042
    /// decision 1 excludes. Sweeping a node through low-pass/high-pass/band
    /// at LFO rate is not a musical control, it is a fault.
    const DESTS: &'static [u32] = &[PARAM_CUTOFF, PARAM_RESONANCE];

    /// Bank value, before the LFO — used for the `lfo_*` params themselves.
    fn raw_param(&self, param_id: u32) -> f32 {
        self.bank.get(param_id) as f32
    }

    /// Effective value: bank, then the LFO on top (ADR-042 amendment 1).
    fn get_param(&self, param_id: u32) -> f32 {
        self.lfo.apply(param_id, self.raw_param(param_id))
    }

    fn lfo_settings(&self) -> LfoSettings {
        LfoSettings {
            shape: LfoShape::from_value(self.raw_param(fid("lfo_shape"))),
            mode: LfoMode::from_value(self.raw_param(fid("lfo_mode"))),
            speed_hz: self.raw_param(fid("lfo_speed")),
            start_phase: self.raw_param(fid("lfo_start_phase")),
            fade: self.raw_param(fid("lfo_fade")),
        }
    }

    fn lfo_dest_id(&self) -> Option<u32> {
        let v = self.raw_param(fid("lfo_dest"));
        if !v.is_finite() || v < 1.0 {
            return None;
        }
        Self::DESTS.get(v as usize - 1).copied()
    }

    fn update_lfo(&mut self, samples: usize) {
        let dest = self.lfo_dest_id();
        // Only meaningful when there IS a destination; `update` ignores the
        // range when `dest` is `None`.
        let range = dest
            .and_then(|d| self.bank.range(d))
            .map(|(lo, hi)| (lo as f32, hi as f32))
            .unwrap_or((0.0, 1.0));
        let depth = self.raw_param(fid("lfo_depth"));
        let settings = self.lfo_settings();
        let sr = self.sr;
        self.lfo.update(settings, dest, range, depth, sr, samples);
    }

    /// Recompute the SVF coefficients if the **effective** cutoff/resonance
    /// have moved since they were last derived.
    ///
    /// The old guard compared the *bank* across `handle_commands`. With an LFO
    /// the bank does not move at all — the modulation rides on `get_param` —
    /// so that guard would never fire and the filter would keep coefficients
    /// describing an unmodulated filter. Silently: the LFO would appear to do
    /// nothing rather than fail.
    fn refresh_coefficients(&mut self) {
        let cutoff = self.get_param(PARAM_CUTOFF);
        let res    = self.get_param(PARAM_RESONANCE);
        if (cutoff, res) == self.coeff_for {
            return;
        }
        // Chamberlin SVF — sin() form is stable at high cutoff; linear approx diverges near Nyquist
        self.f_coeff = 2.0 * (std::f32::consts::PI * cutoff / self.sr).sin();
        self.q_coeff = 1.0 / res;
        self.coeff_for = (cutoff, res);
    }

    #[inline(always)]
    fn svf_sample(&self, x: f32, low: &mut f32, band: &mut f32, filter_type: u32) -> f32 {
        let f = self.f_coeff.min(1.0); // stability guard
        let q = self.q_coeff;

        *low += f * *band;
        let high  = x - *low - q * *band;
        *band += f * high;
        let notch = high + *low;

        match filter_type {
            0 => *low,
            1 => high,
            2 => *band,
            3 => notch,
            _ => *low,
        }
    }
}

impl Default for FilterNode {
    fn default() -> Self { Self::new() }
}

impl ViewPlugin for FilterNode {
    fn to_rule(&self, _node_id: u64, _sub_nodes: &[(u64, &dyn ViewPlugin)]) -> Rule {
        Rule {
            name: Cow::Borrowed("Filter"),
            page_groups: Cow::Owned(vec![Cow::Borrowed("FLTR"), Cow::Borrowed("MOD")]),
            param_pages: Cow::Owned(
                vec![
                    (PARAM_CUTOFF,      PageRef { page: Cow::Borrowed("FLTR"), slot: 0 }),
                    (PARAM_RESONANCE,   PageRef { page: Cow::Borrowed("FLTR"), slot: 1 }),
                    (PARAM_FILTER_TYPE, PageRef { page: Cow::Borrowed("FLTR"), slot: 2 }),
                ]
                .into_iter()
                // MM-C10: the same MOD block every host lays out, from the
                // shared order. On a track that has an engine *and* a filter
                // the merged MOD page stacks both — 8-slot aligned, so the
                // filter's block starts on its own sub-page (ADR-042
                // decision 2, amendment 3).
                .chain(LFO_PAGE_ORDER.iter().enumerate().map(|(i, id)| {
                    (*id, PageRef { page: Cow::Borrowed("MOD"), slot: i as u8 })
                }))
                .collect::<Vec<_>>(),
            ),
            macros: Cow::Borrowed(&[]),
            affordances: Cow::Owned(vec![
                (PARAM_CUTOFF,    AffordanceHint::FilterShape),
                (PARAM_RESONANCE, AffordanceHint::FilterShape),
                (fid("lfo_shape"), AffordanceHint::LfoShape),
            ]),
            envelopes: Cow::Borrowed(&[]),
            routing: Cow::Borrowed(&[]),
            diagram: None,
            view_overrides: Cow::Borrowed(&[]),
            variants: Cow::Borrowed(&[]),
        }
    }
}

impl Node for FilterNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }
    fn set_node_id(&mut self, id: u32) { self.node_id = id; }

    fn capability_document(&self) -> CapabilityDocument {
        let mut doc = Self::default_doc();
        doc.view = Some(self.to_rule(0, &[]));
        doc
    }

    fn set_initial_params(&mut self, params: &HashMap<String, f64>) {
        self.pending_initial_params = params.clone();
    }

    /// Bank only (#154). The SVF's `low`/`band` memory and the cached
    /// coefficients are transient — persisting filter state would reload a
    /// snapshot of a waveform mid-flight.
    fn serialize(&self) -> Vec<u8> {
        self.bank.serialize()
    }

    fn deserialize(&mut self, data: &[u8]) {
        self.bank.deserialize(data);
    }

    fn published_state(&self, buf: &mut Vec<(String, StateBusValue)>) {
        paraclete_node_api::publish_bank_state(self.node_id, &self.bank, buf);
    }

    fn activate(&mut self, sr: f32, _block: usize) {
        self.sr   = sr;
        let doc = Self::default_doc();
        self.bank = ParameterBank::from_capability_document(&doc);
        // BUG-008 fix: consume the pending map so a re-activate (dynamic
        // topology rebuild, P9 C4) cannot overwrite deserialized state.
        for (name, value) in std::mem::take(&mut self.pending_initial_params) {
            if let Some(param) = doc.params.iter().find(|p| p.name.as_str() == name.as_str()) {
                self.bank.set(param.id, value);
            }
        }
        self.refresh_coefficients();
        self.low_l  = 0.0;
        self.band_l = 0.0;
        self.low_r  = 0.0;
        self.band_r = 0.0;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        self.bank.handle_commands(input.commands);

        if let (Some(audio_in), Some(audio_out)) = (
            input.audio_inputs.first(),
            output.audio_outputs.first_mut(),
        ) {
            let frames = input.block_size;
            let filter_type = self.bank.get(PARAM_FILTER_TYPE) as u32;

            // MM-C10: `FilterNode` had no `render_span` to chunk, so the
            // sub-block loop is new here — and unlike the engines it wraps
            // *both* channels, because the two share one set of coefficients
            // and must see the same LFO value for the same samples. Ticking
            // per channel would give the right channel a sub-block-old
            // modulation and split the stereo image.
            for (lo, hi) in sub_blocks(0, frames) {
                self.update_lfo(hi - lo);
                self.refresh_coefficients();

                if audio_in.channels() >= 1 && audio_out.channels() >= 1 {
                    let (mut l, mut b) = (self.low_l, self.band_l);
                    {
                        let src = audio_in.channel(0);
                        let dst = audio_out.channel_mut(0);
                        for f in lo..hi {
                            dst[f] = self.svf_sample(src[f], &mut l, &mut b, filter_type);
                        }
                    }
                    self.low_l  = l;
                    self.band_l = b;
                }
                if audio_out.channels() >= 2 {
                    let (mut l, mut b) = (self.low_r, self.band_r);
                    {
                        let stereo_in = audio_in.channels() >= 2;
                        let src = audio_in.channel(if stereo_in { 1 } else { 0 });
                        let dst = audio_out.channel_mut(1);
                        for f in lo..hi {
                            dst[f] = self.svf_sample(src[f], &mut l, &mut b, filter_type);
                        }
                    }
                    self.low_r  = l;
                    self.band_r = b;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{AudioBuffer, EventOutputBuffer, ExtendedEventSlab, TransportInfo};

    /// Both tables are declared APPEND ONLY in prose and, until #154, nothing
    /// enforced it. They are now written into project files — `lfo_dest`
    /// stores a one-based index into `DESTS`, and every param value is keyed
    /// by these id constants — so a reorder or a renumber silently re-points
    /// saved patches instead of failing. Extend these arrays; never reorder
    /// them. (AGENTS.md design-learning 9: declared-but-unenforced contracts
    /// rot silently.)
    #[test]
    fn the_persisted_id_tables_are_append_only() {
        assert_eq!(
            [PARAM_CUTOFF, PARAM_RESONANCE, PARAM_FILTER_TYPE],
            [0, 1, 2],
            "param id constants are persisted verbatim by ParameterBank::serialize"
        );
        assert_eq!(
            FilterNode::DESTS,
            &[PARAM_CUTOFF, PARAM_RESONANCE],
            "`lfo_dest` stores a one-based index into this"
        );
        assert_eq!(FILTER_DEST_NAMES, &["cutoff", "resonance"]);
        assert_eq!(
            FILTER_DEST_NAMES.len(),
            FilterNode::DESTS.len(),
            "the label table and the id table must stay in step"
        );
    }

    fn run_filter(filter: &mut FilterNode, input_val: f32, frames: usize) -> Vec<f32> {
        let mut src = AudioBuffer::new(2, frames);
        let mut dst = AudioBuffer::new(2, frames);
        let mut events_out = EventOutputBuffer::new(16);
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();
        src.channel_mut(0).fill(input_val);

        let dst_ptr: *mut AudioBuffer = &mut dst;
        let dst_ref: &mut AudioBuffer = unsafe { &mut *dst_ptr };
        let mut outs = [dst_ref];

        let input = ProcessInput {
            audio_inputs: &[&src],
            signal_inputs: &[],
            events: &[],
            transport: &transport,
            sample_rate: 44100.0,
            block_size: frames,
            extended_events: &slab,
            commands: &[],
        };
        filter.process(&input, &mut ProcessOutput::new(
            &mut outs,
            &mut [],
            &mut events_out,
        ));
        dst.channel(0).to_vec()
    }

    // ── MM-C10 ───────────────────────────────────────────────────────────

    /// **The primary guard for MM-C10**, deliberately not a baseline.
    ///
    /// `fx_chain` catches filter *coefficient* changes but was measured to
    /// MISS filter-state re-sequencing at 1% per block — the SVF re-converges
    /// within a block and the reverb smooths what is left. That is exactly
    /// what a sub-block restructure threatens, so the guard cannot be a
    /// baseline: it has to compare the restructured render against the
    /// un-restructured one directly, with no reverb in between.
    ///
    /// Expected to fail at the moment `lfo_depth` is non-zero — that is the
    /// point of the structure, not a regression. Update it deliberately.
    #[test]
    fn chunked_filter_render_is_identical_to_one_unchunked_pass() {
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};

        // 500 is not a multiple of 64, so the final short chunk exercises the
        // `.min(end)` clamp a 512-frame block never would.
        const FRAMES: usize = 500;
        let setup = |f: &mut FilterNode| {
            f.activate(44100.0, FRAMES);
            f.bank.handle_commands(&[
                NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_CUTOFF as i64, arg1: 900.0 },
                NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_RESONANCE as i64, arg1: 3.0 },
            ]);
            f.refresh_coefficients();
        };

        // A signal with content, so filter state actually matters.
        let signal: Vec<f32> = (0..FRAMES)
            .map(|i| ((i as f32 * 0.07).sin() + (i as f32 * 0.31).sin()) * 0.4)
            .collect();

        let mut chunked = FilterNode::new();
        setup(&mut chunked);
        let mut whole = FilterNode::new();
        setup(&mut whole);

        // Un-chunked reference: one pass over the whole block.
        let mut want = vec![0.0f32; FRAMES];
        {
            let (mut l, mut b) = (whole.low_l, whole.band_l);
            let ft = whole.bank.get(PARAM_FILTER_TYPE) as u32;
            for (i, w) in want.iter_mut().enumerate() {
                *w = whole.svf_sample(signal[i], &mut l, &mut b, ft);
            }
        }

        // Chunked, through the real per-sub-block path.
        let mut got = vec![0.0f32; FRAMES];
        {
            let ft = chunked.bank.get(PARAM_FILTER_TYPE) as u32;
            for (lo, hi) in sub_blocks(0, FRAMES) {
                chunked.update_lfo(hi - lo);
                chunked.refresh_coefficients();
                let (mut l, mut b) = (chunked.low_l, chunked.band_l);
                for i in lo..hi {
                    got[i] = chunked.svf_sample(signal[i], &mut l, &mut b, ft);
                }
                chunked.low_l = l;
                chunked.band_l = b;
            }
        }

        assert_eq!(
            got, want,
            "chunking changed the filter's output — the SVF state was \
             re-sequenced across a sub-block boundary. Find it; do not \
             re-fingerprint."
        );
    }

    /// The coefficient cache keys on the **effective** value, not the bank.
    /// With an LFO the bank never moves, so a bank-keyed guard would never
    /// refresh and the modulation would be silently inaudible — which is how
    /// this would fail in practice: not a wrong sound, no sound.
    #[test]
    fn the_coefficient_cache_refreshes_when_the_effective_value_moves() {
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        let mut f = FilterNode::new();
        f.activate(44100.0, 512);
        f.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_CUTOFF as i64, arg1: 1000.0 },
            // dest 1 = cutoff, full depth, fast enough to move per block.
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fid("lfo_dest") as i64, arg1: 1.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fid("lfo_depth") as i64, arg1: 1.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fid("lfo_speed") as i64, arg1: 8.0 },
        ]);
        f.refresh_coefficients();

        let bank_cutoff = f.bank.get(PARAM_CUTOFF);
        let mut seen: Vec<f32> = Vec::new();
        for _ in 0..200 {
            f.update_lfo(64);
            f.refresh_coefficients();
            seen.push(f.f_coeff);
        }
        assert_eq!(
            f.bank.get(PARAM_CUTOFF),
            bank_cutoff,
            "the LFO must never write the bank"
        );
        let first = seen[0];
        assert!(
            seen.iter().any(|c| (*c - first).abs() > 1e-6),
            "the coefficients never moved — a bank-keyed cache guard would \
             look exactly like this, and the LFO would be inaudible"
        );
    }

    #[test]
    fn the_filter_dest_ids_and_names_correspond() {
        assert_eq!(FILTER_DEST_NAMES.len(), FilterNode::DESTS.len());
        assert_eq!(FilterNode::DESTS, &[PARAM_CUTOFF, PARAM_RESONANCE]);
        assert!(
            !FilterNode::DESTS.contains(&PARAM_FILTER_TYPE),
            "filter_type is a stepped shape selector, not a sweepable setting"
        );
    }

    #[test]
    fn filter_depth_zero_is_bit_identical_to_no_lfo() {
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        let mut f = FilterNode::new();
        f.activate(44100.0, 512);
        f.bank.handle_commands(&[
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fid("lfo_dest") as i64, arg1: 1.0 },
            NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: fid("lfo_speed") as i64, arg1: 8.0 },
        ]);
        let base = f.raw_param(PARAM_CUTOFF);
        for _ in 0..40 {
            f.update_lfo(64);
            assert_eq!(f.get_param(PARAM_CUTOFF), base);
        }
    }

    #[test]
    fn filter_at_high_cutoff_passes_dc() {
        let mut f = FilterNode::new();
        f.activate(44100.0, 64);
        // High cutoff → DC passes through LP
        use paraclete_node_api::{NodeCommand, CMD_SET_PARAM};
        f.bank.handle_commands(&[NodeCommand { target_id: 0, type_id: CMD_SET_PARAM, arg0: PARAM_CUTOFF as i64, arg1: 18000.0 }]);
        f.refresh_coefficients();
        let out = run_filter(&mut f, 1.0, 64);
        // After settling, output should be close to input (LP at high cutoff)
        assert!(out[63].abs() > 0.1, "expected signal to pass, got {}", out[63]);
    }

    #[test]
    fn filter_state_zeroed_between_activate_calls() {
        let mut f = FilterNode::new();
        f.activate(44100.0, 64);
        run_filter(&mut f, 1.0, 64);
        f.activate(44100.0, 64); // re-activate clears state
        assert_eq!(f.low_l, 0.0);
        assert_eq!(f.band_l, 0.0);
    }

    #[test]
    fn set_node_id_stored() {
        let mut f = FilterNode::new();
        f.set_node_id(99);
        f.activate(44100.0, 64);
        let mut buf: Vec<(String, StateBusValue)> = Vec::new();
        f.published_state(&mut buf);
        assert!(buf.iter().any(|(k, _)| k.starts_with("/node/99/")),
            "published_state paths should start with /node/99/");
    }
}
