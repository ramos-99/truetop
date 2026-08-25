//! The terminal's background, resolved once before the alternate screen is
//! entered: the query writes an OSC sequence and reads the reply off the same
//! stdin the renderer polls, so asking later would race the event loop.

use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

use super::Background;

/// `requested` when the operator named one, else the terminal's own background.
/// Dark when it does not answer: tmux does not forward the query, and a pipe or
/// a dumb terminal has nothing to answer with.
pub(crate) fn resolve(requested: Option<Background>) -> Background {
    requested.or_else(detect).unwrap_or(Background::Dark)
}

fn detect() -> Option<Background> {
    match theme_mode(QueryOptions::default()).ok()? {
        ThemeMode::Light => Some(Background::Light),
        _ => Some(Background::Dark),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Background, resolve};

    #[test]
    fn a_named_background_is_taken_at_its_word() {
        assert_eq!(resolve(Some(Background::Light)), Background::Light);
        assert_eq!(resolve(Some(Background::Dark)), Background::Dark);
    }

    /// Under tmux and over some ssh setups the query goes unanswered, so startup
    /// must not wait on a reply that is not coming.
    #[test]
    fn asking_a_terminal_that_will_not_answer_still_returns() {
        let started = Instant::now();
        let _ = resolve(None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
