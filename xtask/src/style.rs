//! Terminal decoration, dropped when stdout is not a terminal so redirected
//! output stays plain text.

use std::io::IsTerminal as _;

pub(crate) struct Paint(bool);

impl Paint {
    pub(crate) fn detect() -> Self {
        Self(std::io::stdout().is_terminal())
    }

    /// What to pass a child process as `--color`, so its own decoration survives
    /// being piped through us.
    pub(crate) fn child_flag(&self) -> &'static str {
        if self.0 { "always" } else { "never" }
    }

    pub(crate) fn bold(&self, text: &str) -> String {
        self.wrap(text, "1")
    }

    pub(crate) fn green(&self, text: &str) -> String {
        self.wrap(text, "32")
    }

    fn wrap(&self, text: &str, code: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}
