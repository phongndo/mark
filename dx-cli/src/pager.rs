use std::{
    env,
    ffi::OsString,
    fs::OpenOptions,
    io::{self, IsTerminal, Read, Write},
    process::{Command, Stdio},
    sync::Arc,
};

#[cfg(unix)]
use std::os::fd::OwnedFd;

use dx_core::DxError;

use crate::{
    CliResult,
    args::{PagerArgs, PagerLayoutArg},
    write_stderr, write_stdout_bytes,
};

const DEFAULT_TEXT_PAGER: &str = "less -R";
const DEFAULT_STATIC_WIDTH: usize = 120;
const MIN_STATIC_WIDTH: usize = 20;

pub(crate) fn pager(args: PagerArgs) -> CliResult<()> {
    if io::stdin().is_terminal() {
        return Err(DxError::Usage(
            "dx pager reads diff text from stdin; use `git diff | dx pager`, configure `git config --global core.pager \"dx pager\"`, or run `dx` for the current worktree"
                .to_owned(),
        )
        .into());
    }

    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;

    let env = PagerEnv::current();
    let stdout_tty = io::stdout().is_terminal();
    match pager_action(&input, stdout_tty, &env, controlling_terminal_available()) {
        PagerAction::Passthrough => write_stdout_bytes(&input),
        PagerAction::PlainTextPager => page_plain_text(&input),
        PagerAction::StaticDiff => write_static_diff(&input, &args),
        PagerAction::InteractiveDiff => run_interactive_diff(input, &args),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagerAction {
    Passthrough,
    PlainTextPager,
    StaticDiff,
    InteractiveDiff,
}

fn pager_action(
    input: &[u8],
    stdout_tty: bool,
    env: &PagerEnv,
    has_controlling_terminal: bool,
) -> PagerAction {
    if !looks_like_patch_input(input) {
        return if env.term_is_dumb() || !stdout_tty {
            PagerAction::Passthrough
        } else {
            PagerAction::PlainTextPager
        };
    }

    if env.is_captured_pager_host() {
        return PagerAction::StaticDiff;
    }

    if !stdout_tty || env.term_is_dumb() {
        return PagerAction::Passthrough;
    }

    if !has_controlling_terminal {
        return PagerAction::StaticDiff;
    }

    PagerAction::InteractiveDiff
}

fn write_static_diff(input: &[u8], args: &PagerArgs) -> CliResult<()> {
    let patch = normalized_patch_input(input);
    let options = patch_options(patch);
    let color = io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let rendered = match dx_tui::render_static_pager(
        options,
        dx_tui::StaticPagerOptions {
            width: static_terminal_width(),
            layout: args.layout.into(),
            color,
            syntax: !args.no_syntax,
            ..dx_tui::StaticPagerOptions::default()
        },
    ) {
        Ok(rendered) => rendered,
        Err(error) => {
            write_stderr(format_args!(
                "dx: static pager render failed; falling back to raw diff: {error}\n"
            ))?;
            String::new()
        }
    };
    if rendered.is_empty() {
        let fallback = sanitized_terminal_bytes(input);
        write_stdout_bytes(&fallback)
    } else {
        write_stdout_bytes(rendered.as_bytes())
    }
}

fn run_interactive_diff(input: Vec<u8>, args: &PagerArgs) -> CliResult<()> {
    let _stdin_override = match attach_controlling_terminal_to_stdin() {
        Ok(guard) => guard,
        Err(_) => return write_static_diff(&input, args),
    };
    dx_tui::run_diff_with_live_updates_and_syntax(
        patch_options(normalized_patch_input(&input)),
        false,
        !args.no_syntax,
    )?;
    Ok(())
}

impl From<PagerLayoutArg> for dx_tui::StaticPagerLayout {
    fn from(layout: PagerLayoutArg) -> Self {
        match layout {
            PagerLayoutArg::Auto => Self::Auto,
            PagerLayoutArg::Split => Self::Split,
            PagerLayoutArg::Unified => Self::Unified,
        }
    }
}

fn static_terminal_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
        .unwrap_or(DEFAULT_STATIC_WIDTH)
        .max(MIN_STATIC_WIDTH)
}

fn patch_options(patch: Vec<u8>) -> dx_command::DiffOptions {
    dx_command::DiffOptions {
        repo: None,
        source: dx_command::DiffSource::Patch(dx_command::PatchSource::Stdin(Arc::from(
            patch.into_boxed_slice(),
        ))),
        scope: dx_command::DiffScope::All,
        include_untracked: false,
        stat: false,
    }
}

fn page_plain_text(input: &[u8]) -> CliResult<()> {
    let pager_command = env::var("PAGER").unwrap_or_else(|_| DEFAULT_TEXT_PAGER.to_owned());
    match spawn_shell_command(&pager_command) {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take()
                && let Err(error) = stdin.write_all(input)
                && error.kind() != io::ErrorKind::BrokenPipe
            {
                return Err(error.into());
            }

            let status = child.wait()?;
            if !status.success() {
                write_stderr(format_args!(
                    "dx: pager command exited with {status}: {pager_command}\n"
                ))?;
                write_stdout_bytes(input)?;
            }
            Ok(())
        }
        Err(error) => {
            write_stderr(format_args!(
                "dx: failed to run pager command `{pager_command}`: {error}\n"
            ))?;
            write_stdout_bytes(input)
        }
    }
}

