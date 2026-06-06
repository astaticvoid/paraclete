// SPDX-License-Identifier: GPL-3.0-or-later

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::state::TuiState;

pub fn render(frame: &mut Frame, state: &TuiState) {
    let area = frame.size();
    if area.width < 40 {
        let msg = Paragraph::new("Terminal too narrow (min 40 cols)")
            .style(Style::default().fg(Color::Red));
        frame.render_widget(msg, area);
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    render_transport(frame, sections[0], state);
    render_encoders(frame, sections[1], state);
    render_steps(frame, sections[2], state);
}

fn render_transport(frame: &mut Frame, area: Rect, state: &TuiState) {
    let play_sym = if state.playing { "\u{25B6} PLAYING" } else { "\u{25A0} STOPPED" };
    let text = format!(
        " \u{266A} {:.1} BPM   {}   Step: {} / 16   Track {}",
        state.bpm,
        play_sym,
        state.current_step + 1,
        state.active_track + 1,
    );
    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(para, area);
}

fn render_encoders(frame: &mut Frame, area: Rect, state: &TuiState) {
    let encoders = &state.encoders;
    if encoders.is_empty() {
        let para = Paragraph::new(" No encoders mapped")
            .block(Block::default().borders(Borders::ALL).title("Encoders"));
        frame.render_widget(para, area);
        return;
    }

    let slots_per_row = if area.width < 60 { 4_usize } else { 8_usize };
    let cols = slots_per_row.min(encoders.len());
    let col_width = area.width / cols as u16;

    let outer = Block::default().borders(Borders::ALL).title("Encoders");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let constraints: Vec<Constraint> = (0..cols).map(|_| Constraint::Length(col_width)).collect();
    let col_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(inner);

    for (i, slot) in encoders.iter().take(cols).enumerate() {
        render_encoder_slot(frame, col_areas[i], slot, i);
    }
}

fn render_encoder_slot(
    frame: &mut Frame,
    area: Rect,
    slot: &crate::state::EncoderSlot,
    idx: usize,
) {
    let border_style = if slot.recently_changed {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let label = if slot.label.is_empty() {
        format!("E{}", idx)
    } else {
        format!("E{}: {}", idx, slot.label)
    };

    let fill_ratio = if (slot.max - slot.min).abs() < f64::EPSILON {
        0.0_f64
    } else {
        ((slot.value - slot.min) / (slot.max - slot.min)).clamp(0.0, 1.0)
    };

    let bar_width = (area.width.saturating_sub(2)) as usize;
    let filled = ((fill_ratio * bar_width as f64) as usize).min(bar_width);
    let bar: String = "\u{2593}".repeat(filled) + &"\u{2591}".repeat(bar_width - filled);

    let value_str = format_value(slot);

    let lines = vec![
        Line::from(Span::styled(label, Style::default().add_modifier(Modifier::BOLD))),
        Line::from(Span::raw(format!("[{}]", bar))),
        Line::from(Span::raw(value_str)),
    ];

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).border_style(border_style));
    frame.render_widget(para, area);
}

fn format_value(slot: &crate::state::EncoderSlot) -> String {
    format!("{:.3}", slot.value)
}

fn render_steps(frame: &mut Frame, area: Rect, state: &TuiState) {
    let mut cells: Vec<Span> = Vec::with_capacity(32);
    for (i, &active) in state.steps.iter().enumerate() {
        let sym = if active { "\u{25A0}" } else { "\u{00B7}" };
        let style = if i == state.current_step as usize && state.playing {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        cells.push(Span::styled(format!("{} ", sym), style));
    }
    cells.push(Span::raw(format!("[{}]", state.current_step + 1)));

    let line = Line::from(cells);
    let para = Paragraph::new(vec![line])
        .block(Block::default().borders(Borders::ALL).title("Steps"));
    frame.render_widget(para, area);
}
