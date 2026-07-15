// SPDX-License-Identifier: GPL-3.0-or-later

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
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
    let play_sym = if state.playing {
        "\u{25B6} PLAYING"
    } else {
        "\u{25A0} STOPPED"
    };
    let text = format!(
        " \u{266A} {:.1} BPM   {}   Step: {} / {}   Track {}",
        state.bpm,
        play_sym,
        state.current_step as usize + 1,
        state.pattern_length,
        state.active_track + 1,
    );
    let para = Paragraph::new(text).block(Block::default().borders(Borders::ALL));
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
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
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
        Line::from(Span::styled(
            label,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw(format!("[{}]", bar))),
        Line::from(Span::raw(value_str)),
    ];

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    frame.render_widget(para, area);
}

fn format_value(slot: &crate::state::EncoderSlot) -> String {
    format!("{:.3}", slot.value)
}

fn render_steps(frame: &mut Frame, area: Rect, state: &TuiState) {
    let mut cells: Vec<Span> = Vec::with_capacity(40);
    // Playhead marker is window-relative: the row shows the 16-step window
    // containing the playhead (P10 C5; patterns reach 64 steps).
    let marker = (state.current_step as usize).checked_sub(state.window_base);
    for (i, &active) in state.steps.iter().enumerate() {
        let sym = if active { "\u{25A0}" } else { "\u{00B7}" };
        let style = if marker == Some(i) && state.playing {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        cells.push(Span::styled(format!("{} ", sym), style));
    }
    cells.push(Span::raw(format!("[{}]", state.current_step as usize + 1)));

    // Pattern / page / speed indicator (P10 C5, spec 5.2): P{n} blinks
    // while a different pattern is cued; speed shown only when != 1x.
    let cued_differs =
        state.cued_pattern >= 0 && state.cued_pattern as usize != state.active_pattern;
    let p_style = if cued_differs {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK)
    } else {
        Style::default().fg(Color::Cyan)
    };
    cells.push(Span::styled(
        format!("  P{}", state.active_pattern + 1),
        p_style,
    ));
    if cued_differs {
        cells.push(Span::styled(
            format!("\u{2192}P{}", state.cued_pattern + 1),
            p_style,
        ));
    }
    // The label names the page(s) the 16-step row is SHOWING (a window is
    // two 8-step pages), so label and visible steps can never disagree —
    // the playhead's own page is readable from the marker position.
    let page_count = state.page_count.max(1);
    let first_page = (state.window_base / 8) + 1;
    let last_page = (first_page + 1).min(page_count);
    if page_count > 2 {
        cells.push(Span::raw(format!(
            "  pg {first_page}-{last_page}/{page_count}"
        )));
    } else {
        cells.push(Span::raw(format!(
            "  pg {}/{}",
            state.current_page + 1,
            page_count
        )));
    }
    if state.speed_mult != 1.0 {
        cells.push(Span::styled(
            format!("  \u{00D7}{}", format_speed(state.speed_mult)),
            Style::default().fg(Color::Magenta),
        ));
    }

    let line = Line::from(cells);
    let para =
        Paragraph::new(vec![line]).block(Block::default().borders(Borders::ALL).title("Steps"));
    frame.render_widget(para, area);
}

/// "2", "0.5", "1.33" — trailing zeros trimmed so the common multipliers
/// read like hardware labels.
fn format_speed(mult: f64) -> String {
    let s = format!("{mult:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
