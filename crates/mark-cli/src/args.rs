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
  mark diff
  mark compare main
  mark compare main feature
  mark difftool -- \"$LOCAL\" \"$REMOTE\" \"$MERGED\"
  mark show
  mark show HEAD~1
  mark review 123
  mark review https://github.com/owner/repo/pull/123
  mark patch changes.diff
  cat changes.diff | mark patch -
  git diff | mark pager
  mark diff --watch
  mark diff --no-syntax
  mark diff --minimal
  mark diff --stat
  mark session list --json
  mark skill
  mark skill install --agent pi
  mark config
  mark syntax add ruby elixir";

pub(crate) const INSTALL_SCRIPT: &str = include_str!("../../../scripts/install.sh");
pub(crate) const RELEASE_REPO: &str = "phongndo/mark";

#[derive(Debug, Parser)]
#[command(
    name = "mark",
    version = CLI_VERSION,
    about = "Fast, keyboard-first terminal Git diff reviewer",
    override_usage = "mark [COMMAND]",
    help_template = HELP_TEMPLATE,
    next_help_heading = "options",
    subcommand_help_heading = "commands",
    styles = help_styles()
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
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
        about = "Review all local changes",
        after_help = "\
examples:
  mark diff
  mark diff --no-untracked
  mark diff --watch"
    )]
    Diff(DiffArgs),
    #[command(
        about = "Compare the workspace or two Git revisions",
        after_help = "\
examples:
  mark compare main
  mark compare main feature"
    )]
    Compare(CompareArgs),
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
        about = "Open a GitHub pull request for review",
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
    #[command(about = "Inspect and control a live review session")]
    Session {
        #[command(subcommand)]
        command: SessionSubcommand,
    },
    #[command(
        about = "Show or install the bundled agent review skill",
        after_help = "\
examples:
  mark skill
  mark skill path
  mark skill install --agent pi
  mark skill install --agent cursor
  mark skill install --agent antigravity
  mark skill install --agent copilot"
    )]
    Skill {
        #[command(subcommand)]
        command: Option<SkillCommand>,
    },
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
pub(crate) enum SkillCommand {
    #[command(about = "Materialize the bundled skill and print its path")]
    Path,
    #[command(about = "Print the bundled skill")]
    Show,
    #[command(about = "Install the bundled skill for an agent")]
    Install(SkillInstallArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SkillInstallArgs {
    #[arg(long, value_enum, required = true)]
    pub(crate) agent: SkillAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SkillAgent {
    Pi,
    Codex,
    Claude,
    Cursor,
    Antigravity,
    Copilot,
    Opencode,
}

#[derive(Debug, Args, Default)]
pub(crate) struct SessionSelectorArgs {
    #[arg(value_name = "SESSION_ID", conflicts_with = "repo")]
    pub(crate) session_id: Option<String>,
    #[arg(long, value_name = "PATH", conflicts_with = "session_id")]
    pub(crate) repo: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionSubcommand {
    #[command(visible_alias = "ls", about = "List live review sessions")]
    List(SessionListArgs),
    #[command(about = "Get live session metadata")]
    Get(SessionGetArgs),
    #[command(about = "Inspect live session context and focus")]
    Context(SessionContextArgs),
    #[command(about = "Inspect paginated changeset structure")]
    Review(SessionReviewArgs),
    #[command(about = "Retrieve a bounded file or hunk patch")]
    Patch(SessionPatchArgs),
    #[command(about = "Move the open review focus")]
    Navigate(SessionNavigateArgs),
    #[command(about = "Manage inline review comments")]
    Comment {
        #[command(subcommand)]
        command: SessionCommentCommand,
    },
    #[command(about = "Set reviewed file or hunk progress")]
    Progress(SessionProgressArgs),
    #[command(about = "Manage the human-owned final verdict")]
    Verdict {
        #[command(subcommand)]
        command: SessionVerdictCommand,
    },
    #[command(about = "Explicitly replace the review snapshot")]
    Reload(SessionReloadArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SessionListArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) repo: Option<PathBuf>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionGetArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionContextArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long)]
    pub(crate) changed_files_cursor: Option<String>,
    #[arg(long, value_name = "N")]
    pub(crate) changed_files_limit: Option<usize>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionReviewArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long)]
    pub(crate) cursor: Option<String>,
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    #[arg(long)]
    pub(crate) include_comments: bool,
    #[arg(long, requires = "include_comments")]
    pub(crate) comments_cursor: Option<String>,
    #[arg(long, value_name = "N", requires = "include_comments")]
    pub(crate) comments_limit: Option<usize>,
    #[arg(
        long,
        help = "Only include files changed since the previous review pass"
    )]
    pub(crate) changed_only: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionPatchArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "PATH")]
    pub(crate) file: String,
    #[arg(long, value_name = "N", conflicts_with_all = ["old_line", "new_line"])]
    pub(crate) hunk: Option<usize>,
    #[arg(long, value_name = "N", conflicts_with_all = ["hunk", "new_line"])]
    pub(crate) old_line: Option<usize>,
    #[arg(long, value_name = "N", conflicts_with_all = ["hunk", "old_line"])]
    pub(crate) new_line: Option<usize>,
    #[arg(long, value_name = "N")]
    pub(crate) context: Option<usize>,
    #[arg(long, value_name = "N")]
    pub(crate) max_bytes: Option<usize>,
    #[arg(long)]
    pub(crate) cursor: Option<String>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionNavigateArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<String>,
    #[arg(long, value_name = "N", group = "coordinate")]
    pub(crate) hunk: Option<usize>,
    #[arg(long, value_name = "N", group = "coordinate")]
    pub(crate) old_line: Option<usize>,
    #[arg(long, value_name = "N", group = "coordinate")]
    pub(crate) new_line: Option<usize>,
    #[arg(long, group = "coordinate")]
    pub(crate) next_comment: bool,
    #[arg(long, group = "coordinate")]
    pub(crate) previous_comment: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommentCommand {
    #[command(about = "Add one agent comment")]
    Add(SessionCommentAddArgs),
    #[command(about = "Atomically apply agent comments from stdin")]
    Apply(SessionCommentApplyArgs),
    #[command(visible_alias = "ls", about = "List review comments")]
    List(SessionCommentListArgs),
    #[command(about = "Remove one agent comment")]
    Rm(SessionCommentRemoveArgs),
    #[command(about = "Clear agent comments")]
    Clear(SessionCommentClearArgs),
    #[command(about = "Accept, dismiss, or classify an agent finding")]
    Disposition(SessionCommentDispositionArgs),
}

