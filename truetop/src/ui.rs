//! Renderer thread (frontend).
//!
//! The consume side of the double-buffer (`CLAUDE.md` §3). It is fully isolated
//! from the backend: it only ever does an atomic `load()` of the current
//! snapshot and feeds references into ratatui. It performs *no* collection,
//! allocation of state, or blocking I/O against the kernel.
//!
//! Because this is the boundary that will be heavily iterated on visually, it
//! depends on nothing from `backend` except the read-only [`SystemState`]
//! shape. Swapping the look of the table never touches the collector.

use std::{
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
    layout::{Alignment, Constraint},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Text},
    widgets::{Block, BorderType, Cell, Row, Table},
};

use crate::{backend::SystemState, metrics::ProcessMetrics};

/// Poll timeout that paces the event loop at ~60 fps (16 ms ≈ 62.5 Hz).
const FRAME_BUDGET: Duration = Duration::from_millis(16);

/// Accent colour for the frame and header.
const ACCENT: Color = Color::Cyan;

/// Table columns as `(title, width, alignment)` — one source of truth for the
/// header and the row layout. [`process_row`] emits cells in this order.
const COLUMNS: [(&str, Constraint, Alignment); 4] = [
    ("PID", Constraint::Length(7), Alignment::Right),
    ("CPU%", Constraint::Length(6), Alignment::Right),
    ("MEM", Constraint::Length(9), Alignment::Right),
    ("COMMAND", Constraint::Fill(1), Alignment::Left),
];

/// Run the render loop on the calling (main) thread until the user quits or
/// `running` is cleared by an external signal handler.
///
/// `state` is the shared snapshot pointer published by the collector. The loop
/// re-`load()`s every frame, so the UI tracks whatever the backend last stored
/// regardless of the relative frequencies of the two threads.
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
    while running.load(Ordering::Relaxed) {
        // Atomic, lock-free read of the current immutable snapshot.
        let snapshot = state.load_full();
        terminal.draw(|frame| draw(frame, &snapshot))?;

        // Event-driven wake-up; the timeout caps the redraw cadence.
        if event::poll(FRAME_BUDGET)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        {
            running.store(false, Ordering::Relaxed);
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, state: &SystemState) {
    let header = Row::new(COLUMNS.map(|(title, _, align)| header_cell(title, align))).style(
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
    );

    let widths = COLUMNS.map(|(_, width, _)| width);
    let rows = state.processes.iter().map(process_row);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .title_top(Line::from(" truetop ".fg(ACCENT).bold()))
        .title_top(
            Line::from(format!(" {} procs ", state.processes.len()))
                .right_aligned()
                .dim(),
        )
        .title_bottom(Line::from(" q/esc quit ".dim()).right_aligned());

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(2);

    frame.render_widget(table, frame.area());
}

fn header_cell(title: &'static str, align: Alignment) -> Cell<'static> {
    Cell::from(Text::from(title).alignment(align))
}

/// One process → one table row, in [`COLUMNS`] order. Formatting and styling are
/// lazy here, only for drawn rows (renderer contract, CLAUDE.md §3).
fn process_row(p: &ProcessMetrics) -> Row<'static> {
    let [pid, cpu, mem, cmd] = COLUMNS.map(|(_, _, align)| align);
    Row::new([
        cell(p.pid.to_string(), pid, Style::new().dim()),
        cell(
            format!("{:.1}", p.cpu.cpu_percent),
            cpu,
            cpu_style(p.cpu.cpu_percent),
        ),
        cell(mem_text(p), mem, mem_style(p)),
        cell(p.name.clone(), cmd, Style::new()),
    ])
}

fn cell(text: String, align: Alignment, style: Style) -> Cell<'static> {
    Cell::from(Text::from(text).alignment(align)).style(style)
}

/// Cool→hot gradient so busy processes stand out; idle ones recede.
fn cpu_style(percent: f64) -> Style {
    let colour = if percent < 0.05 {
        Color::DarkGray
    } else if percent < 25.0 {
        Color::Green
    } else if percent < 60.0 {
        Color::Yellow
    } else {
        Color::Red
    };
    Style::new().fg(colour)
}

fn mem_text(p: &ProcessMetrics) -> String {
    p.mem
        .map_or_else(|| "—".to_owned(), |m| format_bytes(m.rss_bytes))
}

fn mem_style(p: &ProcessMetrics) -> Style {
    match p.mem {
        Some(m) if m.rss_bytes > 0 => Style::new(),
        _ => Style::new().dim(),
    }
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
