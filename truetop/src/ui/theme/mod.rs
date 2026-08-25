//! Colours and value → style maps. Everything here is ANSI, and so renders from
//! the terminal's own palette, except the two row backgrounds: those are literal
//! RGB and take a [`Background`] to suit the terminal they are drawn on. I/O wait
//! - the metric no other monitor shows - gets a reserved warm accent.

mod detect;

pub(crate) use detect::resolve;
use ratatui::style::{Color, Modifier, Style};

/// The terminal's background, which the RGB row styles are picked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Background {
    #[default]
    Dark,
    Light,
}

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
/// A process is "stuck on the disk" for row-highlight purposes at this I/O wait.
const IO_ROW_THRESHOLD: f64 = 65.0;

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

/// Row background for a process blocked on I/O: none below the threshold, then a
/// red that deepens with the wait so the stuck processes light up (65% → ~100%).
/// Kept clear of the body text, which is the terminal's own foreground.
pub fn io_row_bg(background: Background, percent: f64) -> Option<Color> {
    if percent < IO_ROW_THRESHOLD {
        return None;
    }
    let ((r0, g0, b0), (r1, g1, b1)) = match background {
        Background::Dark => ((48.0, 16.0, 16.0), (128.0, 30.0, 30.0)),
        Background::Light => ((255.0, 224.0, 224.0), (244.0, 168.0, 168.0)),
    };
    let t = ((percent - IO_ROW_THRESHOLD) / (100.0 - IO_ROW_THRESHOLD)).clamp(0.0, 1.0);
    let r = lerp(r0, r1, t);
    let g = lerp(g0, g1, t);
    let b = lerp(b0, b1, t);
    Some(Color::Rgb(r as u8, g as u8, b as u8))
}

/// Memory: dim when the kernel reports none, warming as the process takes a
/// larger share of installed RAM.
pub fn mem_heat(bytes: u64, total: u64) -> Style {
    if bytes == 0 || total == 0 {
        return Style::new().fg(DIM);
    }
    let share = bytes as f64 / total as f64;
    let colour = if share < 0.02 {
        CPU
    } else if share < 0.10 {
        Color::Yellow
    } else {
        Color::Red
    };
    with_bold(Style::new().fg(colour), share >= 0.10)
}

/// Alternating row tint, close enough to the background to group rows without
/// competing with the I/O highlight that overrides it.
pub fn row_stripe(background: Background, index: usize) -> Option<Color> {
    let stripe = match background {
        Background::Dark => Color::Rgb(24, 24, 28),
        Background::Light => Color::Rgb(238, 238, 243),
    };
    (index % 2 == 1).then_some(stripe)
}

fn with_bold(style: Style, bold: bool) -> Style {
    if bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Body text is the terminal's own foreground, so a stripe fixed dark is
    /// unreadable the moment the terminal is light. Each has to suit its own.
    #[test]
    fn the_row_tints_suit_the_background_they_are_drawn_on() {
        let Some(Color::Rgb(dark, ..)) = row_stripe(Background::Dark, 1) else {
            panic!("the dark theme stripes rows");
        };
        let Some(Color::Rgb(light, ..)) = row_stripe(Background::Light, 1) else {
            panic!("the light theme stripes rows");
        };
        assert!(dark < 64, "a dark stripe should stay dark, got {dark}");
        assert!(light > 192, "a light stripe should stay light, got {light}");
    }

    #[test]
    fn only_alternate_rows_are_striped() {
        assert_eq!(row_stripe(Background::Dark, 0), None);
        assert!(row_stripe(Background::Dark, 1).is_some());
    }

    #[test]
    fn a_blocked_row_is_tinted_on_either_background() {
        for background in [Background::Dark, Background::Light] {
            assert_eq!(io_row_bg(background, IO_ROW_THRESHOLD - 0.1), None);
            assert!(io_row_bg(background, 90.0).is_some(), "{background:?}");
        }
    }
}
