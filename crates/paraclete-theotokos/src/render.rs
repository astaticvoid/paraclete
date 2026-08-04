use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

use crate::input::PanelButton;
use crate::model::{EnvelopeData, RecMode, Screen, SlotBinding, StepState, KIT_PAGE_SIZE};

const PAGE_SIZE: usize = 8;

/// TK2.1 C2 (D3): "chip casing is display-only" — `key_name`'s lowercase
/// storage form (`"tab"`, `"q"`, `"space"`) title-cases multi-character
/// names for display (`Tab`, `Space`) and leaves single characters as
/// typed (`q`). The keymap file format itself is untouched.
fn chip_key_display(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) if label.chars().count() > 1 => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        Some(first) => first.to_string(),
        None => String::new(),
    }
}

/// TK2.2 C3 (E1): `[key]` for a button with an on-screen referent — empty
/// if the button has no resolvable key right now (unbound/shadowed), same
/// omission rule the legend already applies to its own `Dynamic` chips.
fn inline_chip(data: &RenderData, button: PanelButton) -> String {
    data.legend_key_labels
        .get(&button)
        .map(|k| format!("[{}]", chip_key_display(k)))
        .unwrap_or_default()
}

/// TK2 C5 (D8): one of the 8 encoder bank cells on the Param screen.
#[derive(Clone)]
pub struct EncoderCell {
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// TK2.1 C4 (D10, closes BUG-040): false when `min`/`max` are the
    /// 0..1 last-resort fallback (no cap-doc entry found) — the cell
    /// renders dimmed so the condition is visible.
    pub resolved: bool,
    /// #176 (BUG-067): value-indexed names for a stepped selector, so
    /// `lfo_dest` reads `tune` and `machine` reads `AnalogHiHat` instead of
    /// `1.00` / `2.00`. `None` for a continuous or unlabelled param.
    pub options: Option<Vec<Option<String>>>,
    /// TK3 C0 (#180): how the jog dispatch routes this cell. `Real` for a
    /// live bank param; `VirtualStep` for the TRIG page's per-step params.
    pub target: crate::model::EncoderTarget,
}

impl EncoderCell {
    /// The name as displayed. A `VirtualStep` cell carries a `▸` prefix to
    /// signal "this edits a step, not a live param" (#180) — visual
    /// shorthand that the cell's target is a per-step sequencer param.
    pub fn display_name(&self) -> String {
        if matches!(self.target, crate::model::EncoderTarget::VirtualStep { .. }) {
            format!("▸{}", self.name)
        } else {
            self.name.clone()
        }
    }

    /// What the cell's value should read as (#176).
    ///
    /// A stepped selector whose value has a name reads the name; everything
    /// else keeps the two-decimal numeric formatting. Selecting a destination
    /// or a machine by counting positions is not performable — and a control
    /// that cannot say what it is set to cannot say when it refuses to change.
    pub fn value_text(&self) -> String {
        match crate::model::option_label(self.options.as_deref(), self.value) {
            Some(label) => label.to_string(),
            None => format!("{:.2}", self.value),
        }
    }
}

pub struct RenderData {
    /// TK2 C3 (D12): replaces `Mode`.
    pub screen: Screen,
    /// TK2.1 C1 (D5): replaces `grid_rec: bool` — shown as a three-state
    /// REC indicator (transport bar + status line): `REC○` dark gray
    /// (`Off`), `REC▦` red (`Grid`), `REC●` bright red (`Live`).
    pub rec: RecMode,
    /// TK2 C3 (D6): the armed TRK/PTN hold prefix, if any (status line).
    pub armed_prefix: Option<String>,
    pub active_track: usize,
    /// The engine/cap-doc name per track (e.g. "AnalogKick") — the
    /// contextual header's second half (TK2.1 C0).
    pub track_names: Vec<String>,
    /// The instrument file's display name per track (e.g. "Kick") — the
    /// track line, transport and status line (TK2.1 C0, D2).
    pub display_names: Vec<String>,
    /// TK2.1 C2 (D3): key chip label per trig button (`Trig1..16`,
    /// length 16), resolved through the live `Keymap` — shown on the trig
    /// strip's step cells (bright in `RecMode::Grid`, dimmed otherwise).
    pub trig_key_labels: Vec<Option<String>>,
    /// TK2.1 C2 (D3): the same resolution, truncated to the discovered
    /// track count — shown on the track indicator's pad columns (bright
    /// in `RecMode::Off`/`Live`, absent in `Grid`).
    pub track_key_labels: Vec<Option<String>>,
    /// TK2.1 C2 (D3/D4): key chip labels for the legend's non-literal
    /// entries (`Trk`, `Ptn`, `Rec`, `Play`, `Stop`, `Song`, `Tempo`,
    /// `Settings`, `Yes`, `No`), resolved through the live `Keymap`. A
    /// button absent from the map has no reachable key right now (shadowed
    /// or otherwise unbound) and its legend chip is omitted.
    pub legend_key_labels: HashMap<PanelButton, String>,
    pub bpm: f64,
    pub playing: bool,
    pub page_window: usize,
    pub step_state: StepState,
    pub step_states: Vec<StepState>,
    pub slot_a: Option<SlotBinding>,
    pub slot_a_value: f64,
    pub slot_b: Option<SlotBinding>,
    pub slot_b_value: f64,
    /// TK2 C5 (D13): numpad slot C.
    pub slot_c: Option<SlotBinding>,
    pub slot_c_value: f64,
    pub slot_c_locked: bool,
    pub slot_c_flash: bool,
    pub page_groups: Vec<String>,
    pub perf_page: usize,
    /// TK2 C5 (§0 A11): which 8-wide sub-page of the active page is
    /// showing, and how many exist — the sub-page indicator.
    pub sub_page: usize,
    pub sub_page_count: usize,
    pub envelope: Option<(EnvelopeData, f64)>,
    pub live_env_level: Option<f64>,
    pub live_lfo_phase: Option<f64>,
    pub debug_event: Option<String>,
    /// TK2.1 C5a (D9): explicit encoder-access mode — shown on the status
    /// line.
    pub enc: bool,
    /// TK3 C4 (OQ-T4): the step-size tier (0..=4, ×1..×16) — shown on the
    /// status line next to ENC.
    pub step_size_tier: u8,
    /// TK2.1 C5b (D15): the lock target's step, but only if it's on the
    /// active track — replaces `step_focuses` (the per-track vec was
    /// always read via the active track anyway).
    pub lock_target_step: Option<usize>,
    /// TK2.2 C4 (E5): the param name the last encoder jog moved. Whether
    /// `render_status_line` reads this as "wrote a lock" or "wrote the
    /// live value" is decided from `lock_target_step` at render time, not
    /// stored alongside this name — so the two stay in lockstep.
    pub last_jog_param: Option<String>,
    pub step_locks: Vec<Vec<usize>>,
    /// TK2 C4 (D12): per-track mute state, shown on the track indicator
    /// (`render_track_indicator`) — the dedicated Mute screen this was
    /// originally meant for was retired in TK2.1 C6.
    pub mute_states: Vec<bool>,
    pub slot_a_locked: bool,
    pub slot_b_locked: bool,
    pub cmdline: Option<String>,
    pub cmdline_error: Option<String>,
    /// TK2 C8 (D11): non-error confirmations (`:save-bindings`,
    /// `:list-bindings`, `:load-bindings`) — same echo slot as
    /// `cmdline_error` but styled distinctly so success doesn't read as a
    /// failure (post-C8 hostile review finding).
    pub cmdline_status: Option<String>,
    pub cmdline_candidates: Vec<String>,
    pub slot_a_flash: bool,
    pub slot_b_flash: bool,
    pub help_visible: bool,
    /// TK2 C5 (D8): the active page's params in `Rule` order, up to 8 —
    /// `None` past the page's param count.
    pub encoder_cells: Vec<Option<EncoderCell>>,
    pub encoder_flash: Vec<bool>,
    /// TK2 C6 (D12): whether kitty keyboard-enhancement is active — shown
    /// on the Settings screen.
    pub kitty: bool,
    pub pattern_bank_size: usize,
    /// TK2 C6 (D12): Chain screen state — active/cued pattern (bank-row
    /// markers), how many patterns are queued, the page-loop window, and
    /// which bank slot the cursor points at.
    pub active_pattern: usize,
    pub cued_pattern: Option<usize>,
    pub chain_len: usize,
    pub page_loop: (u8, u8),
    pub chain_cursor: usize,
    /// P11 C6a: KIT screen state — list scroll offset and selected slot.
    pub kit_scroll: usize,
    pub kit_cursor: usize,
    /// P11 C6a: parsed `/context/kits` (`slot, name` pairs, slot order).
    /// Names-only on the bus in P11 — the encoder-bank preview is a
    /// documented placeholder.
    pub kit_list: Vec<(usize, String)>,
    /// P11 C6a: the kit slot bound to the selected track's active pattern
    /// (`/context/kit_binding` + `/node/{seq}/state/active_pattern`) — the
    /// list's `◄ loaded` marker.
    pub kit_loaded_slot: Option<usize>,
    /// P11 C6b: perform mode on (`/context/perform` ≥ 0.5) — the status
    /// bar's `⚡` indicator.
    pub perform_mode: bool,
    /// P11 C6e: per-track `/node/{id}/state/pattern_muted` — marks the
    /// track indicator.
    pub pattern_muted_states: Vec<bool>,
    /// TK3 C2: per-track input gains on the MixNode
    /// (`/node/{mix}/param/input_gain_{i}`), one per track — the MIX
    /// screen's level bars (columns 1..N of the encoder bank map to them).
    pub mix_gains: Vec<f64>,
    /// TK3 C2: the MixNode's `master_gain` (`/node/{mix}/param/master_gain`)
    /// — the MIX screen's bottom row; encoder column 8 maps to it.
    pub mix_master: f64,
}

pub fn render(frame: &mut Frame, data: &RenderData) {
    let area = frame.size();
    // TK2.1 C0 (D1): seven fixed regions — heights never change across
    // screens (`region_heights_are_identical_across_screens`). Only the
    // Min(0) contextual window's content varies by screen.
    let chunks = Layout::vertical([
        Constraint::Length(1), // transport
        Constraint::Min(0),    // contextual window
        Constraint::Length(1), // track indicator
        Constraint::Length(2), // trig strip (selected track only)
        Constraint::Length(2), // legend
        Constraint::Length(1), // echo
        Constraint::Length(1), // status
    ])
    .split(area);

    render_transport(frame, chunks[0], data);
    if data.help_visible {
        render_help(frame, chunks[1], data);
    } else {
        match data.screen {
            Screen::Grid => render_track_context(frame, chunks[1], data),
            Screen::Param(_) => render_perf_window(frame, chunks[1], data),
            Screen::Tempo => render_tempo_screen(frame, chunks[1], data),
            Screen::Settings => render_settings_screen(frame, chunks[1], data),
            Screen::Chain => render_chain_screen(frame, chunks[1], data),
            Screen::Kit => render_kit_screen(frame, chunks[1], data),
            Screen::Mix => render_mix_screen(frame, chunks[1], data),
        }
    }
    render_track_indicator(frame, chunks[2], data);
    render_trig_strip(frame, chunks[3], data);
    render_legend(frame, chunks[4], data);
    render_echo_area(frame, chunks[5], data);
    render_status_line(frame, chunks[6], data);
}

