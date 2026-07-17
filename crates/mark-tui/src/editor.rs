use std::{
    env, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::EnableMouseCapture,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use mark_core::MarkResult;

use crate::terminal_input::{disable_mouse_capture_and_discard_reports, discard_pending_input};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorTarget {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
}

pub(crate) fn configured_editor() -> Option<String> {
    ["GIT_EDITOR", "VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(env::var_os)
        .map(|editor| editor.to_string_lossy().trim().to_owned())
        .find(|editor| !editor.is_empty())
}

pub(crate) fn repo_file_path(repo: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    }
}

pub(crate) fn open_text_in_editor(editor: &str, path: &Path) -> MarkResult<ExitStatus> {
    open_editor(
        editor,
        &EditorTarget {
            path: path.to_path_buf(),
            line: 1,
        },
    )
}

pub(crate) fn open_editor(editor: &str, target: &EditorTarget) -> MarkResult<ExitStatus> {
    let mut terminal = SuspendedTerminal::suspend()?;
    let status_result = editor_status(editor, target);
    terminal.resume()?;
    Ok(status_result?)
}

pub(crate) fn editor_status(editor: &str, target: &EditorTarget) -> io::Result<ExitStatus> {
    let Some(parts) = split_editor_command(editor) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "editor command is empty or invalid",
        ));
    };

    let mut command = Command::new(&parts[0]);
    command.args(editor_args(&parts, target));
    attach_terminal_stdio(&mut command)?;

    command.status()
}

pub(crate) fn editor_args(parts: &[String], target: &EditorTarget) -> Vec<String> {
    let mut args = parts.get(1..).unwrap_or_default().to_vec();
    let path = target.path.display().to_string();
    let line = target.line.max(1);

    // Placeholders make unusual editors and wrapper scripts configurable without
    // Mark having to guess their command-line syntax. A file placeholder means
    // the command supplied the complete target; line-only templates still get
    // the file appended below.
    let has_file_placeholder = args.iter().any(|arg| arg.contains("{file}"));
    let has_placeholder = has_file_placeholder
        || args
            .iter()
            .any(|arg| arg.contains("{line}") || arg.contains("{column}"));
    let program = parts.first().map(String::as_str).unwrap_or_default();
    let kind = editor_kind(program);
    if has_placeholder {
        let line = line.to_string();
        for arg in &mut args {
            *arg = expand_editor_placeholders(arg, &path, &line);
        }
        if !has_file_placeholder {
            args.push(path);
        }
        if matches!(kind, EditorKind::VsCode | EditorKind::WaitPathLine) && !has_wait_arg(&args) {
            args.push("--wait".to_owned());
        }
        return args;
    }

    match kind {
        EditorKind::VsCode => {
            if !has_wait_arg(&args) {
                args.push("--wait".to_owned());
            }
            if !args.iter().any(|arg| arg == "--goto" || arg == "-g") {
                args.push("--goto".to_owned());
            }
            args.push(format!("{path}:{line}:1"));
        }
        EditorKind::WaitPathLine => {
            if !has_wait_arg(&args) {
                args.push("--wait".to_owned());
            }
            args.push(format!("{path}:{line}:1"));
        }
        EditorKind::PathLine => args.push(format!("{path}:{line}:1")),
        EditorKind::Vim => {
            args.push(format!("+{line}"));
            // Vim otherwise tends to place command-line targets against a
            // viewport edge. Centering makes entering the editor match Mark's
            // focused-row model and avoids the nearly-empty lower viewport.
            args.push("+normal! zz".to_owned());
            args.push(path);
        }
        EditorKind::Nano => {
            args.push(format!("+{line},1"));
            args.push(path);
        }
        EditorKind::PlusLineColumn => {
            args.push(format!("+{line}:1"));
            args.push(path);
        }
        EditorKind::PlusLine => {
            args.push(format!("+{line}"));
            args.push(path);
        }
    }
    args
}

