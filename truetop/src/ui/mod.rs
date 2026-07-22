//! Renderer (frontend) - the consume side of the double-buffer (CLAUDE.md §3).
//!
//! Fully isolated from the backend: it only `load()`s the current immutable
//! snapshot of raw values, sorts and formats it, and feeds ratatui. It does no
//! collection and no blocking I/O against the kernel. Formatting happens here,
//! lazily, and the loop only redraws when the snapshot or the input changes -
//! an idle truetop costs nothing.

mod theme;

use std::{
    collections::VecDeque,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Axis, Block, BorderType, Cell, Chart, Dataset, GraphType, Padding, Row, Table, TableState,
    },
};

use crate::{backend::SystemState, metrics::ProcessMetrics};

/// Poll timeout that caps the redraw cadence at ~60 fps.
const FRAME_BUDGET: Duration = Duration::from_millis(16);
/// Chart x-span, in samples (matches the backend history depth).
const HISTORY_SPAN: f64 = 120.0;
const GAUGE_WIDTH: usize = 20;

/// Table columns as `(title, width, alignment)`. [`process_row`] emits cells in
/// this order; `Cpu%` and `IO Wait` are the sortable ones.
const COLUMNS: [(&str, Constraint, Alignment); 6] = [
    ("Pid", Constraint::Length(7), Alignment::Right),
    ("User", Constraint::Length(9), Alignment::Left),
    ("Program", Constraint::Fill(1), Alignment::Left),
    ("Cpu%", Constraint::Length(8), Alignment::Right),
    ("Mem", Constraint::Length(10), Alignment::Right),
    ("IO Wait", Constraint::Length(9), Alignment::Right),
];
const COL_CPU: usize = 3;
const COL_MEM: usize = 4;
const COL_IO: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Cpu,
    Mem,
    Io,
}

impl Sort {
    fn column(self) -> usize {
        match self {
            Self::Cpu => COL_CPU,
            Self::Mem => COL_MEM,
            Self::Io => COL_IO,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Mem => "mem",
            Self::Io => "io",
        }
    }
}

/// Renderer-owned view state: which row is selected and which column sorts.
struct App {
    table: TableState,
    sort: Sort,
}

impl App {
    fn new() -> Self {
        Self {
            table: TableState::default().with_selected(0),
            sort: Sort::Cpu,
        }
    }

    /// Apply a keypress; returns `true` if the user asked to quit.
    fn handle_key(&mut self, code: KeyCode, rows: usize) -> bool {
        let last = rows.saturating_sub(1);
        let current = self.table.selected().unwrap_or(0);
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.select(if current >= last { 0 } else { current + 1 })
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select(if current == 0 { last } else { current - 1 })
            }
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(last),
            KeyCode::Char('c') => self.set_sort(Sort::Cpu),
            KeyCode::Char('m') => self.set_sort(Sort::Mem),
            KeyCode::Char('i') => self.set_sort(Sort::Io),
            _ => {}
        }
        false
    }

    fn select(&mut self, i: usize) {
        self.table.select(Some(i));
    }

    fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.select(0);
    }
}

// ── Entry & loop ─────────────────────────────────────────────────────────────

pub fn render_app(state: Arc<ArcSwap<SystemState>>, running: Arc<AtomicBool>) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &state, &running);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    state: &ArcSwap<SystemState>,
    running: &AtomicBool,
) -> io::Result<()> {
    let mut app = App::new();
    let mut last: *const SystemState = std::ptr::null();

    while running.load(Ordering::Relaxed) {
        let snapshot = state.load_full();
        let mut redraw = !std::ptr::eq(Arc::as_ptr(&snapshot), last);
        last = Arc::as_ptr(&snapshot);

        if event::poll(FRAME_BUDGET)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.handle_key(key.code, snapshot.processes.len()) {
                        running.store(false, Ordering::Relaxed);
                    }
                    redraw = true;
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }

        if redraw {
            terminal.draw(|frame| draw(frame, &snapshot, &mut app))?;
        }
    }
    Ok(())
}

// ── Layout ───────────────────────────────────────────────────────────────────