#[derive(Debug, Args, Default)]
pub(crate) struct CommentTargetArgs {
    #[arg(long, value_name = "N", conflicts_with_all = ["old_line", "new_line", "old_start", "new_start"])]
    pub(crate) hunk: Option<usize>,
    #[arg(long, value_name = "N", conflicts_with_all = ["hunk", "new_line", "old_start", "new_start"])]
    pub(crate) old_line: Option<usize>,
    #[arg(long, value_name = "N", conflicts_with_all = ["hunk", "old_line", "old_start", "new_start"])]
    pub(crate) new_line: Option<usize>,
    // Keep legacy range flags parseable for existing scripts without advertising
    // range annotations as a supported authoring workflow.
    #[arg(long, hide = true, value_name = "N", requires = "old_end", conflicts_with_all = ["hunk", "old_line", "new_line"])]
    pub(crate) old_start: Option<usize>,
    #[arg(long, hide = true, value_name = "N", requires = "old_start")]
    pub(crate) old_end: Option<usize>,
    #[arg(long, hide = true, value_name = "N", requires = "new_end", conflicts_with_all = ["hunk", "old_line", "new_line"])]
    pub(crate) new_start: Option<usize>,
    #[arg(long, hide = true, value_name = "N", requires = "new_start")]
    pub(crate) new_end: Option<usize>,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCommentAddArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "PATH")]
    pub(crate) file: String,
    #[command(flatten)]
    pub(crate) target: CommentTargetArgs,
    #[arg(long)]
    pub(crate) summary: String,
    #[arg(long)]
    pub(crate) rationale: Option<String>,
    #[arg(long)]
    pub(crate) author: Option<String>,
    #[arg(long)]
    pub(crate) generation: Option<u64>,
    #[arg(long)]
    pub(crate) focus: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCommentApplyArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, required = true)]
    pub(crate) stdin: bool,
    #[arg(long)]
    pub(crate) focus: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum CommentOriginArg {
    Agent,
    Human,
    All,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCommentListArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<String>,
    #[arg(long, value_enum, default_value_t = CommentOriginArg::All)]
    pub(crate) origin: CommentOriginArg,
    #[arg(long)]
    pub(crate) cursor: Option<String>,
    #[arg(long, value_name = "N")]
    pub(crate) limit: Option<usize>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCommentRemoveArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "COMMENT_ID")]
    pub(crate) comment_id: String,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCommentClearArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<String>,
    #[arg(long, required = true)]
    pub(crate) yes: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FindingDispositionArg {
    Open,
    Accepted,
    Dismissed,
    Blocking,
    NonBlocking,
    Fixed,
}

