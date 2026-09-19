# Usage

Start with the [README examples](../README.md#quick-start). Use
`mark <command> --help` for supported flags and `?` in the reviewer for controls.

## Choosing a source

- `mark diff` reviews staged, unstaged, and untracked changes relative to `HEAD`.
  Add `--no-untracked` to omit untracked files.
- `mark compare main` compares the workspace with a revision;
  `mark compare main feature` compares two revisions.
- `mark show` reviews `HEAD`, or pass a revision such as `HEAD~1`.
- `mark patch changes.diff` reads a unified diff; use `-` to read stdin.
- `mark review 123` resolves a GitHub pull request through the repository's
  `origin`. A full GitHub PR URL works without a local checkout. Fetching uses
  `curl`; set `GH_TOKEN` or `GITHUB_TOKEN` for private repositories.

Use `--repo PATH` when the target repository is elsewhere. Review commands open
an interactive UI when stdout is a terminal and stream output otherwise;
`--stat` prints statistics instead. Bare `mark` does not open a review.

## Snapshots and reloads

Reviews are stable snapshots. A `!` after the statusline change counts means
source changes were detected, not that the displayed patch changed. Press `r`
to replace it explicitly. Use `--watch` on a supporting command for automatic
reloads; a saved `live_reload` setting alone does not opt a new review into watch
mode.

## Pager and difftool

The [README](../README.md#git-integration) has the Git configuration commands.
`mark pager` reads stdin: diff input opens the reviewer when possible and uses
static output in captured pager hosts such as lazygit. Non-diff input passes
through the user's text pager. `--layout split` or `--layout unified` chooses
the static layout; `--no-syntax` disables highlighting.

Git's difftool passes the pre-image, post-image, and display path as `$LOCAL`,
`$REMOTE`, and `$MERGED`. Keep `--` before those paths so filenames beginning
with `-` are not interpreted as options.

## Marks and editor integration

Press `Enter` on a code line, hunk header, or file header to annotate that scope.
`Ctrl-S` saves the draft; Esc cancels it. `A` annotates and advances after saving.
Only saved marks are visible to agents. Use the annotation menu to classify
agent findings; those actions leave human notes unchanged.

Dragging over diff code copies the selected text to the terminal clipboard on
release. Gutters are excluded, and split-view selection stays in its starting
pane. Clipboard support depends on the terminal.

`Ctrl-G` opens the focused location using the first nonempty `GIT_EDITOR`,
`VISUAL`, or `EDITOR`. For an editor or wrapper whose location syntax Mark does
not recognize, use placeholders, for example:

```sh
GIT_EDITOR='my-editor --location {file}:{line}:{column}' mark diff
```

Editor commands are shell-word parsed. `{file}`, `{line}`, and optional
`{column}` provide the location. Custom bindings and hint-based annotation
targeting are described in [configuration](configuration.md).

## Live agent review sessions

Use the [README agent workflow](../README.md#use-with-an-ai-agent) to get started.
`mark skill show` is the version-matched workflow; `mark session --help` is the
command reference. The skill is request-driven, not a background listener.

Every interactive review except pager mode registers a private local session.
Keep Mark open: comments, reviewed progress, dispositions, and verdicts are
in-memory and discarded when it closes. There is no daemon or hosted service.
Export anything you need before closing the review.

Session discovery and bounded inspection start with:

```sh
mark session list --json
mark session context --repo . --json
mark session review --repo . --json
```

When more than one session matches, choose an explicit session ID instead of
guessing. Pin subsequent commands to that ID. Follow the bundled skill for
pagination, complete anchors, generation checks, and publishing comment batches;
this guide does not duplicate that protocol.

Reloading the same source compares file fingerprints: unchanged progress can
survive, while comments are re-anchored only when evidence has one unambiguous
match. Relocated comments are `moved`; unmatched ones remain recorded as `stale`
or `cleared`. Loading a different source starts a fresh review. A changed pass
clears the previous verdict.

The human owns navigation, reviewed state, finding dispositions, and the final
verdict. A `local` verdict lasts only for the live session; a `stdout` verdict
is emitted as a JSON object when the TUI closes. Use
`mark session verdict set --help` for explicit export options.

## Syntax troubleshooting

Grammars are bundled through Syntaxmate; adding a language configures use or
path mappings, not a network download. Inspect the installed build rather than
relying on a catalog count:

```sh
mark syntax available --installed
mark syntax inspect src/lib.rs --line 42
```

See [configuration](configuration.md#syntax-and-resource-limits) for custom
mappings and highlighting limits.
