use std::path::PathBuf;

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};

use crate::version::CLI_VERSION;

pub(crate) const HELP_TEMPLATE: &str = "\
{about-with-newline}
usage:
  {usage}

commands:
{subcommands}

options:
{options}

examples:
  mark
  mark diff --base main
  mark diff main feature
  mark difftool -- \"$LOCAL\" \"$REMOTE\" \"$MERGED\"
  mark show
  mark show HEAD~1
  mark review 123
  mark review https://github.com/owner/repo/pull/123
  mark patch changes.diff
  cat changes.diff | mark patch -
  git diff | mark pager
  mark diff --no-watch
  mark diff --no-syntax
  mark diff --minimal
  mark diff --stat
  mark config
  mark syntax add ruby elixir";

pub(crate) const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install.sh");
pub(crate) const RELEASE_REPO: &str = "phongndo/mark";

#[derive(Debug, Parser)]
#[command(
    name = "mark",
    version = CLI_VERSION,
    about = "Terminal Git diff review tool",
    override_usage = "mark [OPTIONS] [COMMAND|REV] [REV]",
    help_template = HELP_TEMPLATE,
    next_help_heading = "options",
    subcommand_help_heading = "commands",
    styles = help_styles()
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
    #[command(flatten)]
    pub(crate) diff: DiffArgs,
}

pub(crate) fn help_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().bold())
        .usage(AnsiColor::Cyan.on_default().bold())
        .literal(AnsiColor::White.on_default().bold())
        .placeholder(AnsiColor::White.on_default())
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(
        about = "Review a Git diff",
        after_help = "\
examples:
  mark diff
  mark diff --base main
  mark diff main feature"
    )]
    Diff(DiffArgs),
    #[command(
        alias = "page",
        about = "Read pager input from stdin and review diffs",
        after_help = "\
examples:
  git config --global core.pager \"mark pager\"
  git diff | mark pager"
    )]
    Pager(PagerArgs),
    #[command(
        about = "Review Git difftool file pairs",
        after_help = "\
examples:
  git config --global diff.tool mark
  git config --global difftool.mark.cmd 'mark difftool -- \"$LOCAL\" \"$REMOTE\" \"$MERGED\"'
  git difftool HEAD -- src/file.rs
  mark difftool --watch -- \"$LOCAL\" \"$REMOTE\" \"$MERGED\""
    )]
    Difftool(DifftoolArgs),
    #[command(
        about = "Review a Git revision",
        after_help = "\
examples:
  mark show
  mark show HEAD~1"
    )]
    Show(ShowArgs),
    #[command(
        about = "Review a hosted code review",
        after_help = "\
examples:
  mark review 123
  mark review https://github.com/owner/repo/pull/123"
    )]
    Review(ReviewArgs),
    #[command(
        about = "Review an existing unified diff",
        after_help = "\
examples:
  mark patch changes.diff
  cat changes.diff | mark patch -"
    )]
    Patch(PatchArgs),
    #[command(
        alias = "ts",
        about = "Inspect syntax configuration and backend status"
    )]
    Syntax {
        #[command(subcommand)]
        command: SyntaxCommand,
    },
    #[command(about = "Print the user config file path")]
    Config,
    #[command(
        about = "Update this curl-installed mark binary from GitHub releases",
        after_help = "\
examples:
  mark update
  mark update --target-version 0.1.1
  mark update --install-dir ~/.local/bin"
    )]
    Update(UpdateArgs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum SyntaxCommand {
    #[command(about = "Configure syntax languages and mappings")]
    Add(SyntaxAddArgs),
    #[command(about = "Report syntax grammar status")]
    Update(SyntaxUpdateArgs),
    #[command(
        alias = "remove",
        about = "Remove configured syntax languages and custom mappings"
    )]
    Rm(SyntaxLanguagesArgs),
    #[command(
        visible_alias = "ls",
        about = "List installed and enabled syntax languages"
    )]
    List,
    #[command(about = "List languages exposed by the syntax backend")]
    Available(SyntaxAvailableArgs),
    #[command(about = "Remove stale syntax config when a backend is available")]
    Clean,
    #[command(about = "Print syntax config and colorscheme paths")]
    Path,
    #[command(about = "Validate the syntax backend and configured languages")]
    Doctor,
}

