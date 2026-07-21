use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::model::{Mode, StepState};

const PAGE_SIZE: usize = 8;

pub struct RenderData {
    pub mode: Mode,
    pub active_track: usize,
    pub track_names: Vec<String>,
    pub bpm: f64,
    pub playing: bool,
    pub page_window: usize,
    pub step_state: StepState,
}

pub fn render(frame: &mut Frame, data: &RenderData) {
    let area = frame.size();
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_transport(frame, chunks[0], data);
    render_seq_grid(frame, chunks[1], data);
    render_mode_line(frame, chunks[2], data);
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
    let track_count = data.track_names.len().max(1);
    let rows: Vec<Line> = (0..track_count)
        .map(|t| render_track_row(t, data))
        .collect();

    let para = Paragraph::new(rows).block(
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().fg(Color::Gray)),
    );
    frame.render_widget(para, area);
}

fn render_track_row(track_idx: usize, data: &RenderData) -> Line {
    let window = data.page_window * PAGE_SIZE;
    let mut spans: Vec<Span> = Vec::with_capacity(PAGE_SIZE + 2);

    let label = format!("{:>2}:", track_idx + 1);
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
        let is_active = data.step_state.steps.get(step).copied().unwrap_or(false);

        let (glyph, color) = if step == data.step_state.current_step {
            (" ▓", Color::Green)
        } else if is_active {
            (" █", Color::Cyan)
        } else {
            (" ░", Color::DarkGray)
        };

        spans.push(Span::styled(glyph, Style::default().fg(color)));
    }

    Line::from(spans)
}

fn render_mode_line(frame: &mut Frame, area: Rect, data: &RenderData) {
    let page_info = format!(
        "P{}/{}",
        data.page_window + 1,
        data.step_state.page_count.max(1)
    );

    let mode_style = Style::default().fg(Color::Yellow);
    let mode_name = match data.mode {
        Mode::Seq => "SEQ",
        Mode::Perf => "PERF",
    };

    let spans = vec![
        Span::styled(format!(" {:4} ", mode_name), mode_style),
        Span::raw(" "),
        Span::raw(data.track_names.get(data.active_track).map(|s| s.as_str()).unwrap_or("?")),
        Span::raw(" "),
        Span::raw(page_info),
    ];

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
}