#[cfg(unix)]
fn spawn_shell_command(command: &str) -> io::Result<std::process::Child> {
    let shell = env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
    Command::new(shell)
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

#[cfg(windows)]
fn spawn_shell_command(command: &str) -> io::Result<std::process::Child> {
    Command::new("cmd")
        .arg("/C")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

fn looks_like_patch_input(input: &[u8]) -> bool {
    let normalized = normalized_patch_input(input);
    let text = String::from_utf8_lossy(&normalized);
    text.lines().any(|line| line.starts_with("diff --git "))
        || (text.lines().any(|line| line.starts_with("--- "))
            && text.lines().any(|line| line.starts_with("+++ ")))
        || text.lines().any(|line| line.starts_with("@@ "))
}

fn normalized_patch_input(input: &[u8]) -> Vec<u8> {
    strip_terminal_control(input)
}

fn sanitized_terminal_bytes(input: &[u8]) -> Vec<u8> {
    let stripped = strip_terminal_control(input);
    let text = String::from_utf8_lossy(&stripped);
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() && !matches!(character, '\n' | '\t') {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output.into_bytes()
}

fn strip_terminal_control(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            0x1b => skip_escape(input, &mut index),
            b'\r' => {
                if input.get(index + 1) != Some(&b'\n') {
                    output.push(b'\n');
                }
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    output
}

fn skip_escape(input: &[u8], index: &mut usize) {
    *index += 1;
    let Some(introducer) = input.get(*index).copied() else {
        return;
    };
    *index += 1;

    match introducer {
        b'[' => skip_csi(input, index),
        b']' | b'P' | b'^' | b'_' | b'X' => skip_string_escape(input, index),
        0x20..=0x2f if *index < input.len() => *index += 1,
        _ => {}
    }
}

fn skip_csi(input: &[u8], index: &mut usize) {
    while let Some(byte) = input.get(*index).copied() {
        *index += 1;
        if (0x40..=0x7e).contains(&byte) {
            break;
        }
    }
}

fn skip_string_escape(input: &[u8], index: &mut usize) {
    while let Some(byte) = input.get(*index).copied() {
        *index += 1;
        if byte == 0x07 {
            break;
        }
        if byte == 0x1b && input.get(*index) == Some(&b'\\') {
            *index += 1;
            break;
        }
    }
}

#[cfg(unix)]
fn controlling_terminal_available() -> bool {
    OpenOptions::new().read(true).open("/dev/tty").is_ok()
}

#[cfg(not(unix))]
fn controlling_terminal_available() -> bool {
    true
}

#[cfg(unix)]
fn attach_controlling_terminal_to_stdin() -> io::Result<Option<StdinOverride>> {
    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let stdin = io::stdin();
    let original = rustix::io::dup(&stdin).map_err(io::Error::from)?;
    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    rustix::stdio::dup2_stdin(&tty).map_err(io::Error::from)?;
    Ok(Some(StdinOverride { original }))
}

#[cfg(not(unix))]
fn attach_controlling_terminal_to_stdin() -> io::Result<Option<StdinOverride>> {
    Ok(None)
}

#[cfg(unix)]
struct StdinOverride {
    original: OwnedFd,
}

#[cfg(unix)]
impl Drop for StdinOverride {
    fn drop(&mut self) {
        let _ = rustix::stdio::dup2_stdin(&self.original);
    }
}

#[cfg(not(unix))]
struct StdinOverride;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PagerEnv {
    term: Option<OsString>,
    lv: Option<OsString>,
    git_pager: Option<OsString>,
    has_lazygit_env: bool,
}

impl PagerEnv {
    fn current() -> Self {
        Self {
            term: env::var_os("TERM"),
            lv: env::var_os("LV"),
            git_pager: env::var_os("GIT_PAGER"),
            has_lazygit_env: env::vars_os()
                .any(|(key, _)| key.to_string_lossy().starts_with("LAZYGIT")),
        }
    }

    fn term_is_dumb(&self) -> bool {
        self.term.as_deref() == Some(std::ffi::OsStr::new("dumb"))
    }

    fn is_captured_pager_host(&self) -> bool {
        self.term_is_dumb()
            && (self.lv.as_deref() == Some(std::ffi::OsStr::new("-c"))
                || self.git_pager.is_some()
                || self.has_lazygit_env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(
        term: Option<&str>,
        lv: Option<&str>,
        git_pager: Option<&str>,
        lazygit: bool,
    ) -> PagerEnv {
        PagerEnv {
            term: term.map(OsString::from),
            lv: lv.map(OsString::from),
            git_pager: git_pager.map(OsString::from),
            has_lazygit_env: lazygit,
        }
    }

    #[test]
    fn pager_routes_regular_diff_tty_to_interactive() {
        let action = pager_action(
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n",
            true,
            &env(Some("xterm-256color"), None, None, false),
            true,
        );

        assert_eq!(action, PagerAction::InteractiveDiff);
    }

    #[test]
    fn pager_routes_captured_hosts_to_static_diff() {
        let input = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n";

        assert_eq!(
            pager_action(input, true, &env(Some("dumb"), None, None, true), true),
            PagerAction::StaticDiff
        );
        assert_eq!(
            pager_action(
                input,
                true,
                &env(Some("dumb"), Some("-c"), None, false),
                true
            ),
            PagerAction::StaticDiff
        );
        assert_eq!(
            pager_action(
                input,
                true,
                &env(Some("dumb"), None, Some("dx pager"), false),
                true,
            ),
            PagerAction::StaticDiff
        );
        assert_eq!(
            pager_action(input, false, &env(Some("dumb"), None, None, true), true),
            PagerAction::StaticDiff
        );
    }

    #[test]
    fn pager_passthroughs_diff_when_stdout_is_not_tty() {
        let action = pager_action(
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n",
            false,
            &env(Some("xterm-256color"), None, None, false),
            true,
        );

        assert_eq!(action, PagerAction::Passthrough);
    }

    #[test]
    fn pager_falls_back_to_static_diff_without_controlling_terminal() {
        let action = pager_action(
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n",
            true,
            &env(Some("xterm-256color"), None, None, false),
            false,
        );

        assert_eq!(action, PagerAction::StaticDiff);
    }

    #[test]
    fn pager_passthroughs_dumb_non_captured_terminal() {
        let action = pager_action(
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n",
            true,
            &env(Some("dumb"), None, None, false),
            true,
        );

        assert_eq!(action, PagerAction::Passthrough);
    }

    #[test]
    fn pager_pages_plain_text_on_regular_tty() {
        let action = pager_action(
            b"commit abc123\n",
            true,
            &env(Some("xterm-256color"), None, None, false),
            true,
        );

        assert_eq!(action, PagerAction::PlainTextPager);
    }

    #[test]
    fn patch_detection_ignores_ansi_color() {
        assert!(looks_like_patch_input(
            b"\x1b[1mdiff --git a/a b/a\x1b[0m\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n"
        ));
    }

    #[test]
    fn strip_terminal_control_removes_csi_and_osc() {
        let stripped = strip_terminal_control(b"a\x1b[31mb\x1b[0mc\x1b]52;c;secret\x07d");

        assert_eq!(stripped, b"abcd");
    }

    #[test]
    fn sanitized_terminal_bytes_escapes_controls_after_stripping_sequences() {
        let sanitized = sanitized_terminal_bytes(b"a\x07\x1b[31mb\x1b[0m\n");

        assert_eq!(sanitized, b"a\\u{7}b\n");
    }
}
