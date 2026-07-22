//! Command line surface. Parsing runs before the eBPF is loaded, so `--help`
//! and `--version` answer without privileges.

use anyhow::{Context as _, Result, bail};

const USAGE: &str = "\
usage: truetop [--bench <TICKS>] [-h|--help] [-V|--version]

      --bench <TICKS>  run TICKS collector ticks headless, without the UI
  -h, --help           print this help
  -V, --version        print the version";

pub(crate) enum Command {
    Ui,
    Bench(u32),
    /// Text to print before exiting successfully.
    Print(String),
}

pub(crate) fn parse(mut args: impl Iterator<Item = String>) -> Result<Command> {
    let mut command = Command::Ui;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Print(USAGE.to_owned())),
            "-V" | "--version" => {
                let version = env!("CARGO_PKG_VERSION");
                return Ok(Command::Print(format!("truetop {version}")));
            }
            "--bench" => {
                let value = args.next().context("--bench needs a tick count")?;
                let ticks = value
                    .parse()
                    .with_context(|| format!("--bench: `{value}` is not a tick count"))?;
                command = Command::Bench(ticks);
            }
            other => bail!("unknown argument `{other}`\n\n{USAGE}"),
        }
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn no_arguments_render_the_ui() {
        assert!(matches!(parse_args(&[]).unwrap(), Command::Ui));
    }

    #[test]
    fn bench_takes_a_tick_count() {
        assert!(matches!(
            parse_args(&["--bench", "5"]).unwrap(),
            Command::Bench(5)
        ));
    }

    #[test]
    fn help_and_version_answer_without_attaching() {
        assert!(matches!(
            parse_args(&["--help"]).unwrap(),
            Command::Print(_)
        ));
        assert!(matches!(parse_args(&["-V"]).unwrap(), Command::Print(_)));
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        assert!(parse_args(&["--nope"]).is_err());
        assert!(parse_args(&["--bench"]).is_err());
        assert!(parse_args(&["--bench", "many"]).is_err());
    }
}
