//! Renderer (frontend) - the consumer side of the snapshot-per-tick
//! double-buffer.
//!
//! Fully isolated from the backend: it only `load()`s the current immutable
//! snapshot of raw values, sorts and formats it, and feeds ratatui. It does no
//! collection and no blocking I/O against the kernel. Formatting happens here,
//! lazily, and the loop only redraws when the snapshot or the input changes -
//! an idle truetop costs nothing.

mod header;
mod table;
pub(crate) mod theme;

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
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::TableState,
};

use crate::{backend::SystemState, system::Machine, ui::theme::Background};

/// Poll timeout that caps the redraw cadence at ~60 fps.
const FRAME_BUDGET: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Cpu,
    Mem,
    Io,
}

impl Sort {
    fn column(self) -> usize {
        match self {
            Self::Cpu => table::COL_CPU,
            Self::Mem => table::COL_MEM,
            Self::Io => table::COL_IO,
        }
    }
}

/// Renderer-owned view state. None of it reaches the collector.
struct App {
    background: Background,
    selection: TableState,
    sort: Sort,
    descending: bool,
    filter: String,
    editing_filter: bool,
    paused: bool,
    /// Rows the last draw produced, and the height of one page of them.
    rows: usize,
    page: usize,
}

impl App {
    fn new(background: Background) -> Self {
        Self {
            background,
            selection: TableState::default().with_selected(0),
            sort: Sort::Cpu,
            descending: true,
            filter: String::new(),
            editing_filter: false,
            paused: false,
            rows: 0,
            page: 10,
        }
    }

    /// Apply a keypress; returns `true` if the user asked to quit.
    fn handle_key(&mut self, code: KeyCode) -> bool {
        if self.editing_filter {
            self.edit_filter(code);
            return false;
        }
        let last = self.rows.saturating_sub(1);
        let current = self.selection.selected().unwrap_or(0);
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.select(if current >= last { 0 } else { current + 1 })
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select(if current == 0 { last } else { current - 1 })
            }
            KeyCode::PageDown => self.select((current + self.page).min(last)),
            KeyCode::PageUp => self.select(current.saturating_sub(self.page)),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(last),
            KeyCode::Char('c') => self.set_sort(Sort::Cpu),
            KeyCode::Char('m') => self.set_sort(Sort::Mem),
            KeyCode::Char('i') => self.set_sort(Sort::Io),
            KeyCode::Char('/') => self.editing_filter = true,
            KeyCode::Char(' ') => self.paused = !self.paused,
            _ => {}
        }
        false
    }

    fn edit_filter(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.filter.clear();
                self.editing_filter = false;
            }
            KeyCode::Enter => self.editing_filter = false,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => return,
        }
        self.select(0);
    }

    fn select(&mut self, index: usize) {
        self.selection.select(Some(index));
    }

    /// Re-pressing the active column reverses it; a new column starts descending.
    fn set_sort(&mut self, sort: Sort) {
        if self.sort == sort {
            self.descending = !self.descending;
        } else {
            self.sort = sort;
            self.descending = true;
        }
        self.select(0);
    }
}

// ── Entry & loop ─────────────────────────────────────────────────────────────

pub fn render_app(
    state: Arc<ArcSwap<SystemState>>,
    running: Arc<AtomicBool>,
    machine: Machine,
    background: Background,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &state, &running, &machine, background);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    state: &ArcSwap<SystemState>,
    running: &AtomicBool,
    machine: &Machine,
    background: Background,
) -> io::Result<()> {
    let mut app = App::new(background);
    let mut shown = state.load_full();
    let mut last: *const SystemState = Arc::as_ptr(&shown);
    let mut redraw = true;

    while running.load(Ordering::Relaxed) {
        if !app.paused {
            let latest = state.load_full();
            if !std::ptr::eq(Arc::as_ptr(&latest), last) {
                last = Arc::as_ptr(&latest);
                shown = latest;
                redraw = true;
            }
        }

        if event::poll(FRAME_BUDGET)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.handle_key(key.code) {
                        running.store(false, Ordering::Relaxed);
                    }
                    redraw = true;
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }

        if redraw {
            terminal.draw(|frame| draw(frame, &shown, machine, &mut app))?;
            redraw = false;
        }
    }
    Ok(())
}

// ── Layout ───────────────────────────────────────────────────────────────────

fn draw(frame: &mut Frame, state: &SystemState, machine: &Machine, app: &mut App) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(9),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    header::draw_identity(frame, machine, areas[0]);
    header::draw_charts(frame, state, areas[1]);
    header::draw_summary(frame, state, areas[2]);

    app.rows = table::draw(
        frame,
        state,
        table::View {
            background: app.background,
            sort: app.sort,
            descending: app.descending,
            filter: &app.filter,
        },
        &mut app.selection,
        areas[3],
    );
    // Borders and the header row are not selectable.
    app.page = usize::from(areas[3].height).saturating_sub(3).max(1);
    if app.selection.selected().unwrap_or(0) >= app.rows {
        app.selection.select(Some(app.rows.saturating_sub(1)));
    }

    draw_status(frame, app, state, areas[4]);
}