/// TK2 C6 (D12): bpm display; YES-tap and UP/DOWN nudge live in the
/// status line/legend (armed prefix has no meaning here, so the big
/// number is the whole screen).
fn render_tempo_screen(frame: &mut Frame, area: Rect, data: &RenderData) {
    let para = Paragraph::new(format!(" {:.1} BPM", data.bpm))
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    frame.render_widget(para, area);
}

/// TK2 C6 (D12): "shows bpm, kitty status, track/pattern counts, version
/// — read-only in TK2."
fn render_settings_screen(frame: &mut Frame, area: Rect, data: &RenderData) {
    let lines = vec![
        Line::raw(format!(" bpm: {:.1}", data.bpm)),
        Line::raw(format!(
            " kitty keyboard protocol: {}",
            if data.kitty { "yes" } else { "no (sticky fallback)" }
        )),
        Line::raw(format!(" tracks: {}", data.track_names.len())),
        Line::raw(format!(" pattern bank size: {}", data.pattern_bank_size)),
        Line::raw(format!(" version: {}", env!("CARGO_PKG_VERSION"))),
    ];
    let para = Paragraph::new(lines).style(Style::default().fg(Color::White));
    frame.render_widget(para, area);
}

/// TK2 C6 (D12): pattern bank row (active/cued markers, cursor), chain
/// length, page-loop window.
fn render_chain_screen(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

    let mut spans: Vec<Span> = Vec::with_capacity(data.pattern_bank_size);
    for i in 0..data.pattern_bank_size {
        let is_active = i == data.active_pattern;
        let is_cued = data.cued_pattern == Some(i);
        let is_cursor = i == data.chain_cursor;
        let label = format!("P{}", i + 1);
        let text = if is_cursor {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        };
        let color = if is_active {
            Color::Green
        } else if is_cued {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        spans.push(Span::styled(text, Style::default().fg(color)));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);

    let info = format!(
        " Chain: {} pattern(s) queued   Loop: page {}-{}",
        data.chain_len,
        data.page_loop.0 + 1,
        data.page_loop.1 + 1
    );
    frame.render_widget(
        Paragraph::new(info).style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}

/// P11 C6a: the KIT screen — the scrollable kit list (up to 16 slots
/// visible from `kit_scroll`), the loaded-kit marker (`◄ loaded` — the
/// slot bound to the selected track's active pattern), the LOAD/SAVE/
/// COMMIT/RELD bottom-row buttons (trigs 13-16), and the read-only
/// encoder-bank preview (row 2) of the selected kit (`kit_cursor`).
///
/// The bank shows the selected kit's *name* in all 8 cells, standing in
/// for its entries: `/context/kits` publishes names only in P11, so entry
/// (param name + value) data is not on the bus. This is the documented
/// preview limitation from the C6a commit; empty selected slot = `(empty)`.
fn render_kit_screen(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // encoder-bank preview (row 2)
        Constraint::Min(0),    // kit list
        Constraint::Length(1), // [LOAD] [SAVE] [COMMIT] [RELD]
    ])
    .split(area);

    let selected_name = data
        .kit_list
        .iter()
        .find(|(slot, _)| *slot == data.kit_cursor)
        .map(|(_, name)| name.clone());
    let bank_cells: Vec<String> = (0..8)
        .map(|_| selected_name.clone().unwrap_or_else(|| "(empty)".to_string()))
        .collect();
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            format!(" {}", bank_cells.join("  ")),
            Style::default().fg(Color::DarkGray),
        )])),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::with_capacity(KIT_PAGE_SIZE);
    for n in 0..KIT_PAGE_SIZE {
        let slot = data.kit_scroll + n;
        let is_empty = !data.kit_list.iter().any(|(s, _)| *s == slot);
        let name = data
            .kit_list
            .iter()
            .find(|(s, _)| *s == slot)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| "(empty)".to_string());
        let loaded = data.kit_loaded_slot == Some(slot);
        let is_cursor = data.kit_cursor == slot;
        let mut text = format!(" {:>2} {name}", slot + 1);
        if loaded {
            text.push_str("  ◄ loaded");
        }
        let color = if is_cursor {
            Color::White
        } else if loaded {
            Color::Green
        } else if is_empty {
            Color::DarkGray
        } else {
            Color::Gray
        };
        let style = Style::default().fg(color).add_modifier(if is_cursor {
            Modifier::REVERSED
        } else {
            Modifier::empty()
        });
        lines.push(Line::from(Span::styled(text, style)));
    }
    frame.render_widget(Paragraph::new(lines), chunks[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " [LOAD]   [SAVE]   [COMMIT]   [RELD]",
            Style::default().fg(Color::White),
        ))),
        chunks[2],
    );
}

fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::Grid => "GRID",
        Screen::Param(_) => "PARAM",
        Screen::Tempo => "TEMPO",
        Screen::Chain => "CHAIN",
        Screen::Settings => "SETTINGS",
        Screen::Kit => "KIT",
        Screen::Mix => "MIX",
    }
}

