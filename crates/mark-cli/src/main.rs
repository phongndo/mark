// mimalloc measurably outperforms the system allocator on the tokenizer and
// diff-model hot paths (engine sweep +5-20% per corpus, 2026-07-12 report).
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod args;
mod config;
mod dispatch;
mod pager;
mod review;
mod session;
mod skill;
mod syntax;
mod update;
mod version;

use std::{
    fmt,
    io::{self, IsTerminal, Write},
    process::ExitCode,
};

use clap::Parser;
use mark_core::{MarkError, MarkResult};

use crate::{args::Cli, dispatch::run_cli};

fn main() -> ExitCode {
    if version_only_requested() {
        return match write_stdout(format_args!("mark {}\n", version::CLI_VERSION)) {
            Ok(()) | Err(CliError::StdoutBrokenPipe) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(1),
        };
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if is_clean_exit_error(&error) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = write_stderr(format_args!(
                "{} {error}\n",
                styled_error_prefix(io::stderr().is_terminal())
            ));
            ExitCode::from(1)
        }
    }
}

fn version_only_requested() -> bool {
    let mut args = std::env::args_os();
    let _program = args.next();
    matches!(
        (args.next().as_deref(), args.next()),
        (Some(argument), None) if argument == "--version" || argument == "-V"
    )
}

fn styled_error_prefix(color: bool) -> &'static str {
    if color {
        "\x1b[31mmark:\x1b[0m"
    } else {
        "mark:"
    }
}

pub(crate) type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
pub(crate) enum CliError {
    Mark(MarkError),
    StdoutBrokenPipe,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mark(error) => write!(formatter, "{error}"),
            Self::StdoutBrokenPipe => write!(formatter, "broken pipe"),
        }
    }
}

impl From<MarkError> for CliError {
    fn from(error: MarkError) -> Self {
        Self::Mark(error)
    }
}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self::Mark(error.into())
    }
}

pub(crate) fn write_stdout(args: fmt::Arguments<'_>) -> CliResult<()> {
    io::stdout()
        .lock()
        .write_fmt(args)
        .map_err(stdout_write_error)?;
    Ok(())
}

pub(crate) fn write_stdout_bytes(bytes: &[u8]) -> CliResult<()> {
    io::stdout()
        .lock()
        .write_all(bytes)
        .map_err(stdout_write_error)?;
    Ok(())
}

pub(crate) fn write_stdout_io(
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> CliResult<()> {
    let mut stdout = io::stdout().lock();
    write(&mut stdout).map_err(stdout_write_error)?;
    Ok(())
}

pub(crate) fn write_stderr(args: fmt::Arguments<'_>) -> MarkResult<()> {
    io::stderr().lock().write_fmt(args)?;
    Ok(())
}

fn stdout_write_error(error: io::Error) -> CliError {
    if error.kind() == io::ErrorKind::BrokenPipe {
        CliError::StdoutBrokenPipe
    } else {
        error.into()
    }
}

fn is_clean_exit_error(error: &CliError) -> bool {
    matches!(error, CliError::StdoutBrokenPipe)
}

fn run() -> CliResult<()> {
    let cli = Cli::parse();
    run_cli(cli)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn bare_mark_is_a_silent_noop() {
        let cli = Cli::try_parse_from(["mark"]).expect("bare mark should parse");
        run_cli(cli).expect("bare mark should exit successfully");
    }
}
