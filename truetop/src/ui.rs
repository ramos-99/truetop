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
    layout::Constraint,
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Cell, Row, Table},
};

use crate::backend::SystemState;

/// Poll timeout that paces the event loop at ~60 fps (16 ms ≈ 62.5 Hz).
const FRAME_BUDGET: Duration = Duration::from_millis(16);

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
    let header = Row::new([Cell::from("PID"), Cell::from("CPU%"), Cell::from("MEM")])
        .style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED));

    // Formatting (numbers -> human strings) happens lazily here, only for the
    // rows we are about to draw — per the renderer contract in CLAUDE.md §3.
    let rows = state.processes.iter().map(|p| {
        Row::new([
            Cell::from(p.pid.to_string()),
            Cell::from(format!("{:>5.1}", p.cpu_percent)),
            Cell::from(format_bytes(p.memory_bytes)),
        ])
    });

    let widths = [
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(12),
    ];

    let title = Line::from(format!(
        " truetop — tick {} · {} procs · q to quit ",
        state.tick,
        state.processes.len()
    ));

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title(title.bold()))
        .column_spacing(1)
        .row_highlight_style(Style::new().reversed());

    frame.render_widget(table, frame.area());
}

/// Lazily render a byte count as a compact human-readable string.
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
