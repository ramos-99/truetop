//! Colours and value → style maps: the one place the visual language lives, so a
//! future light/dark switch touches only this file. Body text uses the
//! terminal's default foreground (`Reset`) to read on any theme, and I/O wait -
//! the metric no other monitor shows - gets a reserved warm accent.

use ratatui::style::{Color, Modifier, Style};

/// Structural / title accent.
pub const ACCENT: Color = Color::Cyan;
/// Primary body text (the terminal's own foreground).
pub const TEXT: Color = Color::Reset;
/// Idle rows, secondary fields, empty values.
pub const DIM: Color = Color::DarkGray;
/// CPU series and gauge.
pub const CPU: Color = Color::Green;
/// I/O wait: the hero. Dusty orange, distinct from the CPU greens.
pub const IO: Color = Color::Indexed(173);

pub fn header() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

/// CPU%: dim when idle, green → yellow → red as it climbs.
pub fn cpu_heat(percent: f64) -> Style {
    let colour = if percent < 0.05 {
        DIM
    } else if percent < 30.0 {
        Color::Green
    } else if percent < 70.0 {
        Color::Yellow
    } else {
        Color::Red
    };
    with_bold(Style::new().fg(colour), percent >= 70.0)
}

/// I/O wait: dim when none, warm accent that bolds once a process spends a large
/// share of the interval blocked on the disk.
pub fn io_heat(percent: f64) -> Style {
    if percent <= 0.0 {
        return Style::new().fg(DIM);
    }
    with_bold(Style::new().fg(IO), percent >= 50.0)
}

fn with_bold(style: Style, bold: bool) -> Style {
    if bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}