#[derive(Debug, Args)]
pub(crate) struct SessionCommentDispositionArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "COMMENT_ID")]
    pub(crate) comment_id: String,
    #[arg(long, value_enum)]
    pub(crate) disposition: FindingDispositionArg,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionProgressArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_name = "PATH")]
    pub(crate) file: String,
    #[arg(long, value_name = "N")]
    pub(crate) hunk: Option<usize>,
    #[arg(long, conflicts_with = "unreviewed")]
    pub(crate) reviewed: bool,
    #[arg(long, conflicts_with = "reviewed")]
    pub(crate) unreviewed: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionVerdictCommand {
    Get(SessionGetArgs),
    Set(SessionVerdictSetArgs),
    Clear(SessionGetArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum VerdictKindArg {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub(crate) enum VerdictDestinationArg {
    #[default]
    Local,
    Stdout,
}

#[derive(Debug, Args)]
pub(crate) struct SessionVerdictSetArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(long, value_enum)]
    pub(crate) kind: VerdictKindArg,
    #[arg(long)]
    pub(crate) summary: Option<String>,
    #[arg(long, value_enum, default_value_t = VerdictDestinationArg::Local)]
    pub(crate) destination: VerdictDestinationArg,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionReloadArgs {
    #[command(flatten)]
    pub(crate) selector: SessionSelectorArgs,
    #[arg(last = true, required = true, num_args = 1.., value_name = "REQUEST")]
    pub(crate) request: Vec<String>,
    #[arg(long)]
    pub(crate) generation: Option<u64>,
    #[arg(long)]
    pub(crate) json: bool,
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
    #[command(about = "Print syntax config and theme paths")]
    Path,
    #[command(about = "Validate the syntax backend and configured languages")]
    Doctor,
    #[command(about = "Inspect exact TextMate scopes and resolved token styles")]
    Inspect(SyntaxInspectArgs),
}

#[derive(Debug, Args)]
pub(crate) struct SyntaxInspectArgs {
    #[arg(value_name = "PATH")]
    pub(crate) path: PathBuf,
    /// Override language detection.
    #[arg(long, value_name = "LANG")]
    pub(crate) language: Option<String>,
    /// One-based source line to inspect. Prints every line when omitted.
    #[arg(long, value_name = "N")]
    pub(crate) line: Option<usize>,
    /// Built-in TextMate theme.
    #[arg(long, default_value = "github-dark-high-contrast")]
    pub(crate) theme: String,
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
    /// Run against this repository instead of the current directory.
    #[arg(short = 'r', long, value_name = "PATH")]
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
    /// Print diff statistics instead of opening the reviewer.
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
    /// Auto-reload when the reviewed source changes.
    #[arg(long)]
    pub(crate) watch: bool,
    /// Compatibility no-op; reviews are snapshots unless --watch is used.
    #[arg(long = "no-watch", hide = true, conflicts_with = "watch")]
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
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    /// Exclude untracked files from the local changes review.
    #[arg(long = "no-untracked")]
    pub(crate) no_untracked: bool,
    #[command(flatten)]
    pub(crate) watch: DiffWatchArgs,
    #[command(flatten)]
    pub(crate) display: DisplayArgs,
}

#[derive(Debug, Args)]
pub(crate) struct CompareArgs {
    /// One revision is compared with the current workspace; two compare directly.
    #[arg(value_name = "REV", num_args = 1..=2, required = true)]
    pub(crate) revs: Vec<String>,
    #[command(flatten)]
    pub(crate) repo: RepoArgs,
    /// Exclude untracked files when comparing with the current workspace.
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
    fn bare_mark_has_no_command() {
        let cli = parse(&["mark"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn rejects_removed_implicit_diff_forms() {
        parse_err(&["mark", "--stat"]);
        parse_err(&["mark", "main"]);
        parse_err(&["mark", "main", "feature"]);
        parse_err(&["mark", "diff", "main"]);
        parse_err(&["mark", "diff", "--base", "main"]);
        parse_err(&["mark", "compare"]);
        parse_err(&["mark", "compare", "main", "feature", "release"]);
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

        parse_err(&["mark", "diff", "--empty-diff-fill", "--no-empty-diff-fill"]);
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

        parse_err(&["mark", "diff", "--minimal", "--fancy"]);
        parse_err(&["mark", "diff", "--minimal", "--decorations", "auto"]);
    }

    #[test]
    fn parses_source_subcommands() {
        let cli = parse(&["mark", "compare", "main"]);
        assert!(matches!(
            cli.command,
            Some(Command::Compare(CompareArgs { revs, .. })) if revs == ["main"]
        ));

        let cli = parse(&["mark", "compare", "main", "feature"]);
        assert!(matches!(
            cli.command,
            Some(Command::Compare(CompareArgs { revs, .. }))
                if revs == ["main", "feature"]
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
    fn parses_live_session_commands_and_targets() {
        let cli = parse(&[
            "mark",
            "session",
            "review",
            "session-1",
            "--limit",
            "20",
            "--json",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Review(SessionReviewArgs {
                    selector: SessionSelectorArgs { session_id: Some(id), .. },
                    limit: Some(20),
                    json: true,
                    ..
                })
            }) if id == "session-1"
        ));

        let cli = parse(&[
            "mark",
            "session",
            "context",
            "--repo",
            ".",
            "--changed-files-cursor",
            "20",
            "--changed-files-limit",
            "10",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Context(SessionContextArgs {
                    changed_files_cursor: Some(cursor),
                    changed_files_limit: Some(10),
                    ..
                })
            }) if cursor == "20"
        ));

        let cli = parse(&[
            "mark",
            "session",
            "navigate",
            "session-1",
            "--file",
            "src/lib.rs",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Navigate(SessionNavigateArgs {
                    file: Some(file),
                    hunk: None,
                    old_line: None,
                    new_line: None,
                    ..
                })
            }) if file == "src/lib.rs"
        ));

        let cli = parse(&[
            "mark",
            "session",
            "comment",
            "add",
            "--repo",
            ".",
            "--file",
            "src/lib.rs",
            "--new-line",
            "4",
            "--summary",
            "finding",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Comment {
                    command: SessionCommentCommand::Add(SessionCommentAddArgs {
                        target: CommentTargetArgs {
                            new_line: Some(4),
                            ..
                        },
                        ..
                    })
                }
            })
        ));

        let cli = parse(&[
            "mark",
            "session",
            "comment",
            "disposition",
            "--repo",
            ".",
            "--comment-id",
            "agent-1",
            "--disposition",
            "blocking",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Comment {
                    command: SessionCommentCommand::Disposition(SessionCommentDispositionArgs {
                        disposition: FindingDispositionArg::Blocking,
                        ..
                    })
                }
            })
        ));

        let cli = parse(&[
            "mark",
            "session",
            "comment",
            "rm",
            "--repo",
            ".",
            "--comment-id",
            "agent-1",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Comment {
                    command: SessionCommentCommand::Rm(SessionCommentRemoveArgs {
                        selector: SessionSelectorArgs { repo: Some(_), .. },
                        comment_id,
                        ..
                    })
                }
            }) if comment_id == "agent-1"
        ));

        let cli = parse(&[
            "mark",
            "session",
            "verdict",
            "set",
            "--repo",
            ".",
            "--kind",
            "approve",
            "--destination",
            "stdout",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Session {
                command: SessionSubcommand::Verdict {
                    command: SessionVerdictCommand::Set(SessionVerdictSetArgs {
                        kind: VerdictKindArg::Approve,
                        destination: VerdictDestinationArg::Stdout,
                        ..
                    })
                }
            })
        ));

        parse_err(&[
            "mark",
            "session",
            "patch",
            "--file",
            "src/lib.rs",
            "--hunk",
            "1",
            "--new-line",
            "4",
        ]);
        parse_err(&["mark", "session", "get", "session-1", "--repo", "."]);
    }

    #[test]
    fn diff_watch_is_opt_in_and_no_watch_is_compatible() {
        let cli = parse(&["mark", "diff", "--watch"]);
        assert!(matches!(
            cli.command,
            Some(Command::Diff(DiffArgs {
                watch: DiffWatchArgs { watch: true, .. },
                ..
            }))
        ));
        let cli = parse(&["mark", "diff", "--no-watch"]);
        assert!(matches!(
            cli.command,
            Some(Command::Diff(DiffArgs {
                watch: DiffWatchArgs {
                    no_watch: true,
                    watch: false
                },
                ..
            }))
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
