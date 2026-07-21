use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
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
    pub slot_a: Option<SlotBinding>,
    pub slot_a_value: f64,
    pub slot_b: Option<SlotBinding>,
    pub slot_b_value: f64,
    pub page_groups: Vec<String>,
    pub perf_page: usize,
    pub envelope: Option<(EnvelopeData, f64)>,
    pub debug_event: Option<String>,
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
    match data.mode {
        Mode::Seq => render_seq_grid(frame, chunks[1], data),
        Mode::Perf => render_perf_window(frame, chunks[1], data),
    }
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

fn render_track_row(track_idx: usize, data: &RenderData) -> Line<'_> {
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

fn render_perf_window(frame: &mut Frame, area: Rect, data: &RenderData) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .split(area);

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
    let para = Paragraph::new(line)
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}

fn render_envelope_section(frame: &mut Frame, area: Rect, data: &RenderData) {
    if let Some((ref env, val)) = &data.envelope {
        let chunks = Layout::horizontal([
            Constraint::Length(14),
            Constraint::Min(0),
        ])
        .split(area);

        let label_span = Span::styled(
            format!(" {} ", env.param_name),
            Style::default(),
        );
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

fn render_mode_line(frame: &mut Frame, area: Rect, data: &RenderData) {
    let mode_style = Style::default().fg(Color::Yellow);
    let mode_name = match data.mode {
        Mode::Seq => "SEQ",
        Mode::Perf => "PERF",
    };

    let mut spans = vec![
        Span::styled(format!(" {:4} ", mode_name), mode_style),
        Span::raw(" "),
        Span::raw(data.track_names.get(data.active_track).map(|s| s.as_str()).unwrap_or("?")),
        Span::raw(" "),
    ];

    if data.mode == Mode::Seq {
        let page_info = format!(
            "P{}/{}",
            data.page_window + 1,
            data.step_state.page_count.max(1)
        );
        spans.push(Span::raw(page_info));
    } else {
        let a_text = match &data.slot_a {
            Some(s) => format!(" A:{}={:.3}", s.param_name, data.slot_a_value),
            None => " A:--".to_string(),
        };
        let b_text = match &data.slot_b {
            Some(s) => format!(" B:{}={:.3}", s.param_name, data.slot_b_value),
            None => " B:--".to_string(),
        };
        spans.push(Span::raw(format!("{} {}", a_text, b_text)));
    }

    let line = Line::from(spans);
    let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
    frame.render_widget(para, area);
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
                page_count: 2,
            },
            slot_a: None,
            slot_a_value: 0.0,
            slot_b: None,
            slot_b_value: 0.0,
            page_groups: vec![],
            perf_page: 0,
            envelope: None,
            debug_event: None,
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
            envelope: Some((EnvelopeData {
                param_id: 1,
                param_name: "decay".into(),
                node_id: 20,
                env_type: "AD".into(),
                min: 0.0,
                max: 1.0,
            }, 0.42)),
            debug_event: None,
        };
        terminal.draw(|f| render(f, &data)).unwrap();
    }
}
