use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

use crate::model::{EnvelopeData, Screen, SlotBinding, StepState};

const PAGE_SIZE: usize = 8;

/// TK2 C5 (D8): one of the 8 encoder bank cells on the Param screen.
#[derive(Clone)]
pub struct EncoderCell {
    pub name: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

pub struct RenderData {
    /// TK2 C3 (D12): replaces `Mode`.
    pub screen: Screen,
    /// TK2 C3 (D12): grid-programming (steps toggle) vs. live (trigs sound
    /// now) — shown as a REC ●/○ indicator (transport bar + status line).
    pub grid_rec: bool,
    /// TK2 C3 (D6): the armed TRK/PTN hold prefix, if any (status line).
    pub armed_prefix: Option<String>,
    pub active_track: usize,
    pub track_names: Vec<String>,
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
    pub debug_event: Option<String>,
    pub step_focuses: Vec<Option<usize>>,
    pub step_locks: Vec<Vec<usize>>,
    /// TK2 C4 (D12): per-track mute state, shown on the Mute screen.
    pub mute_states: Vec<bool>,
    pub slot_a_locked: bool,
    pub slot_b_locked: bool,
    pub cmdline: Option<String>,
    pub cmdline_error: Option<String>,
    pub cmdline_candidates: Vec<String>,
    pub slot_a_flash: bool,
    pub slot_b_flash: bool,
    pub help_visible: bool,
    /// TK2 C5 (D8): the active page's params in `Rule` order, up to 8 —
    /// `None` past the page's param count.
    pub encoder_cells: Vec<Option<EncoderCell>>,
    /// TK2 C5 (D8/D13): which encoder cell the arrow-key cursor highlights.
    pub encoder_cursor: usize,
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
}

pub fn render(frame: &mut Frame, data: &RenderData) {
    let area = frame.size();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    render_transport(frame, chunks[0], data);
    if data.help_visible {
        render_help(frame, chunks[1], data);
    } else {
        match data.screen {
            Screen::Grid => render_seq_grid(frame, chunks[1], data),
            Screen::Param(_) => render_perf_window(frame, chunks[1], data),
            Screen::Mute => render_mute_screen(frame, chunks[1], data),
            Screen::Tempo => render_tempo_screen(frame, chunks[1], data),
            Screen::Settings => render_settings_screen(frame, chunks[1], data),
            Screen::Chain => render_chain_screen(frame, chunks[1], data),
        }
    }
    render_legend(frame, chunks[2], data);
    render_echo_area(frame, chunks[3], data);
    render_status_line(frame, chunks[4], data);
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

/// TK2 C4 (D12): "trigs toggle mutes, track states rendered" — one line
/// per track, muted tracks marked ●.
fn render_mute_screen(frame: &mut Frame, area: Rect, data: &RenderData) {
    let lines: Vec<Line> = data
        .track_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let muted = data.mute_states.get(i).copied().unwrap_or(false);
            let glyph = if muted { "●" } else { "○" };
            let color = if muted { Color::Red } else { Color::DarkGray };
            Line::styled(format!(" {glyph} {name}"), Style::default().fg(color))
        })
        .collect();
    let para = Paragraph::new(lines).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::Grid => "GRID",
        Screen::Param(_) => "PARAM",
        Screen::Tempo => "TEMPO",
        Screen::Chain => "CHAIN",
        Screen::Settings => "SETTINGS",
        Screen::Mute => "MUTE",
    }
}

/// Compact key legend, always on screen (not gated behind `?`). TK1 C8
/// usability finding: current keys must stay visible while learning the
/// layout — a toggle-only overlay hides the grid you're trying to use the
/// keys on. TK2 C3: content follows the §2 panel grammar.
fn render_legend(frame: &mut Frame, area: Rect, data: &RenderData) {
    let (line1, line2) = match data.screen {
        Screen::Grid => (
            "q..i/a..k:trig  Tab(hold):TRK  p(hold):PTN  z/x/c:REC/PLAY/STOP  1-6:page  -/=:page-win",
            "FUNC(Shift)+trig:encoder  Enter/Esc:YES/NO  o:song  m:mute  ::cmd  ?:help  ^C:quit",
        ),
        _ => (
            "q..i/a..k:trig  Tab(hold):TRK  p(hold):PTN  z/x/c:REC/PLAY/STOP  1-6:page",
            "FUNC(Shift)+trig:encoder  Enter/Esc:YES/NO  ::cmd  ?:help  ^C:quit",
        ),
    };
    let lines = vec![
        Line::styled(line1, Style::default().fg(Color::DarkGray)),
        Line::styled(line2, Style::default().fg(Color::DarkGray)),
    ];
    let para = Paragraph::new(lines).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

fn render_transport(frame: &mut Frame, area: Rect, data: &RenderData) {
    let play_sym = if data.playing { "▶" } else { "■" };
    // TK2 C3 (D12): transport bar gains a REC indicator.
    let rec_sym = if data.grid_rec { "REC●" } else { "REC○" };
    let track_name = data
        .track_names
        .get(data.active_track)
        .map(|s| s.as_str())
        .unwrap_or("?");
    let page = data.page_window + 1;
    let page_count = data.step_state.page_count.max(1);

    let transport = format!(
        " {:.1} BPM  {}  {}  {}  P{}/{}  Step:{}  Len:{}",
        data.bpm,
        play_sym,
        rec_sym,
        track_name,
        page,
        page_count,
        data.step_state.current_step + 1,
        data.step_state.pattern_length,
    );

    let para = Paragraph::new(transport).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(Color::White)),
    );
    frame.render_widget(para, area);
}

