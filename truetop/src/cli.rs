//! Command line surface. Parsing runs before the eBPF is loaded, so `--help`
//! and `--version` answer without privileges.

use anyhow::{Context as _, Result, bail};

use crate::ui::theme::Background;

const USAGE: &str = "\
usage: truetop [--background <NAME>] [--max-processes <N>] [--bench <TICKS>]
               [-h|--help] [-V|--version]

      --background <NAME>  the terminal's background: auto (the default), dark
                           or light. auto asks the terminal
      --max-processes <N>  processes the accounting maps hold; the default is
                           derived from the CPU count, since a per-CPU map costs
                           8 bytes per entry per CPU and is allocated at load
      --bench <TICKS>      run TICKS collector ticks headless, without the UI
  -h, --help               print this help
  -V, --version            print the version";

pub(crate) struct Args {
    pub(crate) command: Command,
    /// `None` asks the terminal; see `ui::theme::resolve`.
    pub(crate) background: Option<Background>,
    /// `None` leaves the size to the machine; see `default_max_processes`.
    pub(crate) max_processes: Option<u32>,
}

pub(crate) enum Command {
    Ui,
    Bench(u32),
    /// Text to print before exiting successfully.
    Print(String),
}

pub(crate) fn parse(mut args: impl Iterator<Item = String>) -> Result<Args> {
    let mut parsed = Args {
        command: Command::Ui,
        background: None,
        max_processes: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                parsed.command = Command::Print(USAGE.to_owned());
                return Ok(parsed);
            }
            "-V" | "--version" => {
                parsed.command = Command::Print(format!("truetop {}", env!("CARGO_PKG_VERSION")));
                return Ok(parsed);
            }
            "--bench" => {
                let value = args.next().context("--bench needs a tick count")?;
                let ticks = value
                    .parse()
                    .with_context(|| format!("--bench: `{value}` is not a tick count"))?;
                parsed.command = Command::Bench(ticks);
            }
            "--background" => {
                let value = args.next().context("--background needs a name")?;
                parsed.background = match value.as_str() {
                    "auto" => None,
                    "dark" => Some(Background::Dark),
                    "light" => Some(Background::Light),
                    other => bail!("--background: `{other}` is not auto, dark or light"),
                };
            }
            "--max-processes" => {
                let value = args.next().context("--max-processes needs a count")?;
                parsed.max_processes =
                    Some(
                        value.parse().ok().filter(|&n| n > 0).with_context(|| {
                            format!("--max-processes: `{value}` is not a count")
                        })?,
                    );
            }
            other => bail!("unknown argument `{other}`\n\n{USAGE}"),
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Args> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn no_arguments_render_the_ui_and_leave_the_size_to_the_machine() {
        let args = parse_args(&[]).unwrap();
        assert!(matches!(args.command, Command::Ui));
        assert_eq!(args.max_processes, None);
    }

    #[test]
    fn bench_takes_a_tick_count() {
        assert!(matches!(
            parse_args(&["--bench", "5"]).unwrap().command,
            Command::Bench(5)
        ));
    }

    #[test]
    fn max_processes_applies_to_whichever_mode_runs() {
        let args = parse_args(&["--max-processes", "1024", "--bench", "5"]).unwrap();
        assert!(matches!(args.command, Command::Bench(5)));
        assert_eq!(args.max_processes, Some(1024));
    }

    #[test]
    fn help_and_version_answer_without_attaching() {
        assert!(matches!(
            parse_args(&["--help"]).unwrap().command,
            Command::Print(_)
        ));
        assert!(matches!(
            parse_args(&["-V"]).unwrap().command,
            Command::Print(_)
        ));
    }

    #[test]
    fn the_background_is_named_or_left_to_the_terminal() {
        assert_eq!(parse_args(&[]).unwrap().background, None);
        assert_eq!(
            parse_args(&["--background", "auto"]).unwrap().background,
            None
        );
        assert_eq!(
            parse_args(&["--background", "light"]).unwrap().background,
            Some(Background::Light)
        );
        assert!(parse_args(&["--background", "sepia"]).is_err());
        assert!(parse_args(&["--background"]).is_err());
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        assert!(parse_args(&["--nope"]).is_err());
        assert!(parse_args(&["--bench"]).is_err());
        assert!(parse_args(&["--bench", "many"]).is_err());
        assert!(parse_args(&["--max-processes"]).is_err());
        assert!(parse_args(&["--max-processes", "0"]).is_err());
    }
}
