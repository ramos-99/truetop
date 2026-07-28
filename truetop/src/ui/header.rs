//! Everything above the process table: the identity line, the trend charts and
//! the summary gauges.

use std::collections::VecDeque;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{Axis, Block, BorderType, Chart, Dataset, GraphType, Padding},
};

use super::theme;
use crate::{backend::SystemState, system::Machine, ui::format_bytes};

const GAUGE_WIDTH: usize = 18;

/// Wordmark with the radar glyph standing in for the `o`, as in the logo.
const BRAND: (&str, &str, &str) = ("truet", "◎", "p");

pub(super) fn draw_identity(frame: &mut Frame, machine: &Machine, area: Rect) {
    let specs = format!(
        "{} · {} · {} cores · {}",
        machine.hostname,
        machine.cpu_model,
        machine.cores,
        format_bytes(machine.memory_total_bytes)
    );
    // Three lines over one row: wordmark left, specs centred, clock right.
    let wordmark = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            BRAND.0,
            Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            BRAND.1,
            Style::new().fg(theme::IO).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            BRAND.2,
            Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(wordmark, area);
    frame.render_widget(
        Line::from(Span::styled(specs, Style::new().fg(theme::DIM))).alignment(Alignment::Center),
        area,
    );
    frame.render_widget(
        Line::from(Span::styled(
            format!("{} ", clock()),
            Style::new().fg(theme::ACCENT),
        ))
        .alignment(Alignment::Right),
        area,
    );
}

pub(super) fn draw_charts(frame: &mut Frame, state: &SystemState, area: Rect) {
    let halves =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    chart(
        frame,
        &state.cpu_history,
        "cpu",
        format!("{:.1}%", state.total_cpu_percent),
        theme::CPU,
        halves[0],
    );
    chart(
        frame,
        &state.io_history,
        "io wait",
        format!("{:.1}% summed over processes", state.total_io_percent),
        theme::IO,
        halves[1],
    );
}

pub(super) fn draw_summary(frame: &mut Frame, state: &SystemState, area: Rect) {
    let mut spans = Vec::new();
    gauge(&mut spans, "cpu", state.total_cpu_percent, theme::CPU);
    spans.push(Span::raw("  "));
    let memory_percent = if state.memory_total_bytes == 0 {
        0.0
    } else {
        state.memory_used_bytes as f64 / state.memory_total_bytes as f64 * 100.0
    };
    gauge(&mut spans, "mem", memory_percent, Color::Cyan);
    spans.push(Span::styled(
        format!(
            " {} of {}",
            format_bytes(state.memory_used_bytes),
            format_bytes(state.memory_total_bytes)
        ),
        Style::new().fg(theme::DIM),
    ));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        format!("load {:.2}", state.load_average),
        Style::new().fg(theme::DIM),
    ));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        format!("{} tasks", state.tracked),
        Style::new().fg(theme::DIM),
    ));
    frame.render_widget(Line::from(spans), area);
}

fn chart(
    frame: &mut Frame,
    history: &VecDeque<f64>,
    label: &str,
    value: String,
    colour: Color,
    area: Rect,
) {
    let title = Line::from(vec![
        Span::styled(
            format!(" {label} "),
            Style::new().fg(colour).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{value} "), Style::new().fg(theme::DIM)),
    ]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::DIM))
        .padding(Padding::right(1))
        .title_top(title);

    let data: Vec<(f64, f64)> = history
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v))
        .collect();
    if data.is_empty() {
        frame.render_widget(block, area);
        return;
    }
    // Span the samples actually held, so the series fills the panel rather than
    // drawing into a fixed axis it has not caught up with yet.
    let x_max = (data.len() - 1).max(1) as f64;
    let y_max = data.iter().map(|&(_, y)| y).fold(1.0, f64::max) * 1.1;

    let dataset = Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(colour))
        .data(&data);
    frame.render_widget(
        Chart::new(vec![dataset])
            .block(block)
            .x_axis(Axis::default().bounds([0.0, x_max]))
            .y_axis(Axis::default().bounds([0.0, y_max])),
        area,
    );
}

fn gauge(out: &mut Vec<Span<'static>>, label: &str, percent: f64, colour: Color) {
    let filled = ((percent / 100.0).clamp(0.0, 1.0) * GAUGE_WIDTH as f64).round() as usize;
    out.push(Span::styled(
        format!(" {label} "),
        Style::new().fg(colour).add_modifier(Modifier::BOLD),
    ));
    out.push(Span::styled("━".repeat(filled), Style::new().fg(colour)));
    out.push(Span::styled(
        "─".repeat(GAUGE_WIDTH - filled),
        Style::new().fg(theme::DIM),
    ));
    out.push(Span::styled(
        format!(" {percent:.1}%"),
        Style::new().fg(theme::TEXT),
    ));
}

fn clock() -> String {
    // SAFETY: `time` accepts a null pointer, and `localtime_r` fills a tm we own.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    /// The clock shares this row and moves every second, so the assertion covers
    /// the left of the line rather than the whole of it.
    #[test]
    fn the_identity_line_states_the_host_it_was_given() {
        let machine = Machine {
            hostname: "testhost".into(),
            cpu_model: "Test CPU".into(),
            cores: 8,
            memory_total_bytes: 16 << 30,
        };

        let mut terminal = Terminal::new(TestBackend::new(90, 1)).expect("test terminal");
        terminal
            .draw(|frame| draw_identity(frame, &machine, frame.area()))
            .expect("draw");
        let line: String = (0..90)
            .filter_map(|x| terminal.backend().buffer().cell((x, 0)).map(|c| c.symbol()))
            .collect();

        assert!(line.starts_with(" truet◎p "), "{line}");
        assert!(
            line.contains("testhost · Test CPU · 8 cores · 16.0 G"),
            "{line}"
        );
    }
}