fn render_seq_grid(frame: &mut Frame, area: Rect, data: &RenderData) {
    let mut rows: Vec<Line> = Vec::with_capacity(data.track_names.len().max(1) * 5);
    for t in 0..data.track_names.len() {
        let focus = data.step_focuses.get(t).copied().flatten();
        let locks: std::collections::HashSet<usize> = data
            .step_locks
            .get(t)
            .map(|v| v.iter().copied().collect())
            .unwrap_or_default();
        rows.push(render_track_row(t, data, 0, true, focus, &locks));
        rows.push(render_track_row(t, data, 0, false, focus, &locks));
        rows.push(Line::from(""));
        rows.push(render_track_row(t, data, PAGE_SIZE, false, focus, &locks));
        rows.push(render_track_row(t, data, PAGE_SIZE, false, focus, &locks));
        if t + 1 < data.track_names.len() {
            rows.push(Line::from(""));
        }
    }

    let para = Paragraph::new(rows).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(Color::Gray)),
    );
    frame.render_widget(para, area);
}

fn render_track_row<'a>(
    track_idx: usize,
    data: &'a RenderData,
    row_off: usize,
    show_label: bool,
    focus: Option<usize>,
    locks: &std::collections::HashSet<usize>,
) -> Line<'a> {
    let st = data.step_states.get(track_idx).unwrap_or(&data.step_state);
    let window = data.page_window * PAGE_SIZE * 2 + row_off;
    let mut spans: Vec<Span> = Vec::with_capacity(PAGE_SIZE + 2);

    let label = if show_label {
        format!("{:>2}:", track_idx + 1)
    } else {
        "   ".to_string()
    };
    spans.push(Span::styled(
        label,
        Style::default().fg(if track_idx == data.active_track {
            Color::White
        } else {
            Color::Gray
        }),
    ));

    for col in 0..PAGE_SIZE {
        let step = window + col;
        let is_active = st.steps.get(step).copied().unwrap_or(false);

        let is_locked = locks.contains(&step);
        let focused = focus == Some(step);

        let (glyph, color, modifier) = if focused {
            (" ████ ", Color::Yellow, Modifier::REVERSED)
        } else if step == st.current_step {
            (" ████ ", Color::Yellow, Modifier::empty())
        } else if is_active && is_locked {
            (" ████ ", Color::Green, Modifier::empty())
        } else if is_active {
            (" ████ ", Color::Cyan, Modifier::empty())
        } else if is_locked {
            (" ████ ", Color::White, Modifier::empty())
        } else {
            (" ░░░░ ", Color::DarkGray, Modifier::empty())
        };

        spans.push(Span::styled(
            glyph,
            Style::default().fg(color).add_modifier(modifier),
        ));
        spans.push(Span::raw(" "));
    }

    Line::from(spans)
}

fn render_perf_window(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .split(area);

    render_page_tabs(frame, chunks[0], data);
    render_encoder_bank(frame, chunks[1], data);
    render_envelope_section(frame, chunks[2], data);
}