/// TK2.1 C2 (D4): one entry in a screen's legend priority list. `Dynamic`
/// resolves its key through the live `Keymap` (`RenderData.legend_key_labels`)
/// and is omitted entirely if that button has no reachable key right now;
/// `Literal` keys are hardcoded, unremappable raw-key checks (`lib.rs`) or
/// key ranges/combinations no single `PanelButton` expresses (`1-6`,
/// `-/=`, `←/→`, `↑/↓`, `FUNC+↑/↓`) — D4 names `:`, `?`, `^C` and the `1-6`
/// range chip explicitly; the other compound range/combo chips are the
/// same kind of thing and are declared the same way here.
enum LegendChip {
    Dynamic(PanelButton, &'static str),
    Literal(&'static str, &'static str),
}

/// TK2.2 C3 (E1): the declared per-screen priority list — truncates from
/// the tail on overflow (`pack_two_lines`), never wraps/scrolls/moves
/// (unchanged from TK2.1 C2/C5). Membership is E1's "residual legend":
/// only affordances with **no on-screen referent** live here. `Trk`/`Ptn`/
/// `Rec`/`Play`/`Stop`/`Tempo` are rendered inline on the panel element
/// they act on instead (`render_transport`/`render_track_indicator`) and
/// so never appear in any list below, including the `enc` override.
/// `Enc`/`Lock` have no referent anywhere on the panel and so appear on
/// **every** screen's own list, Grid included (this closes F8: `[m]` LOCK
/// is a `Grid`-only gesture that used to be advertised only off `Grid`).
fn legend_chips_for_screen(screen: Screen, enc: bool) -> Vec<LegendChip> {
    use LegendChip::{Dynamic, Literal};
    use PanelButton::{Enc, Lock, No, Settings, Song, Yes};

    if enc {
        return vec![
            Literal("trigs", "ENCODER ±"),
            Literal("Ctrl", "FINE"),
            Literal("FUNC", "COARSE"),
            Dynamic(Enc, "ENC off"),
            Dynamic(Lock, "LOCK"),
            Dynamic(No, "BACK"),
            Literal(":", "CMD"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ];
    }

    match screen {
        Screen::Grid => vec![
            Dynamic(Enc, "ENC"),
            Dynamic(Lock, "LOCK"),
            Literal("1-6", "PAGE"),
            Literal("-/=", "WIN"),
            Dynamic(Song, "SONG"),
            Dynamic(Settings, "SET"),
            Dynamic(Yes, "YES"),
            Dynamic(No, "NO"),
            Literal(":", "CMD"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
        Screen::Param(_) => vec![
            Dynamic(Enc, "ENC"),
            Dynamic(Lock, "LOCK"),
            Literal("1-6", "PAGE"),
            Dynamic(No, "BACK"),
            Literal(":", "CMD"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
        Screen::Chain => vec![
            Dynamic(Yes, "PUSH"),
            Dynamic(No, "CLEAR"),
            Literal("←/→", "CURSOR"),
            Dynamic(Song, "SONG"),
            Literal(":", "CMD"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
        Screen::Tempo => vec![
            Dynamic(Yes, "TAP"),
            Literal("↑/↓", "±1"),
            Literal("FUNC+↑/↓", "±0.1"),
            Dynamic(No, "BACK"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
        Screen::Settings => vec![
            Dynamic(No, "BACK"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
        Screen::Kit => vec![
            Dynamic(No, "BACK"),
            Literal("FUNC", "PERF"),
            Literal("FUNC+trig", "SCROLL"),
            Literal("trig13-16", "LOAD/SAVE/CMIT/RLD"),
            Literal(":", "CMD"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
        Screen::Mix => vec![
            Dynamic(Enc, "ENC"),
            Dynamic(No, "BACK"),
            Literal("FUNC+8", "RE-OPEN"),
            Literal("trig N", "TRACK LEVEL"),
            Literal("?", "HELP"),
            Literal("^C", "QUIT"),
        ],
    }
}

/// TK3 C2: the MIX screen — one row per track with a block-element level
/// bar and the gain value, then a MASTER row. The encoder bank's columns
/// 1..N map to those track rows (column 8 = master); the jog dispatch in
/// lib.rs routes them to the MixNode's `input_gain_{n}` / `master_gain`.
fn render_mix_screen(frame: &mut Frame, area: Rect, data: &RenderData) {
    let mut lines: Vec<Line> = Vec::new();
    for (i, name) in data.display_names.iter().enumerate() {
        let gain = data.mix_gains.get(i).copied().unwrap_or(0.0);
        lines.push(level_line(name, gain, Color::Cyan));
    }
    lines.push(level_line("MASTER", data.mix_master, Color::Yellow));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(Color::White)),
        area,
    );
}

/// One MIX row: ` name  ████░░░░ 1.20` — a block bar scaled over the
/// MixNode's 0.0–2.0 gain range and the two-decimal value.
fn level_line(name: &str, value: f64, color: Color) -> Line<'static> {
    let ratio = (value / 2.0).clamp(0.0, 1.0);
    let filled = (ratio * 8.0).round() as usize;
    let bar = "█".repeat(filled) + &"░".repeat(8 - filled);
    Line::from(vec![
        Span::styled(format!(" {:>7} ", name), Style::default().fg(Color::White)),
        Span::styled(format!("{bar} "), Style::default().fg(color)),
        Span::raw(format!("{:.2}", value)),
    ])
}

/// TK2.1 C2 (D4): greedily fills line 0, then line 1, from an ordered chip
/// list — the first chip that doesn't fit anywhere marks the truncation
/// point; everything after it is dropped (never wraps to a third line).
/// Returns indices into `chip_texts` per line, so the caller can re-split
/// each chip into its bright-key/dim-label spans for rendering.
fn pack_two_lines(chip_texts: &[String], width: usize) -> (Vec<usize>, Vec<usize>) {
    let mut line0 = Vec::new();
    let mut line1 = Vec::new();
    let mut on_line1 = false;
    let mut len0 = 0usize;
    let mut len1 = 0usize;
    for (i, text) in chip_texts.iter().enumerate() {
        let w = text.chars().count();
        if !on_line1 {
            let sep = if line0.is_empty() { 0 } else { 2 };
            if len0 + sep + w <= width {
                len0 += sep + w;
                line0.push(i);
                continue;
            }
            on_line1 = true;
        }
        let sep = if line1.is_empty() { 0 } else { 2 };
        if len1 + sep + w <= width {
            len1 += sep + w;
            line1.push(i);
        } else {
            break;
        }
    }
    (line0, line1)
}

/// Key legend, always on screen (not gated behind `?`). TK1 C8 usability
/// finding: current keys must stay visible while learning the layout — a
/// toggle-only overlay hides the grid you're trying to use the keys on.
/// TK2.1 C2 (D4): rewritten as `[key] NAME` chips (bright key, dim label)
/// from the declared per-screen priority list, replacing the grey run-on
/// hint line.
fn render_legend(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chips: Vec<(String, &'static str)> = legend_chips_for_screen(data.screen, data.enc)
        .into_iter()
        .filter_map(|chip| match chip {
            LegendChip::Dynamic(button, label) => data
                .legend_key_labels
                .get(&button)
                .map(|k| (format!("[{}]", chip_key_display(k)), label)),
            LegendChip::Literal(key, label) => Some((format!("[{key}]"), label)),
        })
        .collect();

    let chip_texts: Vec<String> = chips
        .iter()
        .map(|(key, label)| format!("{key} {label}"))
        .collect();
    let (line0_idx, line1_idx) = pack_two_lines(&chip_texts, area.width as usize);

    let render_line = |idxs: &[usize]| -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(idxs.len() * 3);
        for (n, &i) in idxs.iter().enumerate() {
            if n > 0 {
                spans.push(Span::raw("  "));
            }
            let (key, label) = &chips[i];
            spans.push(Span::styled(key.clone(), Style::default().fg(Color::White)));
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    };

    let lines = vec![render_line(&line0_idx), render_line(&line1_idx)];
    let para = Paragraph::new(lines).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

/// TK2.1 C1 (D5): the three-state REC glyph/colour pair, shared by the
/// transport bar and the status line.
fn rec_indicator(rec: RecMode) -> (&'static str, Color) {
    match rec {
        RecMode::Off => ("REC○", Color::DarkGray),
        RecMode::Grid => ("REC▦", Color::Red),
        RecMode::Live => ("REC●", Color::LightRed),
    }
}

fn render_transport(frame: &mut Frame, area: Rect, data: &RenderData) {
    let play_sym = if data.playing { "▶" } else { "■" };
    let (rec_sym, rec_color) = rec_indicator(data.rec);
    let track_name = data
        .display_names
        .get(data.active_track)
        .map(|s| s.as_str())
        .unwrap_or("?");
    let page = data.page_window + 1;
    let page_count = data.step_state.page_count.max(1);

    // TK2.2 C3 (E1): [z]/[x]/[0] move onto the glyph/readout they act on
    // instead of the legend strip — the glyph/readout IS the referent.
    // TK2.2 C5: [c] STOP rejoins them now that ADR-046 gives it a meaning
    // (halt + rewind) — it sits directly on the play/pause glyph, the same
    // element it and [x] both act on.
    let tempo_chip = inline_chip(data, PanelButton::Tempo);
    let play_chip = inline_chip(data, PanelButton::Play);
    let stop_chip = inline_chip(data, PanelButton::Stop);
    let rec_chip = inline_chip(data, PanelButton::Rec);

    let prefix = format!(
        " {tempo_chip}{:.1} BPM  {play_chip}{}{stop_chip}  ",
        data.bpm, play_sym
    );
    let suffix = format!(
        "  {}  P{}/{}  Step:{}  Len:{}",
        track_name,
        page,
        page_count,
        data.step_state.current_step + 1,
        data.step_state.pattern_length,
    );

    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::White)),
        Span::styled(rec_chip, Style::default().fg(Color::White)),
        Span::styled(rec_sym, Style::default().fg(rec_color)),
        Span::styled(suffix, Style::default().fg(Color::White)),
    ]);
    let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

/// TK2.1 C0 (D2): one line per track — selection marker, `N Name`, mute
/// marker, and (right-aligned by trailing spans) the pattern indicator.
/// Below **60** columns *(tunable, §3 minimum width)* names drop, keeping
/// just the selection marker and track number. When the full list doesn't
/// fit, ADR-044 D2 requires windowing around the selected track with
/// `‹`/`›` markers rather than truncating silently.
fn render_track_indicator(frame: &mut Frame, area: Rect, data: &RenderData) {
    let narrow = area.width < 60;
    // TK2.2 C3 (E1): [Tab] TRK moves onto the track line's own head — the
    // track line IS the track selector — and [p] PTN onto the pattern
    // field it names. Both are unconditional: TRK-hold and PTN-hold
    // select silently in every mode, not just pad modes.
    let trk_chip = inline_chip(data, PanelButton::Trk);
    let ptn_chip = inline_chip(data, PanelButton::Ptn);
    let ptn_text = format!("{ptn_chip}PTN P{}", data.active_pattern + 1);
    let entries: Vec<(String, Color)> = data
        .display_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == data.active_track { "▸" } else { " " };
            let muted = data.mute_states.get(i).copied().unwrap_or(false);
            let mute_glyph = if muted { "●" } else { "" };
            // P11 C6e: the per-pattern mute gets its own marker (`◌`) —
            // distinct from the global bank mute's `●` so the two cannot
            // be confused.
            let pattern_muted = data.pattern_muted_states.get(i).copied().unwrap_or(false);
            let pattern_mute_glyph = if pattern_muted { "◌" } else { "" };
            let color = if i == data.active_track {
                Color::White
            } else {
                Color::Gray
            };
            // D3: the track-line chip appears only in pad modes (Off/Live)
            // — a bare trig doesn't select a track in Grid mode. TK2.2 C3
            // (E2): a hidden chip still reserves its width — the same
            // label the pad mode would render, blanked — so no column
            // shifts when RecMode changes (closes F3).
            let chip_would_be = data
                .track_key_labels
                .get(i)
                .and_then(|o| o.as_deref())
                .map(|k| format!("[{}]", chip_key_display(k)))
                .unwrap_or_default();
            let chip = if matches!(data.rec, RecMode::Off | RecMode::Live) {
                chip_would_be
            } else {
                " ".repeat(chip_would_be.chars().count())
            };
            let text = if narrow {
                format!("{marker}{chip}{}{mute_glyph}{pattern_mute_glyph}  ", i + 1)
            } else {
                format!(
                    "{marker}{chip}{} {}{mute_glyph}{pattern_mute_glyph}  ",
                    i + 1,
                    name
                )
            };
            (text, color)
        })
        .collect();

    let mut spans: Vec<Span> = Vec::with_capacity(entries.len() * 2 + 3);
    spans.push(Span::styled(
        trk_chip.clone(),
        Style::default().fg(Color::White),
    ));
    if entries.is_empty() {
        spans.push(Span::styled(ptn_text, Style::default().fg(Color::DarkGray)));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    let entries_width: usize = entries.iter().map(|(t, _)| t.chars().count()).sum();
    let budget = (area.width as usize)
        .saturating_sub(trk_chip.chars().count())
        .saturating_sub(ptn_text.chars().count());
    let active = data.active_track.min(entries.len() - 1);

    if entries_width <= budget {
        for (text, color) in &entries {
            spans.push(Span::styled(text.clone(), Style::default().fg(*color)));
        }
    } else {
        // ADR-044 D2: grow a window outward from the selected track (right
        // first, matching reading order) until the budget — minus room for
        // the `‹`/`›` markers — is spent.
        let marker_budget = budget.saturating_sub(4);
        let mut start = active;
        let mut end = active;
        let mut width = entries[active].0.chars().count();
        loop {
            let can_grow_left = start > 0;
            let can_grow_right = end + 1 < entries.len();
            let right_w = if can_grow_right {
                entries[end + 1].0.chars().count()
            } else {
                usize::MAX
            };
            let left_w = if can_grow_left {
                entries[start - 1].0.chars().count()
            } else {
                usize::MAX
            };
            if can_grow_right && width + right_w <= marker_budget {
                end += 1;
                width += right_w;
            } else if can_grow_left && width + left_w <= marker_budget {
                start -= 1;
                width += left_w;
            } else {
                break;
            }
        }
        if start > 0 {
            spans.push(Span::styled("‹ ", Style::default().fg(Color::DarkGray)));
        }
        for (text, color) in &entries[start..=end] {
            spans.push(Span::styled(text.clone(), Style::default().fg(*color)));
        }
        if end + 1 < entries.len() {
            spans.push(Span::styled("›", Style::default().fg(Color::DarkGray)));
        }
    }
    spans.push(Span::styled(ptn_text, Style::default().fg(Color::DarkGray)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// TK2.1 C0 (D2/D3): the persistent trig strip — **only** the selected
/// track, two rows of 8 cells, rendered on every screen. Below **48**
/// columns *(tunable, §3 minimum width)* the row labels (` 1-8`/`9-16`)
/// drop.
fn render_trig_strip(frame: &mut Frame, area: Rect, data: &RenderData) {
    let track = data.active_track;
    let focus = data.lock_target_step;
    let locks: std::collections::HashSet<usize> = data
        .step_locks
        .get(track)
        .map(|v| v.iter().copied().collect())
        .unwrap_or_default();
    let show_label = area.width >= 48;
    let lines = vec![
        render_trig_row(track, data, 0, " 1-8", show_label, focus, &locks),
        render_trig_row(track, data, PAGE_SIZE, "9-16", show_label, focus, &locks),
    ];
    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(Color::Gray)),
    );
    frame.render_widget(para, area);
}

/// One trig-strip row. §3's exact cell format: `[k]g` (4 cols: bracket,
/// chip, bracket, glyph) joined by one space. TK2.1 C2 (D3): `k` is the
/// trig's key chip, always shown — bright in `RecMode::Grid` (the trig
/// really does write that step right now), dimmed otherwise (display-only:
/// pad modes address tracks, not steps, via this same key). State glyphs
/// keep the TK2 colour/state rules (playhead yellow, active+locked green,
/// active cyan, locked white, empty dark gray); focus/playhead are carried
/// by `Modifier::REVERSED` since a single-column glyph leaves no room for
/// a wider block.
fn render_trig_row<'a>(
    track_idx: usize,
    data: &'a RenderData,
    row_off: usize,
    label: &str,
    show_label: bool,
    focus: Option<usize>,
    locks: &std::collections::HashSet<usize>,
) -> Line<'a> {
    let st = data.step_states.get(track_idx).unwrap_or(&data.step_state);
    let window = data.page_window * PAGE_SIZE * 2 + row_off;
    let mut spans: Vec<Span> = Vec::with_capacity(PAGE_SIZE * 4);

    if show_label {
        spans.push(Span::styled(
            format!("{:>4} ", label),
            Style::default().fg(Color::Gray),
        ));
    }

    for col in 0..PAGE_SIZE {
        let step = window + col;
        let is_active = st.steps.get(step).copied().unwrap_or(false);
        let is_locked = locks.contains(&step);
        let is_playhead = step == st.current_step;
        let focused = focus == Some(step);

        let glyph = if is_active { "▓" } else { "░" };
        let (color, modifier) = if focused || is_playhead {
            (Color::Yellow, Modifier::REVERSED)
        } else if is_active && is_locked {
            (Color::Green, Modifier::empty())
        } else if is_active {
            (Color::Cyan, Modifier::empty())
        } else if is_locked {
            (Color::White, Modifier::empty())
        } else {
            (Color::DarkGray, Modifier::empty())
        };

        let key_index = row_off + col;
        let chip = data
            .trig_key_labels
            .get(key_index)
            .and_then(|o| o.as_deref())
            .map(chip_key_display)
            .unwrap_or_else(|| " ".to_string());
        let chip_color = if data.rec == RecMode::Grid {
            Color::White
        } else {
            Color::DarkGray
        };

        spans.push(Span::styled("[", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(chip, Style::default().fg(chip_color)));
        spans.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            glyph,
            Style::default().fg(color).add_modifier(modifier),
        ));
        if col + 1 < PAGE_SIZE {
            spans.push(Span::raw(" "));
        }
    }

    Line::from(spans)
}

/// TK2.1 C0: the contextual window for `Screen::Grid` — header
/// `{display_name} — {engine_name}`,
/// then whatever occupies the active page's **first 4 encoder columns**
/// *(tunable)* as name/value/bar (reusing the already-resolved
/// `encoder_cells`), then the existing envelope section.
///
/// Column-scoped, not "the first 4 params": since MM-C1 a page can have
/// declared gaps, so a sparse page shows fewer than 4 entries here. That is
/// deliberate — the strip and the encoder bank stay on one convention, and
/// reintroducing rank-ordering here is what MM-C1 removed everywhere else.
///
/// A track with no page params (no composite view, no `Rule` pagination —
/// `model.rs` `resolve_encoder_params` returns an all-empty bank) renders
/// the header plus a placeholder line rather than an empty pane.
fn render_track_context(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    let display_name = data
        .display_names
        .get(data.active_track)
        .map(|s| s.as_str())
        .unwrap_or("?");
    let engine_name = data
        .track_names
        .get(data.active_track)
        .map(|s| s.as_str())
        .unwrap_or("?");
    let header = Paragraph::new(format!(" {display_name} — {engine_name}"))
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
    frame.render_widget(header, chunks[0]);

    let params: Vec<&EncoderCell> = data.encoder_cells.iter().take(4).flatten().collect();
    if params.is_empty() {
        let placeholder = Paragraph::new("   no page params")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(placeholder, chunks[1]);
    } else {
        let mut spans: Vec<Span> = Vec::with_capacity(params.len());
        for cell in params {
            let ratio = ((cell.value - cell.min) / (cell.max - cell.min).max(0.001)).clamp(0.0, 1.0);
            let filled = (ratio * 4.0).round() as usize;
            let bar = "▓".repeat(filled) + &"░".repeat(4 - filled);
            spans.push(Span::raw(format!(
                "  {} {} {bar}",
                cell.display_name(),
                cell.value_text()
            )));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), chunks[1]);
    }

    render_envelope_section(frame, chunks[2], data);
}

fn render_perf_window(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);

    render_page_tabs(frame, chunks[0], data);
    render_encoder_bank(frame, chunks[1], data);
    render_envelope_section(frame, chunks[2], data);
    if data.live_lfo_phase.is_some() {
        render_lfo_phase_indicator(frame, chunks[3], data);
    }
}

/// TK2 C5 (D8): 8 encoder cells, 2×4, name + bar + value. A param page
/// with fewer than 8 params shows blank cells past its count rather than
/// a malformed one. D13's arrow-cursor navigation (BUG-038) was formally
/// descoped in TK2.1 C7 — D9's ENC mode gives every encoder a direct
/// physical (key) address, so there is nothing left for a cursor to
/// navigate between.
fn render_encoder_bank(frame: &mut Frame, area: Rect, data: &RenderData) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    for (row, row_area) in rows.iter().enumerate() {
        let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(*row_area);
        for (col, cell_area) in cols.iter().enumerate() {
            let idx = row * 4 + col;
            render_encoder_cell(frame, *cell_area, data, idx);
        }
    }
}

fn render_encoder_cell(frame: &mut Frame, area: Rect, data: &RenderData, idx: usize) {
    let cell = data.encoder_cells.get(idx).and_then(|c| c.as_ref());
    let line = match cell {
        Some(c) => {
            let ratio = ((c.value - c.min) / (c.max - c.min).max(0.001)).clamp(0.0, 1.0);
            let filled = (ratio * 4.0).round() as usize;
            let bar = "█".repeat(filled) + &"░".repeat(4 - filled);
            let flash = data.encoder_flash.get(idx).copied().unwrap_or(false);
            // TK2.1 C4 (D10, closes BUG-040): an unresolved cell (no
            // cap-doc entry found, min/max are the 0..1 fallback) renders
            // dimmed so the condition is visible rather than looking like
            // an ordinary, trustworthy 0..1 param.
            let color = if !c.resolved {
                Color::DarkGray
            } else if flash {
                Color::Yellow
            } else {
                Color::White
            };
            Line::styled(
                format!("{} {} {}", c.display_name(), bar, c.value_text()),
                Style::default().fg(color),
            )
        }
        None => Line::styled("--", Style::default().fg(Color::DarkGray)),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_lfo_phase_indicator(frame: &mut Frame, area: Rect, data: &RenderData) {
    let phase = data.live_lfo_phase.expect("caller must guard with is_some()");
    let pos = (phase.clamp(0.0, 1.0) * 10.0).round() as usize;
    let mut line = String::with_capacity(13);
    line.push_str(" LFO ");
    for i in 0..=10 {
        if i == pos {
            line.push('●');
        } else {
            line.push('─');
        }
    }
    let para = Paragraph::new(line).style(Style::default().fg(Color::Magenta));
    frame.render_widget(para, area);
}

fn render_page_tabs(frame: &mut Frame, area: Rect, data: &RenderData) {
    let tabs: Vec<String> = data
        .page_groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            if i == data.perf_page {
                format!("[{}]", g)
            } else {
                format!(" {} ", g)
            }
        })
        .collect();
    let mut line = tabs.join("  ");
    // TK2 C5 (§0 A11): a page over 8 params splits into sub-pages instead
    // of truncating — shown only when there's more than one to indicate.
    if data.sub_page_count > 1 {
        line.push_str(&format!(
            "   ¶{}/{}",
            data.sub_page + 1,
            data.sub_page_count
        ));
    }
    let para = Paragraph::new(line).style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}

fn render_envelope_section(frame: &mut Frame, area: Rect, data: &RenderData) {
    if let Some((ref env, static_val)) = &data.envelope {
        let chunks = Layout::horizontal([Constraint::Length(14), Constraint::Min(0)]).split(area);

        let (label_text, display_val, color) = if let Some(lv) = data.live_env_level {
            (
                format!(" {} ▶ ", env.param_name),
                lv,
                Color::Green,
            )
        } else {
            (
                format!(" {} ", env.param_name),
                *static_val,
                Color::Cyan,
            )
        };

        let label_span = Span::styled(label_text, Style::default().fg(color));
        let label = Paragraph::new(label_span);
        frame.render_widget(label, chunks[0]);

        let ratio =
            ((display_val - env.min) / (env.max - env.min).max(0.001)).clamp(0.0, 1.0);
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::NONE))
            .gauge_style(Style::default().fg(color))
            .ratio(ratio);
        frame.render_widget(gauge, chunks[1]);
    }
}

/// TK2 C3: regenerated from the §2 panel table (`design/phases/tk2-theotokos.md`)
/// — the button vocabulary, not TK1's mode-scoped key list.
fn render_help(frame: &mut Frame, area: Rect, data: &RenderData) {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::styled(
        format!(" SCREEN: {}  (? = close)", screen_name(data.screen)),
        Style::default().fg(Color::Yellow),
    ));
    lines.push(Line::from(""));

    lines.push(Line::styled(
        "── PANEL ──",
        Style::default().fg(Color::Cyan),
    ));
    for (key, desc) in &[
        ("q w e r t y u i", "Trig1-8 (top row)"),
        ("a s d f g h j k", "Trig9-16 (bottom row)"),
        ("trig (REC off/Live)", "play + select that track"),
        ("trig (REC Grid)", "write/clear a step"),
        ("Tab (hold)", "TRK — + trig: select track silently"),
        ("p (hold)", "PTN — + trig: select pattern"),
        ("z", "REC — Off<->Grid toggle (kitty); no-kitty: Grid if stopped, Live if playing"),
        ("z (hold)+x, kitty", "REC+PLAY — escalate to Live (record) + start transport"),
        ("x / c", "PLAY / STOP (Space = PLAY)"),
        ("FUNC (Shift)", "coarse jog (ENC on) / secondary chords"),
        ("n", "ENC — toggle encoder-jog mode (bare trig jogs, any screen)"),
        ("FUNC+trig", "encoder jog, ENC off (top row up, bottom row down)"),
        ("m(hold)+trig, Grid", "LOCK — arm the trig's step as the p-lock target"),
        ("1-6", "page select (TRIG SRC FLTR AMP FX MOD)"),
        ("7 / 8 / 9 / 0", "KIT / SETTINGS / SAMPLING / TEMPO"),
        ("Enter / Esc", "YES / NO (Esc also clears a set p-lock target)"),
        ("arrows", "navigation"),
        ("- / =", "step-page window prev / next"),
        ("o", "SONG (opens Chain)"),
        ("v", "KEYBD (reserved)"),
    ] {
        // Widened to 20 (from 16): "trig (REC off/Live)" and a couple of
        // other TK2.1 entries run longer than the old single-key/short-chord
        // labels this used to size for (post-C7 hostile review finding —
        // column misalignment from the longer new labels).
        lines.push(Line::styled(
            format!("  {:20}  {}", key, desc),
            Style::default().fg(Color::White),
        ));
    }
    lines.push(Line::from(""));

    lines.push(Line::styled(
        "── UNBOUND / FIXED ──",
        Style::default().fg(Color::Cyan),
    ));
    for (key, desc) in &[
        (": (or Shift+;)", "open command line"),
        ("Backspace", "clear locks on focused step"),
        ("?", "toggle help"),
        ("Ctrl-C", "quit"),
    ] {
        lines.push(Line::styled(
            format!("  {:16}  {}", key, desc),
            Style::default().fg(Color::White),
        ));
    }

    lines.push(Line::from(""));
    lines.push(Line::styled(
        "── COMMAND LINE ──",
        Style::default().fg(Color::Cyan),
    ));
    for (verb, desc) in &[
        ("set <p> <v>", "set param to value"),
        ("bpm <n>", "set tempo (20-300)"),
        ("track <n>", "select track"),
        ("pattern <n>", "select pattern"),
        ("mute <n>", "mute track"),
        ("unmute <n>", "unmute track"),
        ("clear", "clear current pattern"),
        ("lock-clear", "clear locks on focused step"),
        ("bind <key> <button>", "remap a key (D11)"),
        ("unbind <key>", "remove a user binding"),
        ("list-bindings", "show active user bindings"),
        ("reset-bindings", "clear all user bindings"),
        ("save-bindings", "write bindings to disk"),
        ("load-bindings", "reload bindings from disk"),
    ] {
        lines.push(Line::styled(
            format!("  :{:12}  {}", verb, desc),
            Style::default().fg(Color::White),
        ));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::NONE))
        .scroll((0, 0));
    frame.render_widget(para, area);
}

