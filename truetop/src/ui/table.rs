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
    let rows = visible(&state.processes, sort, descending, filter);

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

/// The rows this frame will hold, filtered then ordered by the active column. A
/// process the kernel gave no RSS or I/O wait ranks as zero rather than dropping
/// out, which is what the `-` cells stand for.
fn visible<'a>(
    processes: &'a [ProcessMetrics],
    sort: Sort,
    descending: bool,
    filter: &str,
) -> Vec<&'a ProcessMetrics> {
    let mut rows: Vec<&ProcessMetrics> = processes
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
    rows
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

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::metrics::{CpuMetrics, IoMetrics, MemMetrics};

    /// Ranked differently by every sort key, so an assertion on one order cannot
    /// pass under another. `cc1plus` has no I/O wait and `sleep` no RSS.
    fn processes() -> Vec<ProcessMetrics> {
        vec![
            row(10, "fio", 5.0, Some(2 << 20), Some(90.0)),
            row(20, "cc1plus", 80.0, Some(512 << 20), None),
            row(31, "chrome", 0.5, Some(900 << 20), Some(1.0)),
            row(42, "sleep", 0.0, None, Some(0.2)),
        ]
    }

    fn row(
        pid: u32,
        name: &str,
        cpu_percent: f64,
        rss_bytes: Option<u64>,
        io_wait_percent: Option<f64>,
    ) -> ProcessMetrics {
        ProcessMetrics {
            pid,
            name: name.to_owned(),
            user: Some("ramos".into()),
            cpu: CpuMetrics { cpu_percent },
            mem: rss_bytes.map(|rss_bytes| MemMetrics { rss_bytes }),
            io: io_wait_percent.map(|io_wait_percent| IoMetrics { io_wait_percent }),
        }
    }

    fn pids(sort: Sort, descending: bool, filter: &str) -> Vec<u32> {
        visible(&processes(), sort, descending, filter)
            .iter()
            .map(|p| p.pid)
            .collect()
    }

    #[test]
    fn each_sort_key_ranks_by_its_own_column() {
        assert_eq!(pids(Sort::Cpu, true, ""), [20, 10, 31, 42]);
        assert_eq!(pids(Sort::Mem, true, ""), [31, 20, 10, 42]);
        assert_eq!(pids(Sort::Io, true, ""), [10, 31, 42, 20]);
    }

    #[test]
    fn ascending_reverses_the_ranking() {
        assert_eq!(pids(Sort::Cpu, false, ""), [42, 31, 10, 20]);
    }

    #[test]
    fn the_filter_matches_a_name_case_insensitively_or_a_pid_by_substring() {
        assert_eq!(pids(Sort::Cpu, true, "FIO"), [10]);
        assert_eq!(pids(Sort::Cpu, true, "3"), [31]);
        assert!(pids(Sort::Cpu, true, "zzz").is_empty());
    }

    const BORDER: &str = "│╭╮╰╯─";

    /// Every line of a rendered frame, borders and padding squeezed out so a row
    /// reads as the cells it holds.
    fn render(sort: Sort, descending: bool, filter: &str) -> Vec<String> {
        let state = SystemState {
            processes: processes(),
            memory_total_bytes: 16 << 30,
            ..SystemState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(64, 8)).expect("test terminal");
        let mut selection = TableState::default();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &state,
                    sort,
                    descending,
                    filter,
                    &mut selection,
                    frame.area(),
                );
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..buffer.area().height)
            .map(|y| {
                let line: String = (0..buffer.area().width)
                    // Not `Cell::symbol`: `Cell` here is the table widget's.
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol()))
                    .collect();
                line.replace(|c| BORDER.contains(c), " ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    fn line_for<'a>(lines: &'a [String], name: &str) -> &'a str {
        lines
            .iter()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("no rendered row for {name}: {lines:#?}"))
    }

    #[test]
    fn a_process_the_kernel_gave_no_value_for_renders_a_dash() {
        let lines = render(Sort::Cpu, true, "");
        assert_eq!(line_for(&lines, "sleep"), "42 ramos sleep 0.0 - 0.2");
        assert_eq!(
            line_for(&lines, "cc1plus"),
            "20 ramos cc1plus 80.0 512.0 M -"
        );
    }

    #[test]
    fn only_the_active_column_carries_the_sort_arrow() {
        let header = |sort, descending| render(sort, descending, "")[1].clone();
        assert!(header(Sort::Cpu, true).contains("Cpu% ▾"));
        assert!(header(Sort::Cpu, false).contains("Cpu% ▴"));
        let by_memory = header(Sort::Mem, true);
        assert!(by_memory.contains("Mem ▾"), "{by_memory}");
        assert!(!by_memory.contains("Cpu% ▾"), "{by_memory}");
    }

    #[test]
    fn the_title_names_the_active_filter() {
        assert!(render(Sort::Cpu, true, "")[0].contains("processes"));
        assert!(render(Sort::Cpu, true, "fio")[0].contains("processes matching \"fio\""));
    }
}