fn expand_editor_placeholders(template: &str, path: &str, line: &str) -> String {
    let mut expanded = String::with_capacity(template.len());
    let mut remainder = template;
    while let Some(index) = remainder.find('{') {
        expanded.push_str(&remainder[..index]);
        remainder = &remainder[index..];
        if let Some(rest) = remainder.strip_prefix("{file}") {
            expanded.push_str(path);
            remainder = rest;
        } else if let Some(rest) = remainder.strip_prefix("{line}") {
            expanded.push_str(line);
            remainder = rest;
        } else if let Some(rest) = remainder.strip_prefix("{column}") {
            expanded.push('1');
            remainder = rest;
        } else {
            expanded.push('{');
            remainder = &remainder[1..];
        }
    }
    expanded.push_str(remainder);
    expanded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorKind {
    VsCode,
    WaitPathLine,
    PathLine,
    Vim,
    Nano,
    PlusLineColumn,
    PlusLine,
}

fn editor_kind(program: &str) -> EditorKind {
    let name = editor_program_name(program);
    match name.as_str() {
        "code" | "code-insiders" | "codium" | "cursor" => EditorKind::VsCode,
        "subl" | "sublime_text" | "zed" => EditorKind::WaitPathLine,
        "helix" | "hx" | "amp" => EditorKind::PathLine,
        "vim" | "nvim" | "gvim" | "mvim" => EditorKind::Vim,
        "nano" | "pico" => EditorKind::Nano,
        "emacs" | "emacsclient" | "kak" | "kakoune" => EditorKind::PlusLineColumn,
        // +line file is shared by the vi family and is the most broadly
        // supported fallback for terminal editors (including micro and vis).
        _ => EditorKind::PlusLine,
    }
}

fn editor_program_name(program: &str) -> String {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    name.trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .to_owned()
}

fn has_wait_arg(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--wait"
            || arg == "-w"
            || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains('w'))
    })
}

pub(crate) fn split_editor_command(editor: &str) -> Option<Vec<String>> {
    let parts = shlex::split(editor)?;
    (!parts.is_empty()).then_some(parts)
}

#[cfg(test)]
pub(crate) fn editor_uses_goto_arg(program: &str) -> bool {
    editor_kind(program) == EditorKind::VsCode
}

struct SuspendedTerminal {
    active: bool,
}

impl SuspendedTerminal {
    fn suspend() -> MarkResult<Self> {
        let terminal = Self { active: true };
        let mut stdout = io::stdout();
        disable_mouse_capture_and_discard_reports(&mut stdout)?;
        execute!(
            stdout,
            LeaveAlternateScreen,
            SetCursorStyle::DefaultUserShape,
            Show
        )?;
        stdout.flush()?;
        disable_raw_mode()?;
        Ok(terminal)
    }

    fn resume(&mut self) -> MarkResult<()> {
        if !self.active {
            return Ok(());
        }

        let _ = discard_pending_input();
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            SetCursorStyle::BlinkingBlock
        )?;
        stdout.flush()?;
        drain_pending_editor_events()?;
        self.active = false;
        Ok(())
    }
}

fn drain_pending_editor_events() -> io::Result<()> {
    // The diff view owns terminal input while it is running. Avoid draining
    // input here; transient editor quit keys are filtered by DiffApp after
    // resume.
    Ok(())
}

#[cfg(unix)]
fn attach_terminal_stdio(command: &mut Command) -> io::Result<()> {
    use std::fs::OpenOptions;
    use std::process::Stdio;

    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    command.stdin(Stdio::from(tty.try_clone()?));
    command.stdout(Stdio::from(tty.try_clone()?));
    command.stderr(Stdio::from(tty));
    Ok(())
}

#[cfg(not(unix))]
fn attach_terminal_stdio(_command: &mut Command) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "opening editors from the TUI is unsupported on this platform",
    ))
}

impl Drop for SuspendedTerminal {
    fn drop(&mut self) {
        let _ = self.resume();
    }
}