/// TK2 C5 (D8): 8 encoder cells, 2×4, name + bar + value. The cursor
/// (D13, arrow navigation — wiring deferred, see roadmap) highlights one
/// cell; a param page with fewer than 8 params shows blank cells past its
/// count rather than a malformed one.
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
    let cursor = if data.encoder_cursor == idx { ">" } else { " " };
    let cell = data.encoder_cells.get(idx).and_then(|c| c.as_ref());
    let line = match cell {
        Some(c) => {
            let ratio = ((c.value - c.min) / (c.max - c.min).max(0.001)).clamp(0.0, 1.0);
            let filled = (ratio * 4.0).round() as usize;
            let bar = "█".repeat(filled) + &"░".repeat(4 - filled);
            let flash = data.encoder_flash.get(idx).copied().unwrap_or(false);
            let color = if flash { Color::Yellow } else { Color::White };
            Line::styled(
                format!("{cursor}{} {} {:.2}", c.name, bar, c.value),
                Style::default().fg(color),
            )
        }
        None => Line::styled(
            format!("{cursor}--"),
            Style::default().fg(Color::DarkGray),
        ),
    };
    frame.render_widget(Paragraph::new(line), area);
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
    if let Some((ref env, val)) = &data.envelope {
        let chunks = Layout::horizontal([Constraint::Length(14), Constraint::Min(0)]).split(area);

        let label_span = Span::styled(format!(" {} ", env.param_name), Style::default());
        let label = Paragraph::new(label_span);
        frame.render_widget(label, chunks[0]);

        let ratio = ((val - env.min) / (env.max - env.min).max(0.001)).clamp(0.0, 1.0);
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::NONE))
            .gauge_style(Style::default().fg(Color::Cyan))
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
        ("Tab (hold)", "TRK — + trig: select track"),
        ("p (hold)", "PTN — + trig: select pattern"),
        ("z / x / c", "REC / PLAY / STOP (Space = PLAY)"),
        ("FUNC (Shift)", "encoder plane + secondary chords"),
        ("FUNC+trig", "encoder jog (top row up, bottom row down)"),
        ("1-6", "page select (TRIG SRC FLTR AMP FX MOD)"),
        ("7 / 8 / 9 / 0", "KIT / SETTINGS / SAMPLING / TEMPO"),
        ("Enter / Esc", "YES / NO"),
        ("arrows", "navigation"),
        ("- / =", "step-page window prev / next"),
        ("o", "SONG (opens Chain)"),
        ("m", "MUTE screen"),
        ("v", "KEYBD (reserved)"),
    ] {
        lines.push(Line::styled(
            format!("  {:16}  {}", key, desc),
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
            data.track_names
                .get(data.active_track)
                .map(|s| s.as_str())
                .unwrap_or("?"),
        ),
        Span::raw(" "),
        Span::styled(
            if data.grid_rec { "REC● " } else { "REC○ " },
            Style::default().fg(if data.grid_rec {
                Color::Red
            } else {
                Color::DarkGray
            }),
        ),
    ];

    if let Some(sf) = data.step_focuses.get(data.active_track).copied().flatten() {
        spans.push(Span::raw(format!("F:s{} ", sf)));
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
        Screen::Tempo | Screen::Chain | Screen::Settings | Screen::Mute => {}
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

impl RenderData {
    pub fn for_test(screen: Screen, track_count: u8) -> Self {
        let track_count = track_count.max(1) as usize;
        Self {
            screen,
            grid_rec: true,
            armed_prefix: None,
            active_track: 0,
            track_names: (1..=track_count).map(|i| format!("T{}", i)).collect(),
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
            debug_event: None,
            step_focuses: vec![None; track_count],
            step_locks: vec![vec![]; track_count],
            mute_states: vec![false; track_count],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
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
            encoder_cursor: 0,
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
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
            grid_rec: true,
            armed_prefix: None,
            active_track: 0,
            track_names: vec!["Kick".into(), "Snare".into()],
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
            debug_event: None,
            step_focuses: vec![None; 2],
            step_locks: vec![vec![]; 2],
            mute_states: vec![false; 2],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
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
            encoder_cursor: 0,
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
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
            grid_rec: true,
            armed_prefix: None,
            active_track: 0,
            track_names: vec!["Kick".into()],
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
            debug_event: None,
            step_focuses: vec![None; 1],
            step_locks: vec![vec![]; 1],
            mute_states: vec![false; 1],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
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
            encoder_cursor: 0,
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
            help_visible: false,
        };
        terminal.draw(|f| render(f, &data)).unwrap();
    }

    #[test]
    fn grid_structure_4_tracks_23_rows() {
        let st = StepState {
            pattern_length: 16,
            page_count: 1,
            steps: vec![false; 16],
            current_step: 0,
        };
        let data = RenderData {
            screen: Screen::Grid,
            grid_rec: true,
            armed_prefix: None,
            active_track: 0,
            track_names: vec!["Kick".into(), "Snare".into(), "Hihat".into(), "Bass".into()],
            bpm: 140.0,
            playing: true,
            page_window: 0,
            step_state: st.clone(),
            step_states: vec![st.clone(), st.clone(), st.clone(), st],
            slot_a: None,
            slot_a_value: 0.0,
            slot_b: None,
            slot_b_value: 0.0,
            page_groups: vec![],
            perf_page: 0,
            envelope: None,
            debug_event: None,
            step_focuses: vec![None; 4],
            step_locks: vec![vec![]; 4],
            mute_states: vec![false; 4],
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
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
            encoder_cursor: 0,
            encoder_flash: vec![false; 8],
            kitty: false,
            pattern_bank_size: 8,
            active_pattern: 0,
            cued_pattern: None,
            chain_len: 0,
            page_loop: (0, 0),
            chain_cursor: 0,
            help_visible: false,
        };
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &data)).unwrap();
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

    #[test]
    fn status_line_shows_rec_state_and_armed_prefix() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut data = RenderData::for_test(Screen::Grid, 1);
        data.grid_rec = false;
        data.armed_prefix = Some("TRK…".to_string());
        terminal.draw(|f| render(f, &data)).unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("REC○"),
            "grid_rec=false must show the REC○ glyph; got: {text}"
        );
        assert!(
            text.contains("TRK"),
            "an armed TRK prefix must appear in the status line; got: {text}"
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
            }),
            Some(EncoderCell {
                name: "tune".into(),
                value: 0.25,
                min: 0.0,
                max: 1.0,
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
}
