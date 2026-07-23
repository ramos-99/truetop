//! The process table: filtering, sorting, and the per-row styling that makes a
//! blocked process visible without reading the column.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::Style,
    text::Line,
    widgets::{Block, BorderType, Cell, Row, Table, TableState},
};

use super::{Sort, format_bytes, theme};
use crate::{backend::SystemState, metrics::ProcessMetrics};

/// Columns as `(title, width, alignment)`; [`process_row`] emits cells in this
/// order.
const COLUMNS: [(&str, Constraint, Alignment); 6] = [
    ("Pid", Constraint::Length(7), Alignment::Right),
    ("User", Constraint::Length(9), Alignment::Left),
    ("Program", Constraint::Fill(1), Alignment::Left),
    ("Cpu%", Constraint::Length(8), Alignment::Right),
    ("Mem", Constraint::Length(10), Alignment::Right),
    ("IO Wait", Constraint::Length(9), Alignment::Right),
];
pub(super) const COL_CPU: usize = 3;
pub(super) const COL_MEM: usize = 4;
pub(super) const COL_IO: usize = 5;

/// Render the table, returning how many rows it holds so the caller can clamp
/// the selection and size a page jump.
pub(super) fn draw(
    frame: &mut Frame,
    state: &SystemState,
    sort: Sort,
    descending: bool,
    filter: &str,
    selection: &mut TableState,
    area: Rect,
) -> usize {
    let mut rows: Vec<&ProcessMetrics> = state
        .processes
        .iter()
        .filter(|p| matches_filter(p, filter))
        .collect();
    match sort {
        Sort::Cpu => rows.sort_by(|a, b| b.cpu.cpu_percent.total_cmp(&a.cpu.cpu_percent)),
        Sort::Mem => rows.sort_by_key(|p| std::cmp::Reverse(mem_of(p))),
        Sort::Io => rows.sort_by(|a, b| io_of(b).total_cmp(&io_of(a))),
    }
    if !descending {
        rows.reverse();
    }

    let arrow = if descending { "▾" } else { "▴" };
    let header = Row::new(COLUMNS.iter().enumerate().map(|(i, (title, _, align))| {
        let text = if i == sort.column() {
            format!("{title} {arrow}")
        } else {
            (*title).to_owned()
        };
        Cell::from(Line::from(text).alignment(*align))
    }))
    .style(theme::header());

    let title = if filter.is_empty() {
        " processes ".to_owned()
    } else {
        format!(" processes matching \"{filter}\" ")
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::DIM))
        .title_top(Line::from(ratatui::text::Span::styled(
            title,
            theme::header(),
        )));

    let body: Vec<Row> = rows
        .iter()
        .enumerate()
        .map(|(index, p)| process_row(index, p, state.memory_total_bytes))
        .collect();
    let count = body.len();
    frame.render_stateful_widget(
        Table::new(body, COLUMNS.map(|(_, width, _)| width))
            .header(header)
            .block(block)
            .row_highlight_style(theme::selected())
            .column_spacing(1),
        area,
        selection,
    );
    count
}

fn matches_filter(p: &ProcessMetrics, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_lowercase();
    p.name.to_lowercase().contains(&needle) || p.pid.to_string().contains(&needle)
}

fn process_row(index: usize, p: &ProcessMetrics, memory_total: u64) -> Row<'static> {
    let [pid_a, user_a, prog_a, cpu_a, mem_a, io_a] = COLUMNS.map(|(_, _, align)| align);
    let idle = p.cpu.cpu_percent < 0.05;
    let name_style = Style::new().fg(if idle { theme::DIM } else { theme::TEXT });

    let row = Row::new([
        cell(p.pid.to_string(), pid_a, name_style),
        cell(
            p.user.clone().unwrap_or_else(|| "-".into()),
            user_a,
            Style::new().fg(theme::DIM),
        ),
        cell(p.name.clone(), prog_a, name_style),
        cell(
            format!("{:.1}", p.cpu.cpu_percent),
            cpu_a,
            theme::cpu_heat(p.cpu.cpu_percent),
        ),
        cell(mem_text(p), mem_a, theme::mem_heat(mem_of(p), memory_total)),
        cell(io_text(p), io_a, io_style(p)),
    ]);
    // A blocked process outranks the striping that only groups rows.
    match theme::io_row_bg(io_of(p)).or_else(|| theme::row_stripe(index)) {
        Some(bg) => row.style(Style::new().bg(bg)),
        None => row,
    }
}

fn io_of(p: &ProcessMetrics) -> f64 {
    p.io.map_or(0.0, |io| io.io_wait_percent)
}

fn io_text(p: &ProcessMetrics) -> String {
    p.io.map_or_else(|| "-".into(), |io| format!("{:.1}", io.io_wait_percent))
}

fn io_style(p: &ProcessMetrics) -> Style {
    p.io.map_or(Style::new().fg(theme::DIM), |io| {
        theme::io_heat(io.io_wait_percent)
    })
}

fn mem_of(p: &ProcessMetrics) -> u64 {
    p.mem.map_or(0, |m| m.rss_bytes)
}

fn mem_text(p: &ProcessMetrics) -> String {
    p.mem
        .map_or_else(|| "-".into(), |m| format_bytes(m.rss_bytes))
}

fn cell(text: String, align: Alignment, style: Style) -> Cell<'static> {
    Cell::from(Line::from(text).alignment(align)).style(style)
}