#[derive(Debug, Args)]
pub(crate) struct SyntaxAddArgs {
    #[arg(value_name = "LANG", required = true)]
    pub(crate) languages: Vec<String>,
    /// Map a file extension to this language. Can be repeated.
    #[arg(long = "ext", value_name = "EXT")]
    pub(crate) extensions: Vec<String>,
    /// Map an exact filename to this language. Can be repeated.
    #[arg(long = "filename", value_name = "NAME")]
    pub(crate) filenames: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SyntaxLanguagesArgs {
    #[arg(value_name = "LANG", required = true)]
    pub(crate) languages: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SyntaxUpdateArgs {
    #[arg(value_name = "LANG", required_unless_present = "all")]
    pub(crate) languages: Vec<String>,
    #[arg(long, conflicts_with = "languages")]
    pub(crate) all: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SyntaxAvailableArgs {
    #[arg(long, conflicts_with = "enabled")]
    pub(crate) installed: bool,
    #[arg(long, conflicts_with = "installed")]
    pub(crate) enabled: bool,
}

#[derive(Debug, Args, Default)]
pub(crate) struct RepoArgs {
    #[arg(short = 'r', long)]
    pub(crate) repo: Option<PathBuf>,
}

#[derive(Debug, Args, Default)]
pub(crate) struct DisplayArgs {
    /// Disable syntax highlighting in the interactive diff viewer.
    #[arg(long = "no-syntax")]
    pub(crate) no_syntax: bool,
    #[command(flatten)]
    pub(crate) decorations: DecorationArgs,
    #[command(flatten)]
    pub(crate) empty_diff_fill: EmptyDiffFillArgs,
    #[arg(short = 's', long)]
    pub(crate) stat: bool,
}

#[derive(Debug, Args, Default)]
pub(crate) struct DecorationArgs {
    /// Use minimal UI decorations for broad terminal compatibility.
    #[arg(long, conflicts_with_all = ["fancy", "decorations"])]
    pub(crate) minimal: bool,
    /// Use fancy UI decorations.
    #[arg(long, conflicts_with_all = ["minimal", "decorations"])]
    pub(crate) fancy: bool,
    /// UI decoration mode.
    #[arg(long, value_enum, conflicts_with_all = ["minimal", "fancy"])]
    pub(crate) decorations: Option<DecorationArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DecorationArg {
    Auto,
    Fancy,
    Minimal,
}

impl From<DecorationArg> for mark_tui::DecorationPreference {
    fn from(value: DecorationArg) -> Self {
        match value {
            DecorationArg::Auto => Self::Auto,
            DecorationArg::Fancy => Self::Fancy,
            DecorationArg::Minimal => Self::Minimal,
        }
    }
}

#[derive(Debug, Args, Default)]
pub(crate) struct EmptyDiffFillArgs {
    /// Draw a diagonal fill pattern in empty split diff cells.
    #[arg(long = "empty-diff-fill", conflicts_with = "no_empty_diff_fill")]
    pub(crate) empty_diff_fill: bool,
    /// Leave empty split diff cells blank.
    #[arg(long = "no-empty-diff-fill")]
    pub(crate) no_empty_diff_fill: bool,
}

#[derive(Debug, Args, Default)]
pub(crate) struct DiffWatchArgs {
    /// Disable live reload in the interactive diff viewer.
    #[arg(long = "no-watch")]
    pub(crate) no_watch: bool,
}

#[derive(Debug, Args, Default)]
pub(crate) struct DifftoolWatchArgs {
    /// Auto-reload when either difftool input file changes.
    #[arg(long)]
    pub(crate) watch: bool,
}

impl DisplayArgs {
    pub(crate) fn syntax_enabled(&self) -> bool {
        !self.no_syntax
    }

    pub(crate) fn empty_diff_fill_override(&self) -> Option<bool> {
        self.empty_diff_fill.override_value()
    }

    pub(crate) fn decoration_override(&self) -> Option<mark_tui::DecorationPreference> {
        self.decorations.override_value()
    }
}

impl DecorationArgs {
    pub(crate) fn override_value(&self) -> Option<mark_tui::DecorationPreference> {
        if self.minimal {
            Some(mark_tui::DecorationPreference::Minimal)
        } else if self.fancy {
            Some(mark_tui::DecorationPreference::Fancy)
        } else {
            self.decorations.map(Into::into)
        }
    }
}

impl EmptyDiffFillArgs {
    pub(crate) fn override_value(&self) -> Option<bool> {
        if self.empty_diff_fill {
            Some(true)
        } else if self.no_empty_diff_fill {
            Some(false)
        } else {
            None
        }
    }
}

#[derive(Debug, Args, Default)]
pub(crate) struct DiffArgs {
    #[arg(value_name = "REV", num_args = 0..=2)]
    pub(crate) revs: Vec<String>,
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    #[arg(short = 'b', long)]
    pub(crate) base: Option<String>,
    #[arg(long = "no-untracked")]
    pub(crate) no_untracked: bool,
    #[command(flatten)]
    pub(crate) watch: DiffWatchArgs,
    #[command(flatten)]
    pub(crate) display: DisplayArgs,
}

#[derive(Debug, Args, Default)]
pub(crate) struct PagerArgs {
    /// Disable syntax highlighting in diff pager output.
    #[arg(long = "no-syntax")]
    pub(crate) no_syntax: bool,
    #[command(flatten)]
    pub(crate) decorations: DecorationArgs,
    #[command(flatten)]
    pub(crate) empty_diff_fill: EmptyDiffFillArgs,
    /// Layout for static diff output.
    #[arg(long, alias = "mode", value_enum, default_value_t = PagerLayoutArg::Auto)]
    pub(crate) layout: PagerLayoutArg,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum PagerLayoutArg {
    #[default]
    Auto,
    Split,
    #[value(alias = "stack")]
    Unified,
}

#[derive(Debug, Args)]
pub(crate) struct DifftoolArgs {
    /// File containing the pre-image from Git difftool.
    #[arg(value_name = "LEFT")]
    pub(crate) left: PathBuf,
    /// File containing the post-image from Git difftool.
    #[arg(value_name = "RIGHT")]
    pub(crate) right: PathBuf,
    /// Display path for the compared file, usually Git's $MERGED value.
    #[arg(value_name = "PATH")]
    pub(crate) path: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    #[command(flatten)]
    pub(crate) watch: DifftoolWatchArgs,
    #[command(flatten)]
    pub(crate) display: DisplayArgs,
}

#[derive(Debug, Args, Default)]
pub(crate) struct ShowArgs {
    /// Revision to show. Defaults to HEAD.
    #[arg(value_name = "REV")]
    pub(crate) rev: Option<String>,
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    #[command(flatten)]
    pub(crate) display: DisplayArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ReviewArgs {
    /// Hosted review target. Currently supports GitHub pull request numbers or URLs.
    #[arg(value_name = "TARGET")]
    pub(crate) target: String,
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    #[command(flatten)]
    pub(crate) display: DisplayArgs,
}

#[derive(Debug, Args)]
pub(crate) struct PatchArgs {
    /// Unified diff file to review, or stdin when FILE is `-`.
    #[arg(value_name = "FILE")]
    pub(crate) path: PathBuf,
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    #[command(flatten)]
    pub(crate) display: DisplayArgs,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateArgs {
    /// Release version to install, or nightly, without or with the leading v.
    #[arg(long = "target-version", value_name = "VERSION")]
    pub(crate) version: Option<String>,
    /// Directory to update. Defaults to the directory containing the invoked mark.
    #[arg(long, value_name = "DIR")]
    pub(crate) install_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    fn parse_err(args: &[&str]) -> clap::Error {
        Cli::try_parse_from(args).expect_err("args should not parse")
    }

    #[cfg(unix)]
    fn parse_os(args: Vec<std::ffi::OsString>) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    #[test]
    fn parses_top_level_diff_compatibility_args() {
        let cli = parse(&["mark", "--stat"]);
        assert!(cli.command.is_none());
        assert!(cli.diff.display.stat);

        let cli = parse(&["mark", "--empty-diff-fill"]);
        assert_eq!(cli.diff.display.empty_diff_fill_override(), Some(true));

        let cli = parse(&["mark", "--minimal"]);
        assert_eq!(
            cli.diff.display.decoration_override(),
            Some(mark_tui::DecorationPreference::Minimal)
        );

        let cli = parse(&["mark", "main", "feature"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.diff.revs, ["main", "feature"]);
    }

    #[test]
    fn parses_empty_diff_fill_flags() {
        let cli = parse(&["mark", "diff", "--no-empty-diff-fill"]);
        assert!(
            matches!(cli.command, Some(Command::Diff(args)) if args.display.empty_diff_fill_override() == Some(false))
        );

        let cli = parse(&["mark", "pager", "--empty-diff-fill"]);
        assert!(
            matches!(cli.command, Some(Command::Pager(args)) if args.empty_diff_fill.override_value() == Some(true))
        );

        parse_err(&["mark", "--empty-diff-fill", "--no-empty-diff-fill"]);
    }

    #[test]
    fn parses_decoration_flags() {
        let cli = parse(&["mark", "diff", "--decorations", "fancy"]);
        assert!(
            matches!(cli.command, Some(Command::Diff(args)) if args.display.decoration_override() == Some(mark_tui::DecorationPreference::Fancy))
        );

        let cli = parse(&["mark", "pager", "--minimal"]);
        assert!(
            matches!(cli.command, Some(Command::Pager(args)) if args.decorations.override_value() == Some(mark_tui::DecorationPreference::Minimal))
        );

        parse_err(&["mark", "--minimal", "--fancy"]);
        parse_err(&["mark", "--minimal", "--decorations", "auto"]);
    }

    #[test]
    fn parses_source_subcommands() {
        let cli = parse(&["mark", "diff", "--base", "main"]);
        assert!(matches!(
            cli.command,
            Some(Command::Diff(DiffArgs {
                base: Some(base),
                ..
            })) if base == "main"
        ));

        let cli = parse(&["mark", "show", "--stat", "HEAD~1"]);
        assert!(matches!(
            cli.command,
            Some(Command::Show(ShowArgs {
                display: DisplayArgs { stat: true, .. },
                ..
            }))
        ));

        let cli = parse(&[
            "mark",
            "review",
            "--stat",
            "https://github.com/owner/repo/pull/123",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Review(ReviewArgs {
                display: DisplayArgs { stat: true, .. },
                ..
            }))
        ));

        let cli = parse(&["mark", "patch", "changes.diff"]);
        assert!(matches!(
            cli.command,
            Some(Command::Patch(PatchArgs { path, .. }))
                if path.as_path() == std::path::Path::new("changes.diff")
        ));

        let cli = parse(&[
            "mark",
            "difftool",
            "left.rs",
            "right.rs",
            "src/file.rs",
            "--watch",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Difftool(DifftoolArgs { left, right, path: Some(path), watch: DifftoolWatchArgs { watch: true }, .. }))
                if left.as_path() == std::path::Path::new("left.rs")
                    && right.as_path() == std::path::Path::new("right.rs")
                    && path.as_path() == std::path::Path::new("src/file.rs")
        ));

        let cli = parse(&["mark", "difftool", "--", "-foo.txt", "--stat"]);
        assert!(matches!(
            cli.command,
            Some(Command::Difftool(DifftoolArgs { left, right, path: None, display: DisplayArgs { stat: false, .. }, .. }))
                if left.as_path() == std::path::Path::new("-foo.txt")
                    && right.as_path() == std::path::Path::new("--stat")
        ));

        let cli = parse(&["mark", "difftool", "--", "left.tmp", "right.tmp", "--stat"]);
        assert!(matches!(
            cli.command,
            Some(Command::Difftool(DifftoolArgs { path: Some(path), display: DisplayArgs { stat: false, .. }, .. }))
                if path.as_path() == std::path::Path::new("--stat")
        ));

        let cli = parse(&[
            "mark", "difftool", "--watch", "--", "-foo.txt", "--stat", "--merged",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Difftool(DifftoolArgs { left, right, path: Some(path), watch: DifftoolWatchArgs { watch: true }, .. }))
                if left.as_path() == std::path::Path::new("-foo.txt")
                    && right.as_path() == std::path::Path::new("--stat")
                    && path.as_path() == std::path::Path::new("--merged")
        ));
    }

    #[test]
    fn rejects_removed_source_compatibility_args() {
        parse_err(&["mark", "--patch", "changes.diff"]);
        parse_err(&["mark", "diff", "--patch", "changes.diff"]);
        parse_err(&["mark", "--pr", "123"]);
        parse_err(&["mark", "diff", "--pr", "123"]);
        parse_err(&["mark", "show", "review", "123"]);
    }

    #[cfg(unix)]
    #[test]
    fn parses_difftool_non_utf8_display_path() {
        use std::{
            ffi::OsString,
            os::unix::ffi::{OsStrExt, OsStringExt},
        };

        let cli = parse_os(vec![
            OsString::from("mark"),
            OsString::from("difftool"),
            OsString::from("--"),
            OsString::from("left.tmp"),
            OsString::from("right.tmp"),
            OsString::from_vec(b"name-\xff.txt".to_vec()),
        ]);

        assert!(matches!(
            cli.command,
            Some(Command::Difftool(DifftoolArgs { path: Some(path), .. }))
                if path.as_os_str().as_bytes() == b"name-\xff.txt"
        ));
    }
}