fn draw(frame: &mut Frame, state: &SystemState, app: &mut App) {
    let rows = Layout::vertical([
        Constraint::Length(9),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    draw_charts(frame, state, rows[0]);
    draw_summary(frame, state, rows[1]);
    draw_table(frame, state, app, rows[2]);
    draw_status(frame, app.sort, rows[3]);
}

// ── Trend charts ─────────────────────────────────────────────────────────────

fn draw_charts(frame: &mut Frame, state: &SystemState, area: Rect) {
    let halves =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    draw_line_chart(
        frame,
        &state.cpu_history,
        "cpu",
        state.total_cpu_percent,
        theme::CPU,
        halves[0],
    );
    draw_line_chart(
        frame,
        &state.io_history,
        "io wait",
        state.total_io_percent,
        theme::IO,
        halves[1],
    );
}

fn draw_line_chart(
    frame: &mut Frame,
    history: &VecDeque<f64>,
    label: &str,
    now: f64,
    colour: ratatui::style::Color,
    area: Rect,
) {
    let title = Line::from(vec![
        Span::styled(
            format!(" {label} "),
            Style::new().fg(colour).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{now:.1}% "), Style::new().fg(theme::DIM)),
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
    let y_max = data.iter().map(|&(_, y)| y).fold(1.0, f64::max) * 1.1;

    let dataset = Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(colour))
        .data(&data);
    let chart = Chart::new(vec![dataset])
        .block(block)
        .x_axis(Axis::default().bounds([0.0, HISTORY_SPAN]))
        .y_axis(Axis::default().bounds([0.0, y_max]));
    frame.render_widget(chart, area);
}

// ── Summary line ─────────────────────────────────────────────────────────────

fn draw_summary(frame: &mut Frame, state: &SystemState, area: Rect) {
    let mut spans = Vec::new();
    gauge(&mut spans, "cpu", state.total_cpu_percent, theme::CPU);
    spans.push(Span::raw("   "));
    gauge(&mut spans, "io", state.total_io_percent, theme::IO);
    spans.push(Span::raw("   "));
    spans.push(Span::styled(
        format!("{} tasks", state.processes.len()),
        Style::new().fg(theme::DIM),
    ));
    frame.render_widget(Line::from(spans), area);
}

fn gauge(out: &mut Vec<Span<'static>>, label: &str, percent: f64, colour: ratatui::style::Color) {
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

// ── Process table ────────────────────────────────────────────────────────────

fn draw_table(frame: &mut Frame, state: &SystemState, app: &mut App, area: Rect) {
    let mut sorted: Vec<&ProcessMetrics> = state.processes.iter().collect();
    match app.sort {
        Sort::Cpu => sorted.sort_by(|a, b| b.cpu.cpu_percent.total_cmp(&a.cpu.cpu_percent)),
        Sort::Mem => sorted.sort_by_key(|p| std::cmp::Reverse(mem_of(p))),
        Sort::Io => sorted.sort_by(|a, b| io_of(b).total_cmp(&io_of(a))),
    }

    let active = app.sort.column();
    let header = Row::new(COLUMNS.iter().enumerate().map(|(i, (title, _, align))| {
        let text = if i == active {
            format!("{title} ▾")
        } else {
            (*title).to_owned()
        };
        Cell::from(Line::from(text).alignment(*align))
    }))
    .style(theme::header());

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::DIM))
        .title_top(Line::from(Span::styled(" processes ", theme::header())));

    let table = Table::new(
        sorted.iter().map(|p| process_row(p)),
        COLUMNS.map(|(_, w, _)| w),
    )
    .header(header)
    .block(block)
    .row_highlight_style(theme::selected())
    .column_spacing(1);
    frame.render_stateful_widget(table, area, &mut app.table);
}

fn process_row<'a>(p: &'a ProcessMetrics) -> Row<'a> {
    let [pid_a, user_a, prog_a, cpu_a, mem_a, io_a] = COLUMNS.map(|(_, _, a)| a);
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
        cell(mem_text(p), mem_a, mem_style(p)),
        cell(io_text(p), io_a, io_style(p)),
    ]);
    match theme::io_row_bg(io_of(p)) {
        Some(bg) => row.style(Style::new().bg(bg)),
        None => row,
    }
}

fn draw_status(frame: &mut Frame, sort: Sort, area: Rect) {
    let key = |k: &'static str, desc: &'static str| {
        [
            Span::styled(k, Style::new().fg(theme::ACCENT)),
            Span::styled(format!(" {desc}   "), Style::new().fg(theme::DIM)),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key("q", "quit"));
    spans.extend(key("↑↓", "nav"));
    spans.extend(key("c", "sort cpu"));
    spans.extend(key("m", "sort mem"));
    spans.extend(key("i", "sort io"));
    spans.push(Span::styled(
        format!("│ by {}", sort.label()),
        Style::new().fg(theme::DIM),
    ));
    frame.render_widget(Line::from(spans), area);
}

// ── Cell formatting ──────────────────────────────────────────────────────────

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

fn mem_style(p: &ProcessMetrics) -> Style {
    match p.mem {
        Some(m) if m.rss_bytes > 0 => Style::new().fg(theme::CPU),
        _ => Style::new().fg(theme::DIM),
    }
}

fn cell<'a>(text: String, align: Alignment, style: Style) -> Cell<'a> {
    Cell::from(Line::from(text).alignment(align)).style(style)
}

/// Render a byte count as a compact human-readable string (e.g. `12.3 M`).
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_keeps_raw_count_below_one_kib() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_scales_to_higher_units() {
        assert_eq!(format_bytes(1024), "1.0 K");
        assert_eq!(format_bytes(1536), "1.5 K");
        assert_eq!(format_bytes(1024 * 1024), "1.0 M");
    }

    #[test]
    fn format_bytes_caps_at_terabytes() {
        assert!(format_bytes(u64::MAX).ends_with(" T"));
    }
}