fn draw_status(frame: &mut Frame, app: &App, state: &SystemState, area: Rect) {
    if app.editing_filter {
        frame.render_widget(
            Line::from(vec![
                Span::styled(" filter: ", Style::new().fg(theme::ACCENT)),
                Span::styled(format!("{}_", app.filter), Style::new().fg(theme::TEXT)),
                Span::styled(
                    "   enter to accept, esc to clear",
                    Style::new().fg(theme::DIM),
                ),
            ]),
            area,
        );
        return;
    }

    let key = |k: &'static str, desc: &'static str| {
        [
            Span::styled(k, Style::new().fg(theme::ACCENT)),
            Span::styled(format!(" {desc}   "), Style::new().fg(theme::DIM)),
        ]
    };
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key("q", "quit"));
    spans.extend(key("↑↓", "nav"));
    push_sort_keys(&mut spans, app.sort);
    spans.extend(key("/", "filter"));
    spans.extend(key("space", "pause"));
    if !app.filter.is_empty() {
        spans.push(Span::styled(
            format!("  filter \"{}\"", app.filter),
            Style::new().fg(theme::ACCENT),
        ));
    }
    if app.paused {
        spans.push(Span::styled(
            "  paused",
            Style::new().fg(theme::IO).add_modifier(Modifier::BOLD),
        ));
    }
    if state.at_capacity {
        spans.push(Span::styled(
            "  at capacity, raise --max-processes",
            Style::new().fg(theme::IO).add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(Line::from(spans), area);
}

/// The `c` / `m` / `i` sort keys with the active one lit and the rest dim, so a
/// sort change is legible at a glance - the active key moves as it is pressed.
fn push_sort_keys(spans: &mut Vec<Span<'static>>, active: Sort) {
    spans.push(Span::styled("sort ", Style::new().fg(theme::DIM)));
    for (label, sort) in [("c", Sort::Cpu), ("m", Sort::Mem), ("i", Sort::Io)] {
        let style = if sort == active {
            Style::new()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new().fg(theme::DIM)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    spans.push(Span::raw("   "));
}

/// Render a byte count as a compact human-readable string (e.g. `12.3 M`).
pub(super) fn format_bytes(bytes: u64) -> String {
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
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::metrics::ProcessMetrics;

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

    #[test]
    fn re_pressing_the_active_column_reverses_it() {
        let mut app = App::new(Background::Dark);
        assert!(app.descending);
        app.set_sort(Sort::Cpu);
        assert!(!app.descending);
        app.set_sort(Sort::Mem);
        assert!(app.descending);
    }

    #[test]
    fn filter_editing_captures_keys_instead_of_commands() {
        let mut app = App::new(Background::Dark);
        app.handle_key(KeyCode::Char('/'));
        for c in "fio".chars() {
            app.handle_key(KeyCode::Char(c));
        }
        assert_eq!(app.filter, "fio");
        assert!(!app.handle_key(KeyCode::Char('q')));
        assert_eq!(app.filter, "fioq");
        app.handle_key(KeyCode::Esc);
        assert!(app.filter.is_empty());
    }

    #[test]
    fn space_toggles_pause() {
        let mut app = App::new(Background::Dark);
        app.handle_key(KeyCode::Char(' '));
        assert!(app.paused);
        app.handle_key(KeyCode::Char(' '));
        assert!(!app.paused);
    }

    const WIDTH: u16 = 120;
    const HEIGHT: u16 = 24;

    fn rows(state: &SystemState) -> Vec<String> {
        let machine = Machine {
            hostname: "testhost".into(),
            cpu_model: "Test CPU".into(),
            cores: 8,
            memory_total_bytes: 16 << 30,
        };
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, state, &machine, &mut App::new(Background::Dark)))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..HEIGHT)
            .map(|y| {
                (0..WIDTH)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol()))
                    .collect()
            })
            .collect()
    }

    /// Row 0 without the right-aligned clock, which moves every second.
    fn identity_line(state: &SystemState) -> String {
        const CLOCK: usize = 10;
        rows(state)[0]
            .chars()
            .take(WIDTH as usize - CLOCK)
            .collect()
    }

    fn status_line(state: &SystemState) -> String {
        rows(state).pop().expect("a status row")
    }

    fn summary_line(state: &SystemState) -> String {
        rows(state)
            .into_iter()
            .find(|row| row.contains("load "))
            .expect("a summary row")
    }

    /// The renderer draws its first frame before the collector has ticked. The
    /// header used to read its core count off that empty snapshot and rendered
    /// "0 cores" into the README's demo, so what it shows must not depend on the
    /// metrics at all.
    #[test]
    fn the_identity_line_does_not_depend_on_the_snapshot() {
        let populated = SystemState {
            processes: vec![ProcessMetrics {
                pid: 1,
                name: "init".into(),
                ..ProcessMetrics::default()
            }],
            memory_used_bytes: 4 << 30,
            memory_total_bytes: 8 << 30,
            load_average: 1.5,
            ..SystemState::default()
        };
        let before_the_first_tick = identity_line(&SystemState::default());

        assert_eq!(before_the_first_tick, identity_line(&populated));
        assert!(
            before_the_first_tick.contains("8 cores"),
            "{before_the_first_tick}"
        );
    }

    /// The count used to come off the row list, which holds the top of each
    /// metric and so stops at 512 however many processes are running.
    #[test]
    fn the_summary_counts_processes_not_visible_rows() {
        let state = SystemState {
            processes: vec![ProcessMetrics::default()],
            tracked: 4242,
            ..SystemState::default()
        };
        let summary = summary_line(&state);

        assert!(summary.contains("4242 tasks"), "{summary}");
    }

    #[test]
    fn a_saturated_map_is_said_out_loud() {
        let full = SystemState {
            at_capacity: true,
            ..SystemState::default()
        };
        assert!(status_line(&full).contains("at capacity"), "{full:?}");
        assert!(!status_line(&SystemState::default()).contains("at capacity"));
    }
}
