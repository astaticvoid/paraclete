use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

use crate::model::{EnvelopeData, Mode, SlotBinding, StepState};

const PAGE_SIZE: usize = 8;

pub struct RenderData {
    pub mode: Mode,
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
    pub page_groups: Vec<String>,
    pub perf_page: usize,
    pub envelope: Option<(EnvelopeData, f64)>,
    pub debug_event: Option<String>,
    pub step_focuses: Vec<Option<usize>>,
    pub step_locks: Vec<Vec<usize>>,
    pub slot_a_locked: bool,
    pub slot_b_locked: bool,
    pub cmdline: Option<String>,
    pub cmdline_error: Option<String>,
    pub cmdline_candidates: Vec<String>,
    pub leader: Option<crate::model::LeaderState>,
    pub slot_a_flash: bool,
    pub slot_b_flash: bool,
    pub help_visible: bool,
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
        match data.mode {
            Mode::Seq => render_seq_grid(frame, chunks[1], data),
            Mode::Perf => render_perf_window(frame, chunks[1], data),
        }
    }
    render_legend(frame, chunks[2], data);
    render_echo_area(frame, chunks[3], data);
    render_mode_line(frame, chunks[4], data);
}

/// Compact mode-scoped key legend, always on screen (not gated behind `?`).
/// TK1 C8 usability finding: current-mode keys must stay visible while
/// learning the layout — a toggle-only overlay hides the grid you're
/// trying to use the keys on.
fn render_legend(frame: &mut Frame, area: Rect, data: &RenderData) {
    let (line1, line2) = match data.mode {
        Mode::Seq => (
            "q..p:track  a;/z/:step  Space:play  Tab:mode  -/=:page  1-8:pattern",
            "Enter:focus  Esc:cancel  Bksp:clr-lock  y/Y:yank/paste  \\:leader  ::cmd  ?:help  ^C:quit",
        ),
        Mode::Perf => (
            "q..p:track  Space:play  Tab:mode  1-6:page  ↑↓:jog A  ←→:jog B",
            "Shift+Arrows:fine  Shift+q..p:mute  \\:leader  ::cmd  ?:help  ^C:quit",
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
    let track_name = data
        .track_names
        .get(data.active_track)
        .map(|s| s.as_str())
        .unwrap_or("?");
    let page = data.page_window + 1;
    let page_count = data.step_state.page_count.max(1);

    let transport = format!(
        " {:.1} BPM  {}  {}  P{}/{}  Step:{}  Len:{}",
        data.bpm,
        play_sym,
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
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);

    render_page_tabs(frame, chunks[0], data);
    render_envelope_section(frame, chunks[1], data);
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
    let line = tabs.join("  ");
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

fn render_help(frame: &mut Frame, area: Rect, data: &RenderData) {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::styled(
        format!(
            " MODE: {}  (? = close)",
            if data.mode == Mode::Seq {
                "SEQ"
            } else {
                "PERF"
            }
        ),
        Style::default().fg(Color::Yellow),
    ));
    lines.push(Line::from(""));

    // Global keys (all modes)
    lines.push(Line::styled(
        "── GLOBAL ──",
        Style::default().fg(Color::Cyan),
    ));
    for (key, desc) in &[
        ("q..p", "select track 1-8"),
        ("a;/z/", "toggle step (16 keys, 2 rows)"),
        ("Space", "play / stop"),
        ("Tab", "cycle mode (SEQ ↔ PERF)"),
        ("Shift+q..p", "mute / unmute track"),
        ("Shift+;", "open command line"),
        ("Enter", "focus / release step"),
        ("Esc", "release focus / cancel"),
        ("Backspace", "clear all locks on focused step"),
        ("y / Y", "yank / paste pattern"),
        ("\\", "leader (slot rebind: \\a3, \\b5)"),
        ("?", "toggle help"),
        ("Ctrl-C", "quit"),
    ] {
        lines.push(Line::styled(
            format!("  {:12}  {}", key, desc),
            Style::default().fg(Color::White),
        ));
    }
    lines.push(Line::from(""));

    if data.mode == Mode::Seq {
        lines.push(Line::styled(
            "── SEQ MODE ──",
            Style::default().fg(Color::Cyan),
        ));
        for (key, desc) in &[
            ("- / =", "page window prev / next"),
            ("1..8", "select pattern 1-8"),
        ] {
            lines.push(Line::styled(
                format!("  {:12}  {}", key, desc),
                Style::default().fg(Color::White),
            ));
        }
    } else {
        lines.push(Line::styled(
            "── PERF MODE ──",
            Style::default().fg(Color::Cyan),
        ));
        for (key, desc) in &[
            ("1..6", "select param page"),
            ("↑ / ↓", "jog slot A (coarse)"),
            ("← / →", "jog slot B (coarse)"),
            ("Shift+Arrows", "fine jog (smaller steps)"),
        ] {
            lines.push(Line::styled(
                format!("  {:12}  {}", key, desc),
                Style::default().fg(Color::White),
            ));
        }
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
        ("mode seq", "switch to SEQ mode"),
        ("mode perf", "switch to PERF mode"),
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

fn render_mode_line(frame: &mut Frame, area: Rect, data: &RenderData) {
    let mode_style = Style::default().fg(Color::Yellow);
    let mode_name = match data.mode {
        Mode::Seq => "SEQ",
        Mode::Perf => "PERF",
    };

    let mut spans = vec![
        Span::styled(format!(" {:4} ", mode_name), mode_style),
        Span::raw(" "),
        Span::raw(
            data.track_names
                .get(data.active_track)
                .map(|s| s.as_str())
                .unwrap_or("?"),
        ),
        Span::raw(" "),
    ];

    if let Some(sf) = data.step_focuses.get(data.active_track).copied().flatten() {
        spans.push(Span::raw(format!("F:s{} ", sf)));
    }

    if let Some(ref _leader) = data.leader {
        spans.push(Span::styled("\\_ ", Style::default().fg(Color::Cyan)));
    }

    if data.mode == Mode::Seq {
        let page_info = format!(
            "P{}/{}",
            data.page_window + 1,
            data.step_state.page_count.max(1)
        );
        spans.push(Span::raw(page_info));
    } else {
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
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}

impl RenderData {
    pub fn for_test(mode: Mode, track_count: u8) -> Self {
        let track_count = track_count.max(1) as usize;
        Self {
            mode,
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
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_candidates: vec![],
            leader: None,
            slot_a_flash: false,
            slot_b_flash: false,
            help_visible: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mode, StepState};

    #[test]
    fn render_seq_does_not_panic() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData {
            mode: Mode::Seq,
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
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_candidates: vec![],
            leader: None,
            slot_a_flash: false,
            slot_b_flash: false,
            help_visible: false,
        };
        terminal.draw(|f| render(f, &data)).unwrap();
    }

    #[test]
    fn render_perf_does_not_panic() {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let data = RenderData {
            mode: Mode::Perf,
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
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_candidates: vec![],
            leader: None,
            slot_a_flash: false,
            slot_b_flash: false,
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
            mode: Mode::Seq,
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
            slot_a_locked: false,
            slot_b_locked: false,
            cmdline: None,
            cmdline_error: None,
            cmdline_candidates: vec![],
            leader: None,
            slot_a_flash: false,
            slot_b_flash: false,
            help_visible: false,
        };
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &data)).unwrap();
    }
}
