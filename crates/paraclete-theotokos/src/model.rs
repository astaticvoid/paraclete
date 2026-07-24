use crate::action::GRID_STEPS;
use paraclete_node_api::{CapabilityDocument, PageRef, StateBusHandle, StateBusValue};
use paraclete_view_assembly::CompositeView;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Seq,
    Perf,
}

impl Mode {
    pub fn next(self) -> Self {
        match self {
            Mode::Seq => Mode::Perf,
            Mode::Perf => Mode::Seq,
        }
    }
}

/// TK2 C2 (D12): replaces `Mode` — additive for now (§0 A4). `Mode` and
/// the TK1 dispatch it drives stay live until C3's wiring flip; nothing
/// reads `Screen` yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Grid,
    Param(usize),
    Tempo,
    Chain,
    Settings,
    Mute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Prev,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
    C,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mag {
    Normal,
    Fine,
    Coarse,
}

#[derive(Clone)]
pub struct SlotBinding {
    pub node_id: u32,
    pub param_id: u32,
    pub param_name: String,
    pub min: f64,
    pub max: f64,
}

pub struct TrackInfo {
    pub sequencer_id: u32,
    pub generator_id: u32,
    pub name: String,
}

pub struct Model {
    pub mode: Mode,
    pub active_track: usize,
    pub tracks: Vec<TrackInfo>,
    pub clock_id: u32,
    pub page_windows: Vec<usize>,
    pub caps: HashMap<u32, CapabilityDocument>,
    /// TK1 C3: composite views, one per track.
    pub composite: Vec<CompositeView>,
    pub perf_page: usize,
    pub slot_a: Option<SlotBinding>,
    pub slot_b: Option<SlotBinding>,
    pub step_focus: Vec<Option<usize>>,
    pub last_step: Vec<Option<usize>>,
    /// TK1 C6: command line editor state (None = closed).
    pub cmdline: Option<String>,
    /// TK1 C6: error message from last command execution (shown in red).
    pub cmdline_error: Option<String>,
    /// TK1 C6: fuzzy index built at startup from caps + tracks + static verbs.
    pub fuzzy_index: Vec<FuzzyEntry>,
    /// TK1 C7: yanked pattern data for paste.
    pub yank_buffer: Vec<YankedStep>,
    /// TK1 C7: leader state for \\-prefix key chords (slot rebinding).
    pub leader: Option<LeaderState>,
    /// TK1 C7: Instant when each slot value last changed (for yellow flash).
    pub slot_flash: [Option<std::time::Instant>; 2],
    /// TK1 C7: previous slot values (to detect change).
    pub last_slot_values: [f64; 2],
    /// Visibility toggle for help overlay (shows mode-specific key bindings).
    pub help_visible: bool,
}

/// TK1 C7: a yanked step for paste.
#[derive(Clone, Default)]
pub struct YankedStep {
    pub active: bool,
    pub note: i64,
    pub velocity: f64,
    pub length: f64,
    pub timing: i64,
    pub condition: f64,
    pub locks: Vec<YankedLock>,
}

/// TK1 C7: a yanked param lock.
#[derive(Clone)]
pub struct YankedLock {
    pub node_id: u32,
    pub param_id: u32,
    pub value: f64,
}

/// TK1 C7: `\\` leader chord state for slot rebinding.
#[derive(Clone)]
pub struct LeaderState {
    pub slot: Option<Slot>,
}

/// TK1 C6: a searchable entry in the fuzzy command index.
#[derive(Clone)]
pub struct FuzzyEntry {
    pub text: String,
    pub category: String,
}

/// TK1 C6: parsed command from the `:` line.
pub enum CmdlineVerb {
    Set {
        node_id: u32,
        param_name: String,
        value: f64,
    },
    Bpm(f64),
    Track(usize),
    Pattern(usize),
    Mute(usize),
    Unmute(usize),
    Clear,
    LockClear,
    Mode(Mode),
}