fn render_echo_area(frame: &mut Frame, area: Rect, data: &RenderData) {
    if let Some(ref err) = data.cmdline_error {
        let err_span = Span::styled(format!(" {} ", err), Style::default().fg(Color::Red));
        let para = Paragraph::new(err_span);
        frame.render_widget(para, area);
        return;
    }
    // TK2 C8: success confirmations (`:save-bindings`, `:list-bindings`,
    // `:load-bindings`, ...) share the echo slot but not the red styling —
    // green reads as "done", not "failed" (post-C8 hostile review).
    if let Some(ref status) = data.cmdline_status {
        let status_span = Span::styled(format!(" {} ", status), Style::default().fg(Color::Green));
        let para = Paragraph::new(status_span);
        frame.render_widget(para, area);
        return;
    }
    let text = match &data.cmdline {
        Some(t) => {
            let candidates = if data.cmdline_candidates.is_empty() {
                String::new()
            } else {
                format!("  ─ {}", data.cmdline_candidates.join("  "))
            };
            format!(" :{} {}", t, candidates)
        }
        None => String::new(),
    };
    let para = Paragraph::new(text).style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}

/// TK2 C3 (D12): replaces the TK1 mode line. Shows the current screen,
/// active track, REC ●/○, and the armed TRK/PTN prefix (if any) — encoder
/// bindings join this once the encoder bank exists (TK2 C5).
fn render_status_line(frame: &mut Frame, area: Rect, data: &RenderData) {
    let screen_style = Style::default().fg(Color::Yellow);

    let mut spans = vec![
        Span::styled(format!(" {:8} ", screen_name(data.screen)), screen_style),
        Span::raw(" "),
        Span::raw(
            data.display_names
                .get(data.active_track)
                .map(|s| s.as_str())
                .unwrap_or("?"),
        ),
        Span::raw(" "),
        {
            let (rec_sym, rec_color) = rec_indicator(data.rec);
            Span::styled(format!("{rec_sym} "), Style::default().fg(rec_color))
        },
    ];

    // P11 C6b: perform-mode indicator — `⚡` while `/context/perform` is
    // on (≥ 0.5). FUNC+KIT toggles it.
    if data.perform_mode {
        spans.push(Span::styled("⚡ ", Style::default().fg(Color::Yellow)));
    }

    // TK2.1 C5b: "L:" (Lock), not "F:" (Focus) — the status line is
    // showing the lock target now, not the retired step_focus concept.
    if let Some(sf) = data.lock_target_step {
        spans.push(Span::raw(format!("L:s{} ", sf)));
    }

    // TK2.2 C4 (E5): "a jog says what it is writing" — names the last
    // jogged param and, read from the *current* lock target (not a
    // frozen copy from jog time), whether it wrote a lock or the live
    // value. This is why the locked-step naming clears the instant the
    // target itself clears, in lockstep, rather than on the next jog.
    if let Some(ref name) = data.last_jog_param {
        let text = match data.lock_target_step {
            Some(step) => format!("J:{name}→s{step} "),
            None => format!("J:{name}→live "),
        };
        spans.push(Span::styled(text, Style::default().fg(Color::Magenta)));
    }

    // TK2.1 C5a (D9): status line shows ENC when on; TK3 C4 appends the
    // step-size tier (×1..×16) alongside it.
    if data.enc {
        let mult = (2i32).pow(data.step_size_tier as u32);
        spans.push(Span::styled(
            format!(" ENC×{mult} "),
            Style::default().fg(Color::Cyan),
        ));
    }

    if let Some(ref prefix) = data.armed_prefix {
        spans.push(Span::styled(
            format!("{} ", prefix),
            Style::default().fg(Color::Cyan),
        ));
    }

    match data.screen {
        Screen::Grid => {
            let page_info = format!(
                "P{}/{}",
                data.page_window + 1,
                data.step_state.page_count.max(1)
            );
            spans.push(Span::raw(page_info));
        }
        Screen::Param(_) => {
            let a_lock = if data.slot_a_locked { "L" } else { "" };
            let a_color = if data.slot_a_flash {
                Color::Yellow
            } else {
                Color::White
            };
            let a_text = match &data.slot_a {
                Some(s) => format!(" A:{}={:.3}{}", s.param_name, data.slot_a_value, a_lock),
                None => " A:--".to_string(),
            };
            spans.push(Span::styled(a_text, Style::default().fg(a_color)));
            spans.push(Span::raw(" "));
            let b_color = if data.slot_b_flash {
                Color::Yellow
            } else {
                Color::White
            };
            let b_lock = if data.slot_b_locked { "L" } else { "" };
            let b_text = match &data.slot_b {
                Some(s) => format!("B:{}={:.3}{}", s.param_name, data.slot_b_value, b_lock),
                None => "B:--".to_string(),
            };
            spans.push(Span::styled(b_text, Style::default().fg(b_color)));
            spans.push(Span::raw(" "));
            // TK2 C5 (D13): slot C, extending A/B.
            let c_color = if data.slot_c_flash {
                Color::Yellow
            } else {
                Color::White
            };
            let c_lock = if data.slot_c_locked { "L" } else { "" };
            let c_text = match &data.slot_c {
                Some(s) => format!("C:{}={:.3}{}", s.param_name, data.slot_c_value, c_lock),
                None => "C:--".to_string(),
            };
            spans.push(Span::styled(c_text, Style::default().fg(c_color)));
        }
        // TK2 C6 builds these screens; until then, no stale slot A/B info
        // next to the "not yet implemented" placeholder (review finding,
        // post-C3 hostile review).
        Screen::Tempo | Screen::Chain | Screen::Settings | Screen::Kit | Screen::Mix => {}
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

impl RenderData {
    pub fn for_test(screen: Screen, track_count: u8) -> Self {
        let track_count = track_count.max(1) as usize;
        let keymap = crate::input::Keymap::default();
        let trig_key_labels: Vec<Option<String>> = (0..16)
            .map(|i| crate::input::trig_button(i).and_then(|b| crate::input::key_label(&keymap, b)))
            .collect();
        // Mirror lib.rs's real clamp: only 16 physical trig keys exist, so
        // track_key_labels can never be longer than trig_key_labels.
        let track_key_labels: Vec<Option<String>> =
            trig_key_labels[..track_count.min(16)].to_vec();
        let legend_key_labels: HashMap<PanelButton, String> = [
            PanelButton::Trk,
            PanelButton::Ptn,
            PanelButton::Rec,
            PanelButton::Play,
            PanelButton::Stop,
            PanelButton::Song,
            PanelButton::Tempo,
            PanelButton::Settings,
            PanelButton::Yes,
            PanelButton::No,
            PanelButton::Enc,
            PanelButton::Lock,
        ]
        .into_iter()
        .filter_map(|b| crate::input::key_label(&keymap, b).map(|k| (b, k)))
        .collect();
        Self {
            screen,
            rec: RecMode::Off,
            armed_prefix: None,
            active_track: 0,
            track_names: (1..=track_count).map(|i| format!("T{}", i)).collect(),
            display_names: (1..=track_count).map(|i| format!("T{}", i)).collect(),
            trig_key_labels,
            track_key_labels,
            legend_key_labels,
            bpm: 120.0,
            playing: false,
            page_window: 0,
            step_state: StepState::default(),
            step_states: vec![],
            slot_a: None,
            slot_a_value: 0.0,
            slot_b: None,
            slot_b_value: 0.0,
            page_groups: vec![],
            perf_page: 0,
            envelope: None,
            live_env_level: None,
            live_lfo_phase: None,
            debug_event: None,
            enc: false,
            lock_target_step: None,
            last_jog_param: None,
            step_locks: vec![vec![]; track_count],
            mute_states: vec![false; track_count],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_status: None,
            cmdline_candidates: vec![],
            slot_a_flash: false,
            slot_c: None,
            slot_c_value: 0.0,
            slot_c_locked: false,
            slot_c_flash: false,
            slot_b_flash: false,
            sub_page: 0,
            sub_page_count: 1,
            encoder_cells: vec![None; 8],
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
            kit_scroll: 0,
            kit_cursor: 0,
            kit_list: vec![],
            kit_loaded_slot: None,
            perform_mode: false,
            pattern_muted_states: vec![false; track_count],
            mix_gains: vec![0.0; track_count],
            mix_master: 0.0,
            step_size_tier: 0,
            help_visible: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StepState;

    #[test]
    fn render_seq_does_not_panic() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData {
            screen: Screen::Grid,
            rec: RecMode::Off,
            armed_prefix: None,
            active_track: 0,
            track_names: vec!["AnalogKick".into(), "AnalogSnare".into()],
            display_names: vec!["Kick".into(), "Snare".into()],
            trig_key_labels: vec![None; 16],
            track_key_labels: vec![None; 2],
            legend_key_labels: HashMap::new(),
            bpm: 140.0,
            playing: true,
            page_window: 0,
            step_state: StepState {
                current_step: 3,
                pattern_length: 16,
                steps: vec![true; 16],
                page_count: 1,
            },
            step_states: vec![],
            slot_a: None,
            slot_a_value: 0.0,
            slot_b: None,
            slot_b_value: 0.0,
            page_groups: vec![],
            perf_page: 0,
            envelope: None,
            live_env_level: None,
            live_lfo_phase: None,
            debug_event: None,
            enc: false,
            lock_target_step: None,
            last_jog_param: None,
            step_locks: vec![vec![]; 2],
            mute_states: vec![false; 2],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_status: None,
            cmdline_candidates: vec![],
            slot_a_flash: false,
            slot_c: None,
            slot_c_value: 0.0,
            slot_c_locked: false,
            slot_c_flash: false,
            slot_b_flash: false,
            sub_page: 0,
            sub_page_count: 1,
            encoder_cells: vec![None; 8],
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
            kit_scroll: 0,
            kit_cursor: 0,
            kit_list: vec![],
            kit_loaded_slot: None,
            perform_mode: false,
            pattern_muted_states: vec![false; 2],
            mix_gains: vec![0.0; 2],
            mix_master: 0.0,
            step_size_tier: 0,
            help_visible: false,
        };
        terminal.draw(|f| render(f, &data)).unwrap();
    }

    #[test]
    fn render_perf_does_not_panic() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData {
            screen: Screen::Param(1),
            rec: RecMode::Off,
            armed_prefix: None,
            active_track: 0,
            track_names: vec!["AnalogKick".into()],
            display_names: vec!["Kick".into()],
            trig_key_labels: vec![None; 16],
            track_key_labels: vec![None; 1],
            legend_key_labels: HashMap::new(),
            bpm: 120.0,
            playing: false,
            page_window: 0,
            step_state: StepState::default(),
            step_states: vec![],
            slot_a: Some(SlotBinding {
                node_id: 20,
                param_id: 1,
                param_name: "decay".into(),
                min: 0.0,
                max: 1.0,
            }),
            slot_a_value: 0.42,
            slot_b: Some(SlotBinding {
                node_id: 20,
                param_id: 2,
                param_name: "tune".into(),
                min: 0.0,
                max: 1.0,
            }),
            slot_b_value: 0.7,
            page_groups: vec!["SRC".into(), "AMP".into()],
            perf_page: 1,
            envelope: Some((
                EnvelopeData {
                    param_id: 1,
                    param_name: "decay".into(),
                    node_id: 20,
                    env_type: "AD".into(),
                    min: 0.0,
                    max: 1.0,
                },
                0.42,
            )),
            live_env_level: Some(0.73),
            live_lfo_phase: None,
            debug_event: None,
            enc: false,
            lock_target_step: None,
            last_jog_param: None,
            step_locks: vec![vec![]; 1],
            mute_states: vec![false; 1],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_status: None,
            cmdline_candidates: vec![],
            slot_a_flash: false,
            slot_c: None,
            slot_c_value: 0.0,
            slot_c_locked: false,
            slot_c_flash: false,
            slot_b_flash: false,
            sub_page: 0,
            sub_page_count: 1,
            encoder_cells: vec![None; 8],
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
            kit_scroll: 0,
            kit_cursor: 0,
            kit_list: vec![],
            kit_loaded_slot: None,
            perform_mode: false,
            pattern_muted_states: vec![false; 1],
            mix_gains: vec![0.0; 1],
            mix_master: 0.0,
            step_size_tier: 0,
            help_visible: false,
        };
        terminal.draw(|f| render(f, &data)).unwrap();
    }

    /// TK2.1 C0 (D2): renamed from `grid_structure_4_tracks_23_rows`, which
    /// pinned the deleted all-tracks-stacked grid — the trig strip renders
    /// exactly two rows (the selected track only), regardless of track
    /// count.
    #[test]
    fn strip_structure_is_two_rows() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 4);
        data.active_track = 1;
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert_eq!(
            text.matches(" 1-8").count(),
            1,
            "the trig strip must render exactly one ' 1-8' row (selected \
             track only), not one per track; got: {text}"
        );
        assert_eq!(
            text.matches("9-16").count(),
            1,
            "the trig strip must render exactly one '9-16' row (selected \
             track only), not one per track; got: {text}"
        );
    }

    fn buffer_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    /// Extracts a single row's text — used where whole-buffer text would
    /// mix glyphs from two regions that legitimately reuse the same
    /// characters (e.g. `▓`/`░` in both the trig strip and the contextual
    /// window's param bars).
    fn buffer_row_text(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        row: u16,
        width: u16,
    ) -> String {
        let buf = terminal.backend().buffer();
        (0..width).map(|x| buf.get(x, row).symbol()).collect()
    }

    /// Row index of the first line whose text contains `needle`, or panics.
    fn find_row(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        height: u16,
        width: u16,
        needle: &str,
    ) -> u16 {
        for row in 0..height {
            if buffer_row_text(terminal, row, width).contains(needle) {
                return row;
            }
        }
        panic!("no row contains {needle:?}");
    }

    /// TK2.1 C0 (D2): the trig strip shows the selected track's steps only
    /// — a second track's pattern must not leak into the rendered strip.
    #[test]
    fn trig_strip_renders_only_selected_track() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 2);
        data.active_track = 1;
        // Track 0: every step active. Track 1 (selected): every step empty.
        // If the strip leaked track 0's pattern, active glyphs would show.
        data.step_states = vec![
            StepState {
                pattern_length: 16,
                page_count: 1,
                steps: vec![true; 16],
                current_step: usize::MAX, // no playhead cell to confound the glyph count
            },
            StepState {
                pattern_length: 16,
                page_count: 1,
                steps: vec![false; 16],
                current_step: usize::MAX,
            },
        ];
        terminal.draw(|f| render(f, &data)).unwrap();

        // The strip is the two rows directly below the one-line track
        // indicator, which is the row containing "PTN P" (§3).
        let indicator_row = find_row(&terminal, 24, 80, "PTN P");
        let strip_row_1 = buffer_row_text(&terminal, indicator_row + 1, 80);
        let strip_row_2 = buffer_row_text(&terminal, indicator_row + 2, 80);
        assert!(
            !strip_row_1.contains('▓') && !strip_row_2.contains('▓'),
            "the strip must render track 1 (all empty), not track 0 (all \
             active); got row1={strip_row_1:?} row2={strip_row_2:?}"
        );
    }

    /// TK2.2 C3 audit (spec: "verify, don't assume" `render_trig_row` is
    /// stable against the E2 reflow class): the trig strip's `[k]g` cell
    /// format is fixed at 5 columns (tk2.1-theotokos.md §3) and the chip
    /// is shown — bright or dim — in every `RecMode`, never hidden, so
    /// unlike the track line there is no mode-dependent column shift to
    /// fix here. This pins that finding as a regression test.
    #[test]
    fn trig_strip_cells_do_not_shift_between_off_and_grid() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);

        data.rec = RecMode::Off;
        terminal.draw(|f| render(f, &data)).unwrap();
        let indicator_row = find_row(&terminal, 24, 80, "PTN P");
        let off_row1 = buffer_row_text(&terminal, indicator_row + 1, 80);

        data.rec = RecMode::Grid;
        terminal.draw(|f| render(f, &data)).unwrap();
        let grid_row1 = buffer_row_text(&terminal, indicator_row + 1, 80);

        let off_col = off_row1
            .find("[q]")
            .expect("Trig1's cell must render in Off");
        let grid_col = grid_row1
            .find("[q]")
            .expect("Trig1's cell must render in Grid");
        assert_eq!(
            off_col, grid_col,
            "the trig strip's cells must not shift between Off and Grid; \
             Off: {off_row1:?}, Grid: {grid_row1:?}"
        );
    }

    /// TK2.1 C0 (D2): the track line lists every track with its display
    /// name and a mute marker for muted tracks.
    #[test]
    fn track_indicator_lists_tracks_with_mute_markers() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 2);
        data.display_names = vec!["Kick".into(), "Snare".into()];
        data.mute_states = vec![false, true];
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("Kick"), "must list Kick; got: {text}");
        assert!(text.contains("Snare"), "must list Snare; got: {text}");
        assert!(
            text.contains("Snare●") || text.contains("Snare ●"),
            "muted Snare must carry a mute marker; got: {text}"
        );
    }

    /// P11 C6e: the track indicator also marks pattern-muted tracks with
    /// a distinct glyph (`◌`), separate from the global mute's `●`.
    #[test]
    fn track_indicator_marks_pattern_muted_tracks() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 2);
        data.display_names = vec!["Kick".into(), "Snare".into()];
        data.pattern_muted_states = vec![false, true];
        terminal.draw(|f| render(f, &data)).unwrap();

        let indicator_row = find_row(&terminal, 24, 80, "PTN P");
        let line = buffer_row_text(&terminal, indicator_row, 80);
        assert!(
            line.contains("Snare◌"),
            "a pattern-muted track must carry the pattern-mute marker; got: {line:?}"
        );
        assert!(
            !line.contains("Kick◌"),
            "an unmuted track must not carry the marker; got: {line:?}"
        );
    }

    /// P11 C6b: the status line shows `⚡` while perform mode is on, and
    /// nothing when it's off.
    #[test]
    fn status_line_shows_perform_mode_indicator() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.perform_mode = true;
        terminal.draw(|f| render(f, &data)).unwrap();
        let status_row = 23; // fixed last row on the 24-row test terminal
        let line = buffer_row_text(&terminal, status_row, 80);
        assert!(
            line.contains('⚡'),
            "the status line must show the perform indicator; got: {line:?}"
        );

        let mut terminal2 = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24))
            .unwrap();
        let data2 = RenderData::for_test(Screen::Grid, 1); // perform_mode = false
        terminal2.draw(|f| render(f, &data2)).unwrap();
        let line2 = buffer_row_text(&terminal2, status_row, 80);
        assert!(
            !line2.contains('⚡'),
            "perform mode off must not show the indicator; got: {line2:?}"
        );
    }

    /// TK3 C2: the MIX screen draws one row per track, a MASTER row, and
    /// the two-decimal gain values.
    #[test]
    fn mix_screen_renders_track_rows_and_master() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Mix, 2);
        data.display_names = vec!["Kick".into(), "Snare".into()];
        data.mix_gains = vec![1.0, 0.5];
        data.mix_master = 0.8;
        terminal.draw(|f| render(f, &data)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            text.contains("Kick") && text.contains("Snare"),
            "each track row must render; got: {text}"
        );
        assert!(text.contains("MASTER"), "a MASTER row must render: {text}");
        assert!(
            text.contains("0.50") && text.contains("0.80"),
            "gain values must render; got: {text}"
        );
    }

    /// P11 C6a: the KIT screen renders the parsed kit list with the
    /// loaded-kit marker, the cursor, and the bottom-row button labels.
    fn kit_render_data() -> RenderData {
        let mut data = RenderData::for_test(Screen::Kit, 1);
        data.kit_list = crate::model::parse_kit_list("0:Kick Basic;1:Snare Tight;2:Hat Crisp");
        data.kit_loaded_slot = Some(1);
        data.kit_cursor = 0;
        data
    }

    #[test]
    fn kit_screen_renders_list_loaded_marker_and_buttons() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = kit_render_data();
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("Kick Basic"), "must list kit 0; got: {text}");
        assert!(text.contains("Snare Tight"), "must list kit 1; got: {text}");
        assert!(text.contains("Hat Crisp"), "must list kit 2; got: {text}");
        assert!(text.contains("(empty)"), "unlisted slots read empty; got: {text}");
        assert!(
            text.contains("◄ loaded"),
            "the slot bound to the active pattern must be marked loaded; got: {text}"
        );
        assert!(
            text.contains("[LOAD]") && text.contains("[SAVE]") && text.contains("[COMMIT]")
                && text.contains("[RELD]"),
            "the bottom row must show the four button labels; got: {text}"
        );
        // The encoder-bank preview (row 2 per spec — the contextual
        // window's first row, which is terminal row 1 after the transport
        // bar) shows the selected kit's name as the names-only placeholder
        // for its entries.
        let bank_row = find_row(&terminal, 24, 80, "Kick Basic");
        assert_eq!(bank_row, 1, "the bank preview must sit on row 2; got: {bank_row}");
    }

    #[test]
    fn kit_screen_marks_the_cursor_slot_with_reversed_style() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = kit_render_data();
        data.kit_cursor = 2;
        terminal.draw(|f| render(f, &data)).unwrap();

        // The cursor row is ` 3 Hat Crisp` in the LIST (the bank preview
        // row above also spells "Hat Crisp", so match the numbered line).
        let row = find_row(&terminal, 24, 80, " 3 Hat Crisp");
        let buf = terminal.backend().buffer();
        let cell = buf.get(2, row); // the `3` in the slot number prefix
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "the cursor slot must render inverted; got {:?} at ({}, {})",
            cell,
            2,
            row
        );
    }

    /// P11 C6a: the selected slot's name feeds the read-only encoder-bank
    /// preview; an empty selected slot renders `(empty)`.
    #[test]
    fn kit_screen_empty_selected_slot_bank_shows_empty() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Kit, 1);
        data.kit_list = crate::model::parse_kit_list("0:Kick Basic");
        data.kit_cursor = 5; // not in the list → empty
        terminal.draw(|f| render(f, &data)).unwrap();

        let bank_row = find_row(&terminal, 24, 80, "(empty)");
        assert_eq!(bank_row, 1, "an empty selected slot must preview (empty) on row 2");
    }

    /// ADR-044 D2: when the full track list doesn't fit, the indicator
    /// windows around the selected track with `‹`/`›` markers rather than
    /// truncating silently.
    #[test]
    fn track_indicator_windows_around_selected_track_when_crowded() {
        let backend = ratatui::backend::TestBackend::new(70, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 12);
        data.display_names = (1..=12).map(|i| format!("TrackNumber{i:02}")).collect();
        data.active_track = 6; // mid-list: both edges must be hidden
        terminal.draw(|f| render(f, &data)).unwrap();

        let indicator_row = find_row(&terminal, 24, 70, "PTN P");
        let line = buffer_row_text(&terminal, indicator_row, 70);
        assert!(
            line.contains('‹') && line.contains('›'),
            "a crowded track list must window with both markers when the \
             selection sits mid-list; got: {line:?}"
        );
        assert!(
            line.contains("TrackNumber07"),
            "the selected track must stay visible inside the window; \
             got: {line:?}"
        );
    }

    /// TK2.1 C0 (D1/D2): the trig strip and track line are rendered from
    /// `render()` directly, never from a per-screen branch — they must
    /// appear on every screen, not just Grid.
    #[test]
    fn trig_strip_and_track_line_render_on_every_screen() {
        for screen in [
            Screen::Grid,
            Screen::Param(0),
            Screen::Tempo,
            Screen::Chain,
            Screen::Settings,
            Screen::Kit,
        ] {
            let backend = ratatui::backend::TestBackend::new(80, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let data = RenderData::for_test(screen, 1);
            terminal.draw(|f| render(f, &data)).unwrap();

            let text = buffer_text(&terminal);
            assert!(
                text.contains("PTN P"),
                "track indicator must render on {screen:?}; got: {text}"
            );
            assert!(
                text.contains(" 1-8") && text.contains("9-16"),
                "trig strip must render on {screen:?}; got: {text}"
            );
        }
    }

    /// TK2.1 C0 (D1): region heights never change across screens — only
    /// the contextual window's content varies.
    #[test]
    fn region_heights_are_identical_across_screens() {
        let mut rows: Vec<u16> = Vec::new();
        for screen in [
            Screen::Grid,
            Screen::Param(0),
            Screen::Tempo,
            Screen::Chain,
            Screen::Settings,
            Screen::Kit,
        ] {
            let backend = ratatui::backend::TestBackend::new(80, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let data = RenderData::for_test(screen, 1);
            terminal.draw(|f| render(f, &data)).unwrap();
            rows.push(find_row(&terminal, 24, 80, "PTN P"));
        }
        assert!(
            rows.iter().all(|r| *r == rows[0]),
            "the track indicator must land on the same row on every \
             screen; got: {rows:?}"
        );
    }

    /// TK2.1 C0: the contextual window's header shows both the display
    /// name and the engine name.
    #[test]
    fn track_context_shows_display_name_and_engine_name() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.display_names = vec!["Kick".into()];
        data.track_names = vec!["AnalogKick".into()];
        terminal.draw(|f| render(f, &data)).unwrap();

        // Scoped to the header line itself (found via the `—` separator
        // only the header emits) — a whole-buffer search can't tell this
        // apart from the engine name leaking into the transport bar or
        // status line (the exact bug this field split exists to prevent).
        let header_row = find_row(&terminal, 24, 80, "—");
        let line = buffer_row_text(&terminal, header_row, 80);
        assert!(
            line.contains("Kick") && line.contains("AnalogKick"),
            "contextual header must show display name and engine name; \
             got: {line:?}"
        );
    }

    /// TK2.1 C0 (D2, §3): the transport bar and status line show the
    /// display name ("Kick"), not the engine/cap-doc name ("AnalogKick") —
    /// review finding: these two consumers were initially left reading
    /// `track_names` (engine name) after the display_name/track_names
    /// split, exactly reintroducing the bug the split exists to fix.
    #[test]
    fn transport_and_status_line_show_display_name_not_engine_name() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.display_names = vec!["Kick".into()];
        data.track_names = vec!["AnalogKick".into()];
        terminal.draw(|f| render(f, &data)).unwrap();

        let transport_line = buffer_row_text(&terminal, 0, 80);
        assert!(
            transport_line.contains("Kick") && !transport_line.contains("AnalogKick"),
            "transport bar must show the display name, not the engine \
             name; got: {transport_line:?}"
        );

        let status_row = 23; // fixed last row: 24-row terminal, status is Length(1) at the bottom
        let status_line = buffer_row_text(&terminal, status_row, 80);
        assert!(
            status_line.contains("Kick") && !status_line.contains("AnalogKick"),
            "status line must show the display name, not the engine name; \
             got: {status_line:?}"
        );
    }

    /// TK2.1 C0: a track with no page params (no composite view, no `Rule`
    /// pagination) renders a placeholder line, not an empty pane.
    #[test]
    fn track_context_without_page_params_renders_placeholder() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1); // encoder_cells: all None
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("no page params"),
            "an empty page-param resolution must render the placeholder; \
             got: {text}"
        );
    }

    /// TK2.1 C0 (§3 minimum width): below 60 columns the track line drops
    /// names, keeping the track number.
    #[test]
    fn narrow_terminal_drops_names_before_chips() {
        let backend = ratatui::backend::TestBackend::new(40, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.display_names = vec!["Kick".into()];
        terminal.draw(|f| render(f, &data)).unwrap();

        // Scoped to the track line itself — the contextual header
        // legitimately shows the name at any width; only the §3 minimum-
        // width rule for the track line is under test here.
        let indicator_row = find_row(&terminal, 24, 40, "PTN P");
        let line = buffer_row_text(&terminal, indicator_row, 40);
        assert!(
            !line.contains("Kick"),
            "below the 60-column minimum, the track line must drop names; \
             got: {line:?}"
        );
        assert!(
            line.contains("[q]1"),
            "the track number AND its key chip must survive the narrow \
             drop — names drop before chips (D3); got: {line:?}"
        );
    }

    #[test]
    fn status_line_shows_rec_state_and_armed_prefix() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.rec = RecMode::Off;
        data.armed_prefix = Some("TRK…".to_string());
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("REC○"),
            "RecMode::Off must show the REC○ glyph; got: {text}"
        );
        assert!(
            text.contains("TRK"),
            "an armed TRK prefix must appear in the status line; got: {text}"
        );
    }

    /// TK2.2 C4 (E5): with a lock target armed, the status line names
    /// which step the last jog wrote — not just that some step is
    /// locked, which parameter.
    #[test]
    fn jog_indicator_names_the_locked_step_when_target_armed() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.last_jog_param = Some("cutoff".to_string());
        data.lock_target_step = Some(3);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("J:cutoff→s3"),
            "with a target armed the panel must name the locked step and \
             param; got: {text}"
        );
    }

    /// TK2.2 C4 (E5): with no lock target, the same jog reads as writing
    /// the live value, not a step.
    #[test]
    fn jog_indicator_shows_live_when_no_target() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.last_jog_param = Some("cutoff".to_string());
        data.lock_target_step = None;
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("J:cutoff→live"),
            "with no target the panel must show the live parameter; \
             got: {text}"
        );
        assert!(
            !text.contains("→s"),
            "with no target nothing must read as a step write; got: {text}"
        );
    }

    /// TK2.2 C4 (E5 test): "the indicator clears with the target" — the
    /// locked-step naming must track the CURRENT lock target, not a
    /// snapshot from jog time, so it reverts to live-framing the instant
    /// the target itself clears (e.g. on Esc), not on the next jog.
    #[test]
    fn jog_indicator_locked_naming_clears_with_the_target() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.last_jog_param = Some("cutoff".to_string());
        data.lock_target_step = Some(3);
        terminal.draw(|f| render(f, &data)).unwrap();
        assert!(buffer_text(&terminal).contains("J:cutoff→s3"));

        // The target clears (e.g. Esc) — no new jog has happened, but the
        // step-naming must already be gone on this same render.
        data.lock_target_step = None;
        terminal.draw(|f| render(f, &data)).unwrap();
        let text = buffer_text(&terminal);
        assert!(
            !text.contains("→s3"),
            "the locked-step naming must clear the instant the target \
             does, not linger until the next jog; got: {text}"
        );
        assert!(
            text.contains("J:cutoff→live"),
            "it must fall back to live-framing for the same last-jogged \
             param, not vanish entirely; got: {text}"
        );
    }

    /// TK2.2 C4 (E5): with no jog yet, nothing is shown — there is
    /// nothing to name.
    #[test]
    fn jog_indicator_absent_before_any_jog() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            !text.contains("J:"),
            "with no jog yet, no jog indicator must render; got: {text}"
        );
    }

    /// TK2.1 C1 (D5): the three-state REC glyph on both the transport bar
    /// and the status line.
    #[test]
    fn rec_indicator_shows_three_states() {
        for (mode, glyph) in [
            (RecMode::Off, "REC○"),
            (RecMode::Grid, "REC▦"),
            (RecMode::Live, "REC●"),
        ] {
            let backend = ratatui::backend::TestBackend::new(80, 24);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            let mut data = RenderData::for_test(Screen::Grid, 1);
            data.rec = mode;
            terminal.draw(|f| render(f, &data)).unwrap();

            let text = buffer_text(&terminal);
            assert!(
                text.contains(glyph),
                "{mode:?} must render {glyph}; got: {text}"
            );
        }
    }

    // ── TK2.1 C2: key chips and legend (D3/D4) ──────────────────────────

    #[test]
    fn strip_cells_show_key_chips_in_grid_mode() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.rec = RecMode::Grid;
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("[q]"),
            "Grid mode must show the trig's key chip on its step cell; \
             got: {text}"
        );
    }

    #[test]
    fn chips_move_to_track_line_in_pad_mode() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.rec = RecMode::Off;
        terminal.draw(|f| render(f, &data)).unwrap();

        let indicator_row = find_row(&terminal, 24, 80, "PTN P");
        let line = buffer_row_text(&terminal, indicator_row, 80);
        assert!(
            line.contains("[q]1"),
            "pad mode must show the key chip on the track indicator; \
             got: {line:?}"
        );
    }

    /// TK2.2 C3 (E2 regression, closes F3): a hidden chip still reserves
    /// its width — the track line's columns must be identical whether the
    /// per-track chip renders (Off) or is blanked (Grid).
    #[test]
    fn track_line_columns_are_identical_in_off_and_grid() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);

        data.rec = RecMode::Off;
        terminal.draw(|f| render(f, &data)).unwrap();
        let row = find_row(&terminal, 24, 80, "PTN P");
        let off_line = buffer_row_text(&terminal, row, 80);

        data.rec = RecMode::Grid;
        terminal.draw(|f| render(f, &data)).unwrap();
        let grid_line = buffer_row_text(&terminal, row, 80);

        let off_name_col = off_line.find(" T1").expect("track name must render in Off");
        let grid_name_col = grid_line
            .find(" T1")
            .expect("track name must render in Grid");
        assert_eq!(
            off_name_col, grid_name_col,
            "the track name must land in the same column in Off and Grid; \
             Off: {off_line:?}, Grid: {grid_line:?}"
        );

        let off_ptn_col = off_line
            .find("PTN P")
            .expect("PTN field must render in Off");
        let grid_ptn_col = grid_line
            .find("PTN P")
            .expect("PTN field must render in Grid");
        assert_eq!(
            off_ptn_col, grid_ptn_col,
            "the pattern field must land in the same column in Off and \
             Grid; Off: {off_line:?}, Grid: {grid_line:?}"
        );
    }

    /// TK2.2 C3 (E2 caution): `chip_key_display` can yield a multi-char
    /// label under ADR-037 remapping (e.g. "Tab") — the reserved width
    /// when the chip is hidden must be computed from that same label, not
    /// a hardcoded single-character assumption.
    #[test]
    fn remapped_multichar_track_key_does_not_reflow() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.track_key_labels = vec![Some("tab".to_string())];

        data.rec = RecMode::Off;
        terminal.draw(|f| render(f, &data)).unwrap();
        let row = find_row(&terminal, 24, 80, "PTN P");
        let off_line = buffer_row_text(&terminal, row, 80);
        assert!(
            off_line.contains("[Tab]"),
            "sanity: the remapped chip must title-case in Off; got: {off_line:?}"
        );

        data.rec = RecMode::Grid;
        terminal.draw(|f| render(f, &data)).unwrap();
        let grid_line = buffer_row_text(&terminal, row, 80);

        let off_col = off_line.find(" T1").expect("track name must render in Off");
        let grid_col = grid_line
            .find(" T1")
            .expect("track name must render in Grid");
        assert_eq!(
            off_col, grid_col,
            "a multi-char remapped chip must reserve the same width when \
             hidden; Off: {off_line:?}, Grid: {grid_line:?}"
        );
    }

    /// D3 "no chip without an action": with only one discovered track, no
    /// entry (and so no chip) exists for the keys addressing tracks 2-16.
    #[test]
    fn pad_column_without_a_track_has_no_chip() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.rec = RecMode::Off;
        terminal.draw(|f| render(f, &data)).unwrap();

        let indicator_row = find_row(&terminal, 24, 80, "PTN P");
        let line = buffer_row_text(&terminal, indicator_row, 80);
        assert!(
            !line.contains("[w]"),
            "a pad column past the discovered track count must have no \
             chip; got: {line:?}"
        );
    }

    #[test]
    fn legend_renders_labeled_chips() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        // TK2.2 C3 (E1): Trk/Ptn/Rec/Play moved inline onto the panel
        // elements they act on and left the legend — only affordances
        // with no on-screen referent remain here.
        for expected in [
            "[n] ENC",
            "[m] LOCK",
            "[o] SONG",
            "[8] SET",
            "[Enter] YES",
            "[Esc] NO",
        ] {
            assert!(
                text.contains(expected),
                "legend must render {expected:?}; got: {text}"
            );
        }
    }

    /// TK2.1 C5: fulfills the `[n] ENC`/`[m] LOCK` entries C2 deferred —
    /// with ENC on, the legend shows the "any screen" ENC row regardless
    /// of which screen is open, and the ENC chip's label reflects state
    /// (`ENC off`, not `ENC`, since a second press turns it off).
    #[test]
    fn enc_legend_overrides_the_per_screen_list() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.enc = true;
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("[m] LOCK"),
            "ENC-on legend must show the LOCK chip; got: {text}"
        );
        assert!(
            text.contains("ENC off"),
            "the ENC chip's label must reflect the current state; got: {text}"
        );
        assert!(
            !text.contains("[Tab] TRK"),
            "the ENC-on legend replaces the per-screen list entirely; \
             got: {text}"
        );
    }

    /// D4: "on overflow it truncates from the tail of that list. It never
    /// wraps, scrolls or moves" — a narrow terminal must drop the lowest-
    /// priority chips, not wrap them to a third line.
    #[test]
    fn legend_truncates_from_the_tail_when_crowded() {
        let backend = ratatui::backend::TestBackend::new(30, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("[n] ENC"),
            "the highest-priority chip must always survive; got: {text}"
        );
        assert!(
            !text.contains("QUIT"),
            "at this width the lowest-priority chip must be dropped, not \
             wrapped to a third line; got: {text}"
        );
    }

    /// D4: `:`, `?` and the `1-6`/`-/=` range chips have no `PanelButton`
    /// and bypass the keymap entirely — they must render even when every
    /// dynamic (key_label-derived) legend entry has nothing to resolve.
    #[test]
    fn legend_literal_entries_are_not_derived() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.legend_key_labels.clear();
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        for literal in ["[:] CMD", "[?] HELP", "[1-6] PAGE", "[-/=] WIN"] {
            assert!(
                text.contains(literal),
                "a literal legend entry must render even with no dynamic \
                 bindings resolved; got: {text}"
            );
        }
        assert!(
            !text.contains("TRK"),
            "a dynamic chip with no resolvable key must be omitted \
             entirely, not shown with a blank/garbled key; got: {text}"
        );
    }

    /// TK2.2 C3 (E1 regression): z/x/p/Tab/0 all moved onto panel elements
    /// (transport/track line) — they legitimately appear elsewhere on
    /// screen now, so this isolates the legend's own two rows rather than
    /// searching the whole buffer.
    #[test]
    fn legend_contains_no_inlined_chips() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1);
        terminal.draw(|f| render(f, &data)).unwrap();

        let legend_row0 = find_row(&terminal, 24, 100, "[n] ENC");
        let legend_text = format!(
            "{}{}",
            buffer_row_text(&terminal, legend_row0, 100),
            buffer_row_text(&terminal, legend_row0 + 1, 100),
        );
        for absent in ["[z]", "[x]", "[p]", "[Tab]", "[0]", "[c]"] {
            assert!(
                !legend_text.contains(absent),
                "the legend must not advertise {absent:?} — it moved \
                 inline; got: {legend_text:?}"
            );
        }
    }

    /// TK2.2 C5 (E1 completion, ADR-046): [c] STOP rejoins the transport
    /// line's inline chips now that it has a meaning (halt + rewind).
    #[test]
    fn transport_line_shows_stop_chip() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("[c]"),
            "the transport line must advertise [c] STOP; got: {text}"
        );
    }

    /// D3: chip casing is display-only — multi-character key names
    /// title-case (`[Tab]`, not `[tab]`), single characters stay as typed
    /// (`[q]`, not `[Q]`). The keymap storage form (`key_name`) is
    /// untouched by this — only the rendered chip.
    #[test]
    fn chip_titlecases_named_keys_only() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData::for_test(Screen::Grid, 1);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("[Tab]"),
            "multi-char key names must title-case; got: {text}"
        );
        assert!(
            !text.contains("[tab]"),
            "must not render the lowercase storage form; got: {text}"
        );
        assert!(
            text.contains("[q]"),
            "single-char key names must stay as typed; got: {text}"
        );
        assert!(
            !text.contains("[Q]"),
            "single-char keys must not be uppercased; got: {text}"
        );
    }

    #[test]
    fn help_overlay_lists_panel_buttons() {
        let backend = ratatui::backend::TestBackend::new(100, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.help_visible = true;
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        for token in ["TRK", "PTN", "REC", "FUNC", "Trig"] {
            assert!(
                text.contains(token),
                "help overlay must list panel button/concept {token}; got: {text}"
            );
        }
    }

    #[test]
    fn param_screen_shows_eight_encoders() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Param(0), 1);
        data.encoder_cells = vec![
            Some(EncoderCell {
                name: "decay".into(),
                value: 0.5,
                min: 0.0,
                max: 1.0,
                resolved: true,
                options: None,
                target: crate::model::EncoderTarget::Real,
            }),
            Some(EncoderCell {
                name: "tune".into(),
                value: 0.25,
                min: 0.0,
                max: 1.0,
                resolved: true,
                options: None,
                target: crate::model::EncoderTarget::Real,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("decay") && text.contains("tune"),
            "the Param screen must render all populated encoder cells; got: {text}"
        );
        assert!(
            text.matches("--").count() >= 6,
            "encoder cells past the page's param count must render blank, not \
             a malformed cell; got: {text}"
        );
    }

    /// #176 (BUG-067): a stepped selector draws its name, not its index.
    ///
    /// The reported symptom was `lfo_dest █░░░ 1.00` — a performer choosing a
    /// modulation destination by counting table positions. Asserted at the
    /// drawn buffer, since that is the thing the session actually observed.
    #[test]
    fn a_stepped_selector_draws_its_name_not_its_index() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Param(0), 1);
        data.encoder_cells = vec![
            Some(EncoderCell {
                name: "lfo_dest".into(),
                value: 1.0,
                min: 0.0,
                max: 8.0,
                resolved: true,
                options: Some(vec![Some("off".into()), Some("tune".into())]),
                target: crate::model::EncoderTarget::Real,
            }),
            Some(EncoderCell {
                name: "decay".into(),
                value: 0.25,
                min: 0.0,
                max: 1.0,
                resolved: true,
                options: None,
                target: crate::model::EncoderTarget::Real,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("lfo_dest") && text.contains("tune"),
            "the dest encoder must name its destination; got: {text}"
        );
        assert!(
            !text.contains("lfo_dest █░░░ 1.00") && !text.contains("lfo_dest ▓░░░ 1.00"),
            "and must not still show the bare index; got: {text}"
        );
        assert!(
            text.contains("0.25"),
            "a continuous param keeps its numeric readout; got: {text}"
        );
    }

    /// TK2.1 C4 (D10, closes BUG-040 §1): an unresolved cell (a composite
    /// param with no matching cap-doc entry — `min`/`max` are the 0..1
    /// last-resort fallback) renders dimmed so the condition is visible.
    #[test]
    fn unresolvable_param_falls_back_and_renders_dim() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Param(0), 1);
        data.encoder_cells = vec![
            Some(EncoderCell {
                name: "mystery".into(),
                value: 0.5,
                min: 0.0,
                max: 1.0,
                resolved: false,
                options: None,
                target: crate::model::EncoderTarget::Real,
            }),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        terminal.draw(|f| render(f, &data)).unwrap();

        let row = find_row(&terminal, 24, 100, "mystery");
        let line = buffer_row_text(&terminal, row, 100);
        let col = line.find('m').expect("the cell's name must render") as u16;
        let fg = terminal.backend().buffer().get(col, row).fg;
        assert_eq!(
            fg,
            Color::DarkGray,
            "an unresolved cell must render dimmed, not the normal white"
        );
    }

    #[test]
    fn param_screen_animates_envelope_and_lfo() {
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Param(0), 1);
        data.envelope = Some((
            EnvelopeData {
                param_id: 1,
                param_name: "decay".into(),
                node_id: 20,
                env_type: "AD".into(),
                min: 0.0,
                max: 1.0,
            },
            0.5,
        ));
        data.live_env_level = Some(0.72);
        data.live_lfo_phase = Some(0.34);
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("▶"),
            "live env_level: must show the play glyph indicating animation; got: {text}"
        );
        assert!(
            text.contains("●"),
            "live LFO phase: must show the dot marker in the phase track; got: {text}"
        );
    }
}