impl Model {
    pub fn new(
        clock_id: u32,
        seq_ids: &[u32],
        gen_ids: &[u32],
        gen_names: &[String],
        caps: HashMap<u32, CapabilityDocument>,
        composite: Vec<CompositeView>,
    ) -> Self {
        let count = seq_ids.len().min(gen_ids.len());
        let tracks: Vec<TrackInfo> = (0..count)
            .map(|i| TrackInfo {
                sequencer_id: seq_ids[i],
                generator_id: gen_ids[i],
                name: gen_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Trk{}", i + 1)),
            })
            .collect();
        let track_count = tracks.len();
        let page_windows: Vec<usize> = vec![0; track_count];
        let step_focus: Vec<Option<usize>> = vec![None; track_count];
        let last_step: Vec<Option<usize>> = vec![None; track_count];
        let fuzzy_index = Self::build_fuzzy_index(&caps, &tracks);
        let mut model = Self {
            mode: Mode::Seq,
            active_track: 0,
            tracks,
            clock_id,
            page_windows,
            caps,
            composite,
            perf_page: 0,
            slot_a: None,
            slot_b: None,
            step_focus,
            last_step,
            cmdline: None,
            cmdline_error: None,
            fuzzy_index,
            yank_buffer: Vec::new(),
            leader: None,
            slot_flash: [None; 2],
            last_slot_values: [0.0; 2],
            help_visible: false,
        };
        model.bind_page();
        model
    }

    pub fn cycle_mode(&mut self) {
        self.mode = self.mode.next();
    }

    pub fn select_track(&mut self, i: usize) {
        if i < self.tracks.len() {
            self.active_track = i;
            self.bind_page();
        }
    }

    pub fn select_perf_page(&mut self, idx: usize) {
        let max = self
            .composite
            .get(self.active_track)
            .map(|cv| cv.pages.len())
            .unwrap_or_else(|| {
                let gen_id = self.tracks[self.active_track].generator_id;
                self.caps
                    .get(&gen_id)
                    .and_then(|c| c.view.as_ref())
                    .map(|r| r.page_groups.len())
                    .unwrap_or(0)
            });
        if idx >= max {
            return;
        }
        self.perf_page = idx;
        self.bind_page();
    }

    fn bind_page(&mut self) {
        let (a, b) = self.resolve_page_params();
        self.slot_a = a.map(|(nid, pid, name, min, max)| SlotBinding {
            node_id: nid,
            param_id: pid,
            param_name: name,
            min,
            max,
        });
        self.slot_b = b.map(|(nid, pid, name, min, max)| SlotBinding {
            node_id: nid,
            param_id: pid,
            param_name: name,
            min,
            max,
        });
    }

    fn resolve_page_params(
        &self,
    ) -> (
        Option<(u32, u32, String, f64, f64)>,
        Option<(u32, u32, String, f64, f64)>,
    ) {
        // TK1 C3: composite pages first — slot A/B bind from the composite
        // page's params (which know their owning node_id).
        if let Some(cv) = self.composite.get(self.active_track) {
            if let Some(page) = cv.pages.get(self.perf_page) {
                let a = page
                    .params
                    .first()
                    .map(|p| (p.node_id, p.param_id, p.name.clone(), 0.0, 1.0));
                let b = page
                    .params
                    .get(1)
                    .map(|p| (p.node_id, p.param_id, p.name.clone(), 0.0, 1.0));
                if a.is_some() {
                    return (a, b);
                }
            }
        }
        // Fallback: engine-local Rule (existing TK0 path).
        let gen_id = self.tracks[self.active_track].generator_id;
        let cap = match self.caps.get(&gen_id) {
            Some(c) => c,
            None => return (None, None),
        };
        let rule = match &cap.view {
            Some(r) => r,
            None => return (None, None),
        };
        let page = match rule.page_groups.get(self.perf_page) {
            Some(p) => p.as_ref(),
            None => {
                let fallback_params = &cap.params;
                let a = fallback_params.first();
                let b = fallback_params.get(1);
                return (
                    a.map(|p| (gen_id, p.id, p.name.to_string(), p.min, p.max)),
                    b.map(|p| (gen_id, p.id, p.name.to_string(), p.min, p.max)),
                );
            }
        };

        let mut params: Vec<&(u32, PageRef)> = rule
            .param_pages
            .iter()
            .filter(|(_, pr)| pr.page.as_ref() == page)
            .collect();
        params.sort_by_key(|(_, pr)| pr.slot);
        let a = params.first().and_then(|(pid, _)| {
            cap.params
                .iter()
                .find(|pd| pd.id == *pid)
                .map(|pd| (gen_id, pd.id, pd.name.to_string(), pd.min, pd.max))
        });
        let b = params.get(1).and_then(|(pid, _)| {
            cap.params
                .iter()
                .find(|pd| pd.id == *pid)
                .map(|pd| (gen_id, pd.id, pd.name.to_string(), pd.min, pd.max))
        });
        (a, b)
    }

    pub fn playing(&self, bus: &StateBusHandle) -> bool {
        bus.read("/transport/playing")
            .map(|v| matches!(v, StateBusValue::Bool(true)))
            .unwrap_or(false)
    }

    pub fn read_bpm(&self, bus: &StateBusHandle) -> f64 {
        bus.read("/transport/bpm")
            .and_then(|v| match v {
                StateBusValue::Float(f) => Some(*f),
                _ => None,
            })
            .unwrap_or(120.0)
    }

    pub fn read_param_value(&self, bus: &StateBusHandle, node_id: u32, param_id: u32) -> f64 {
        let param_name = self
            .caps
            .get(&node_id)
            .and_then(|c| c.params.iter().find(|p| p.id == param_id))
            .map(|p| p.name.to_string());

        match param_name {
            Some(name) => bus
                .read(&format!("/node/{}/param/{}", node_id, name))
                .and_then(|v| match v {
                    StateBusValue::Float(f) => Some(*f),
                    StateBusValue::Int(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(0.0),
            None => 0.0,
        }
    }

    pub fn read_step_state(&self, bus: &StateBusHandle, track_idx: usize) -> StepState {
        let seq_id = self.tracks[track_idx].sequencer_id;

        let current_step = bus
            .read(&format!("/node/{}/state/current_step", seq_id))
            .and_then(|v| match v {
                StateBusValue::Int(i) => Some(*i as usize),
                _ => None,
            })
            .unwrap_or(0);

        let pattern_length = bus
            .read(&format!("/node/{}/state/pattern_length", seq_id))
            .and_then(|v| match v {
                StateBusValue::Int(i) => Some(*i as usize),
                _ => None,
            })
            .unwrap_or(16);

        let steps_text = bus
            .read(&format!("/node/{}/state/steps", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let steps: Vec<bool> = steps_text.chars().map(|c| c == '1').collect();

        let page_count = pattern_length.div_ceil(GRID_STEPS);

        StepState {
            current_step,
            pattern_length,
            steps,
            page_count,
        }
    }

    pub fn page_groups_for_active_track(&self) -> Vec<String> {
        // TK1 C3: composite page labels first.
        if let Some(cv) = self.composite.get(self.active_track) {
            if !cv.pages.is_empty() {
                return cv.pages.iter().map(|p| p.label.clone()).collect();
            }
        }
        let gen_id = self.tracks[self.active_track].generator_id;
        self.caps
            .get(&gen_id)
            .and_then(|c| c.view.as_ref())
            .map(|r| r.page_groups.iter().map(|g| g.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn read_lock_value(
        &self,
        bus: &StateBusHandle,
        sequencer_id: u32,
        step: usize,
        node_id: u32,
        param_id: u32,
    ) -> Option<f64> {
        let locks_text = bus
            .read(&format!("/node/{}/state/locks", sequencer_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Self::parse_lock_value(&locks_text, step, node_id, param_id)
    }

    pub fn parse_lock_value(
        locks_text: &str,
        step: usize,
        node_id: u32,
        param_id: u32,
    ) -> Option<f64> {
        for entry in locks_text.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let parts: Vec<&str> = entry.splitn(4, [':', '=']).collect();
            if parts.len() != 4 {
                continue;
            }
            let entry_step: usize = parts[0].strip_prefix('s').and_then(|s| s.parse().ok())?;
            let entry_nid: u32 = parts[1].parse().ok()?;
            let entry_pid: u32 = parts[2].parse().ok()?;
            if entry_step == step && entry_nid == node_id && entry_pid == param_id {
                return parts[3].parse::<f64>().ok();
            }
        }
        None
    }

    pub fn read_step_locks(&self, bus: &StateBusHandle, track_idx: usize) -> Vec<usize> {
        let seq_id = self.tracks[track_idx].sequencer_id;
        let locks_text = bus
            .read(&format!("/node/{}/state/locks", seq_id))
            .and_then(|v| match v {
                StateBusValue::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let mut steps: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for entry in locks_text.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some(rest) = entry.strip_prefix('s') {
                if let Some(colon) = rest.find(':') {
                    if let Ok(s) = rest[..colon].parse::<usize>() {
                        steps.insert(s);
                    }
                }
            }
        }
        let mut sorted: Vec<usize> = steps.into_iter().collect();
        sorted.sort_unstable();
        sorted
    }

    pub fn envelope_for_active_track(&self) -> Option<EnvelopeData> {
        let gen_id = self.tracks[self.active_track].generator_id;
        let cap = self.caps.get(&gen_id)?;
        let rule = cap.view.as_ref()?;
        let env = rule.envelopes.first()?;
        let pid = env.param_ids[0];
        if pid == 0 {
            return None;
        }
        let param = cap.params.iter().find(|p| p.id == pid)?;
        Some(EnvelopeData {
            param_id: pid,
            param_name: param.name.to_string(),
            node_id: gen_id,
            env_type: env.env_type.to_string(),
            min: param.min,
            max: param.max,
        })
    }

    // ── C6: command line ──

    pub fn build_fuzzy_index(
        caps: &HashMap<u32, CapabilityDocument>,
        tracks: &[TrackInfo],
    ) -> Vec<FuzzyEntry> {
        let mut entries = Vec::new();
        // static verbs
        for verb in &[
            "set",
            "bpm",
            "track",
            "pattern",
            "mute",
            "unmute",
            "clear",
            "lock-clear",
            "mode",
        ] {
            entries.push(FuzzyEntry {
                text: verb.to_string(),
                category: "verb".into(),
            });
        }
        // param names from all cap-docs
        for cap in caps.values() {
            for p in &cap.params {
                entries.push(FuzzyEntry {
                    text: p.name.to_string(),
                    category: "param".into(),
                });
            }
        }
        // track names
        for t in tracks {
            entries.push(FuzzyEntry {
                text: t.name.to_string(),
                category: "track".into(),
            });
        }
        entries
    }

    /// Returns top candidates matching `query` using subsequence fuzzy match.
    pub fn cmdline_candidates(&self) -> Vec<String> {
        let query = match &self.cmdline {
            Some(s) if !s.is_empty() => s,
            _ => return vec![],
        };
        let lower = query.to_lowercase();
        let mut scored: Vec<(&FuzzyEntry, usize)> = self
            .fuzzy_index
            .iter()
            .filter_map(|e| {
                let text = e.text.to_lowercase();
                let score = Self::fuzzy_score(&lower, &text)?;
                Some((e, score))
            })
            .collect();
        scored.sort_by_key(|(e, s)| (*s, e.text.len()));
        scored.dedup_by_key(|(e, _)| &e.text);
        scored
            .into_iter()
            .take(5)
            .map(|(e, _)| e.text.clone())
            .collect()
    }

    fn fuzzy_score(query: &str, target: &str) -> Option<usize> {
        let mut qi = query.chars();
        let mut qc = qi.next()?;
        let mut score = 0usize;
        for (i, tc) in target.char_indices() {
            if tc.to_ascii_lowercase() == qc {
                // first char match = prefix bonus
                if i == 0 && score == 0 {
                    score = 0;
                }
                qc = match qi.next() {
                    Some(c) => c,
                    None => return Some(score),
                };
            }
            score = score.saturating_add(1);
        }
        if qi.next().is_some() {
            None // not all query chars consumed
        } else {
            Some(score)
        }
    }

    pub fn parse_cmdline(&self, input: &str) -> Result<CmdlineVerb, String> {
        let input = input.trim();
        if input.is_empty() {
            return Err("empty command".into());
        }
        let (verb, rest) = match input.split_once(char::is_whitespace) {
            Some((v, r)) => (v, r.trim()),
            None => (input, ""),
        };
        match verb {
            "set" => {
                let (name, val) = rest
                    .rsplit_once(char::is_whitespace)
                    .ok_or_else(|| "set <param> <value>".to_string())?;
                let value: f64 = val.parse().map_err(|_| format!("invalid value: {val}"))?;
                let lname = name.to_lowercase();
                let best = self
                    .fuzzy_index
                    .iter()
                    .filter(|e| e.category == "param")
                    .filter_map(|e| {
                        let score = Self::fuzzy_score(&lname, &e.text.to_lowercase())?;
                        Some((e, score))
                    })
                    .min_by_key(|(_, s)| *s)
                    .ok_or_else(|| format!("unknown param: {name}"))?;
                // Find the node that has this param on the active track
                let track = &self.tracks[self.active_track];
                let mut node_id = track.generator_id;
                let mut found = self.caps.get(&node_id).is_some_and(|c| {
                    c.params
                        .iter()
                        .any(|p| p.name.to_string().to_lowercase() == best.0.text.to_lowercase())
                });
                // Also check composite chain nodes
                if !found {
                    if let Some(cv) = self.composite.get(self.active_track) {
                        for page in &cv.pages {
                            for cp in &page.params {
                                let cap = self.caps.get(&cp.node_id);
                                if cap.is_some_and(|c| c.params.iter().any(|p| p.id == cp.param_id))
                                {
                                    node_id = cp.node_id;
                                    found = true;
                                    break;
                                }
                            }
                            if found {
                                break;
                            }
                        }
                    }
                }
                if !found {
                    return Err(format!("param {} not found on active track", best.0.text));
                }
                Ok(CmdlineVerb::Set {
                    node_id,
                    param_name: best.0.text.clone(),
                    value: value.clamp(0.0, 1.0),
                })
            }
            "bpm" => {
                let val: f64 = rest.parse().map_err(|_| "bpm <number>".to_string())?;
                Ok(CmdlineVerb::Bpm(val.clamp(20.0, 300.0)))
            }
            "track" => {
                let n: usize = rest
                    .parse::<usize>()
                    .map_err(|_| "track <number>".to_string())?;
                if n < 1 || n > self.tracks.len() {
                    return Err(format!(
                        "track {} out of range (1-{})",
                        n,
                        self.tracks.len()
                    ));
                }
                Ok(CmdlineVerb::Track(n - 1))
            }
            "pattern" => {
                let n: usize = rest.parse().map_err(|_| "pattern <number>".to_string())?;
                if n < 1 {
                    return Err("pattern <number>".into());
                }
                Ok(CmdlineVerb::Pattern(n - 1))
            }
            "mute" => {
                let n: usize = rest
                    .parse()
                    .map_err(|_| "mute <track number>".to_string())?;
                if n < 1 || n > self.tracks.len() {
                    return Err(format!("track {} out of range", n));
                }
                Ok(CmdlineVerb::Mute(n - 1))
            }
            "unmute" => {
                let n: usize = rest
                    .parse()
                    .map_err(|_| "unmute <track number>".to_string())?;
                if n < 1 || n > self.tracks.len() {
                    return Err(format!("track {} out of range", n));
                }
                Ok(CmdlineVerb::Unmute(n - 1))
            }
            "clear" => Ok(CmdlineVerb::Clear),
            "lock-clear" => Ok(CmdlineVerb::LockClear),
            "mode" => match rest {
                "seq" => Ok(CmdlineVerb::Mode(Mode::Seq)),
                "perf" => Ok(CmdlineVerb::Mode(Mode::Perf)),
                other => Err(format!("unknown mode: {other} (seq or perf)")),
            },
            _ => Err(format!("?{input}")),
        }
    }

    // ── C7: yank/paste, leader, flash ──

    pub fn set_slot_lead(&mut self, slot: Slot, dig: usize) {
        let n = dig.saturating_sub(1);
        let params = self.current_page_params();
        if n < params.len() {
            let (node_id, param_id, name) = &params[n];
            let binding = SlotBinding {
                node_id: *node_id,
                param_id: *param_id,
                param_name: name.clone(),
                min: 0.0,
                max: 1.0,
            };
            match slot {
                Slot::A => self.slot_a = Some(binding),
                Slot::B => self.slot_b = Some(binding),
                Slot::C => {}
            }
        }
    }

    fn current_page_params(&self) -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        if let Some(cv) = self.composite.get(self.active_track) {
            if let Some(page) = cv.pages.get(self.perf_page) {
                for p in &page.params {
                    out.push((p.node_id, p.param_id, p.name.clone()));
                }
                return out;
            }
        }
        let gen_id = self.tracks[self.active_track].generator_id;
        if let Some(cap) = self.caps.get(&gen_id) {
            for p in &cap.params {
                out.push((gen_id, p.id, p.name.to_string()));
            }
        }
        out
    }

    pub fn update_flash(&mut self, slot: usize, new_value: f64) {
        if (new_value - self.last_slot_values[slot]).abs() > 0.0001 {
            self.slot_flash[slot] = Some(std::time::Instant::now());
            self.last_slot_values[slot] = new_value;
        }
    }
}

#[derive(Clone, Default)]
pub struct StepState {
    pub current_step: usize,
    pub pattern_length: usize,
    pub steps: Vec<bool>,
    pub page_count: usize,
}

#[derive(Clone)]
pub struct EnvelopeData {
    pub param_id: u32,
    pub param_name: String,
    pub node_id: u32,
    pub env_type: String,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone)]
pub struct Tuning {
    pub base_divisor: f64,
    pub min_step: f64,
    pub fine_divisor: f64,
    pub coarse_multiplier: f64,
    pub ramp_hz: f64,
    pub ramp_dwell_ms: u64,
    pub ramp_accel_factor: f64,
    pub ramp_accel_cap: f64,
    /// TK1 C7: duration slot values display in Yellow after a change (ms).
    pub flash_ms: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            base_divisor: 128.0,
            min_step: 0.001,
            fine_divisor: 8.0,
            coarse_multiplier: 4.0,
            ramp_hz: 60.0,
            ramp_dwell_ms: 150,
            ramp_accel_factor: 1.05,
            ramp_accel_cap: 8.0,
            flash_ms: 400,
        }
    }
}

impl Tuning {
    pub fn jog_step(&self, range: f64, held_ms: u64, mag: Mag) -> f64 {
        let base = (range / self.base_divisor).max(self.min_step);
        let step = match mag {
            Mag::Normal => base,
            Mag::Fine => base / self.fine_divisor,
            Mag::Coarse => base * self.coarse_multiplier,
        };
        if held_ms > self.ramp_dwell_ms {
            let n = (held_ms - self.ramp_dwell_ms) as f64 / (1000.0 / self.ramp_hz);
            let mult = self.ramp_accel_factor.powf(n).min(self.ramp_accel_cap);
            step * mult
        } else {
            step
        }
    }
}

pub struct JogTracker {
    pub held_since: Option<std::time::Instant>,
    pub last_tick_ms: u64,
}

impl JogTracker {
    pub fn new() -> Self {
        Self {
            held_since: None,
            last_tick_ms: 0,
        }
    }

    pub fn press(&mut self, now: std::time::Instant, tick_ms: u64) -> u64 {
        self.held_since = Some(now);
        self.last_tick_ms = tick_ms;
        0
    }

    pub fn repeat(&mut self, now: std::time::Instant, tick_ms: u64) -> Option<u64> {
        let held_since = self.held_since?;
        if tick_ms <= self.last_tick_ms + 200 {
            self.last_tick_ms = tick_ms;
            let held = now.duration_since(held_since).as_millis() as u64;
            Some(held)
        } else {
            self.held_since = None;
            self.last_tick_ms = 0;
            None
        }
    }

    pub fn release(&mut self) {
        self.held_since = None;
        self.last_tick_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn jog_step_normal() {
        let t = Tuning::default();
        let step = t.jog_step(1.0, 0, Mag::Normal);
        assert!((step - 1.0 / 128.0).abs() < 0.0001);
    }

    #[test]
    fn jog_step_fine() {
        let t = Tuning::default();
        let step = t.jog_step(1.0, 0, Mag::Fine);
        assert!((step - 1.0 / 128.0 / 8.0).abs() < 0.00001);
    }

    #[test]
    fn jog_step_coarse() {
        let t = Tuning::default();
        let step = t.jog_step(1.0, 0, Mag::Coarse);
        assert!((step - 1.0 / 128.0 * 4.0).abs() < 0.0001);
    }

    #[test]
    fn jog_step_minimum() {
        let t = Tuning::default();
        let step = t.jog_step(0.0001, 0, Mag::Normal);
        assert!((step - 0.001).abs() < 0.0001, "must floor at min_step");
    }

    #[test]
    fn jog_step_ramp_accelerates() {
        let t = Tuning::default();
        let base = t.jog_step(1.0, 0, Mag::Normal);
        let ramped = t.jog_step(1.0, 500, Mag::Normal);
        assert!(ramped > base, "ramp must accelerate over time");
    }

    #[test]
    fn jog_step_ramp_capped() {
        let t = Tuning::default();
        let base = t.jog_step(1.0, 0, Mag::Normal);
        let capped = t.jog_step(1.0, 10000, Mag::Normal);
        let ratio = capped / base;
        assert!(ratio <= 8.0 + 0.01, "ramp must not exceed cap ×8");
    }

    #[test]
    fn jog_tracker_press_sets_held_returns_zero() {
        let mut jt = JogTracker::new();
        let now = Instant::now();
        let held = jt.press(now, 0);
        assert_eq!(held, 0);
    }

    #[test]
    fn jog_tracker_repeat_within_window_returns_duration() {
        let mut jt = JogTracker::new();
        let t0 = Instant::now();
        jt.press(t0, 0);
        let t1 = t0 + Duration::from_millis(100);
        let held = jt.repeat(t1, 100);
        assert!(held.is_some());
        assert!(held.unwrap() >= 90);
    }

    #[test]
    fn jog_tracker_repeat_outside_window_resets() {
        let mut jt = JogTracker::new();
        let t0 = Instant::now();
        jt.press(t0, 0);
        let t1 = t0 + Duration::from_millis(300);
        let held = jt.repeat(t1, 300);
        assert!(held.is_none(), "200ms+ gap must reset tracker");
    }

    #[test]
    fn jog_tracker_release_clears_state() {
        let mut jt = JogTracker::new();
        jt.press(Instant::now(), 0);
        jt.release();
        let t1 = Instant::now() + Duration::from_millis(10);
        let held = jt.repeat(t1, 10);
        assert!(held.is_none(), "release must prevent future repeats");
    }
}
