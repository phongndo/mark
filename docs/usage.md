# Usage

Mark's review commands open Git diffs in an interactive terminal UI when stdout
is a terminal. When stdout is not a terminal they stream rendered diff output
instead. When `--stat` is requested they stream diff statistics instead of
opening the UI. Bare `mark` is reserved for the upcoming dashboard and currently
exits without output.

Run `mark --help` for the authoritative command list.

## Diff sources

`mark diff` reviews all local changes relative to `HEAD`, including staged,
unstaged, and untracked files:

```sh
mark diff
mark diff --no-untracked
```

`mark compare` compares one revision with the current workspace or compares two
revisions directly. Revisions can be branches, tags, or commit IDs:

```sh
mark compare main             # current workspace against main
mark compare main feature     # main against feature
mark compare HEAD~2 HEAD      # two commits
```

Use `--repo` when running from outside the target repository:

```sh
mark diff --repo ../project
mark compare --repo ../project main
mark show --repo ../project HEAD~1
```

Reviews are stable snapshots by default. Relevant worktree changes set a
warning `!` after the statusline `+/-` counts without replacing the visible
changeset. Press `r` to reload explicitly, or opt into continuous replacement
with `--watch`:

```sh
mark diff --watch
mark compare main --watch
mark diff --no-syntax
```

`--no-watch` remains a hidden compatibility no-op for one release.

Mark chooses between fancy and minimal UI decorations automatically. Use minimal
decorations for constrained terminals, or force fancy decorations when auto
detection is too conservative:

```sh
mark diff --minimal
mark diff --fancy
mark diff --decorations minimal
```

Fancy mode draws the diagonal empty split-cell fill by default; minimal mode
suppresses it. Use `--no-empty-diff-fill` or `--empty-diff-fill` to override it
for one run.

Use `--stat` to print summary statistics instead of opening the interactive UI:

```sh
mark diff --stat
mark compare main feature --stat
mark show HEAD~1 --stat
```

## Revisions and hosted reviews

`mark show` reviews a revision. With no target it shows `HEAD`:

```sh
mark show
mark show HEAD~1
```

Hosted reviews currently support GitHub pull requests:

```sh
mark review 123
mark review https://github.com/owner/repo/pull/123
```

Numeric pull request targets are resolved from the current repository's
`origin` remote. Full GitHub pull request URLs do not need a local repository.
Fetching pull requests uses `curl`. Set `GH_TOKEN` or `GITHUB_TOKEN` for
private repositories or higher rate limits.

## Patch files and stdin

Review an existing unified diff:

```sh
mark patch changes.diff
cat changes.diff | mark patch -
```

## Pager mode

Use `mark pager` for `git diff` and `git show` output:

```sh
git config --global core.pager "mark pager"
git diff | mark pager
```

`mark pager` reads stdin. Diff input opens the interactive reviewer when possible
and falls back to static ANSI output in captured pager hosts such as lazygit.
Non-diff input is passed through the user's text pager.

Static diff output reuses mark's renderer, theme, and layout. It falls
back to plain diff text while no syntax backend is bundled. Override the static
layout when needed:

```sh
mark pager --layout split
mark pager --layout unified
mark pager --no-syntax
mark pager --minimal
mark pager --empty-diff-fill
```

## Difftool mode

Configure Git to launch `mark` for Git-provided file pairs:

```sh
git config --global diff.tool mark
git config --global difftool.mark.cmd 'mark difftool -- "$LOCAL" "$REMOTE" "$MERGED"'
```

Git sets `$LOCAL` to the pre-image, `$REMOTE` to the post-image, and `$MERGED`
to the display path. `mark difftool` turns that pair into a normal review:

```sh
git difftool HEAD -- src/file.rs
mark difftool -- "$LOCAL" "$REMOTE" "$MERGED"
mark difftool --watch -- "$LOCAL" "$REMOTE" "$MERGED"
```

## Live agent review sessions

Every interactive review except pager mode registers a private local session.
An external shell-driven agent can inspect bounded structure and talk on the
same marks as the human, without launching or steering the TUI. Saved marks are
visible on the CLI; a draft the human has not saved is not.

```sh
mark skill
mark skill path
mark skill install --agent pi      # pi, codex, claude, cursor, antigravity,
                                   # copilot, or opencode
mark session list --json
mark session context --repo . --json
mark session review --repo . --json
mark session review --repo . --changed-only --json
mark session patch --repo . --file src/lib.rs --hunk 1 --json
```

Apply an atomic comment batch using the generation returned by `context`:

```sh
cat comments.json | mark session comment apply --repo . --stdin --json
```

Navigate or explicitly advance the stable snapshot:

```sh
mark session navigate --repo . --next-comment --json
mark session reload --repo . --json -- diff -- src/lib.rs
```

Session selection is deterministic: use an ID, `--repo`, or rely on implicit
selection only when exactly one live session exists. Sessions use a private
Unix socket and disappear when the TUI closes. They do not run a daemon or
contact a network service. Comments, reviewed file/hunk progress, dispositions,
and the final verdict remain in memory only while the TUI is open and are
discarded when it closes. Reloading the same source compares per-file
fingerprints and can start another pass: Mark reports changed files, retains
progress for unchanged files, and re-anchors comments only when their evidence
has one unambiguous match. Unmatched comments remain recorded as `stale` or `cleared`; relocated
comments are marked `moved`. Loading a different source starts a fresh review.

Humans can manage lifecycle state from the open TUI or with explicit session
commands:

```sh
mark session progress --repo . --file src/lib.rs --hunk 1 --json
mark session comment disposition --repo . --comment-id agent-2 --disposition blocking --json
mark session verdict set --repo . --kind approve --destination local --json
mark session verdict clear --repo . --json
```

A `local` verdict remains in the current live review until Mark closes. A
`stdout` verdict is emitted as one JSON object after the TUI closes. Advancing
to a changed pass clears the previous verdict. Agents should report findings
but leave dispositions and the final verdict to the human. `mark skill` prints
the exact bundled, version-matched agent workflow; `mark skill show` remains an
explicit equivalent. `mark skill install --agent AGENT` installs it into the
selected agent's user-wide skill directory. A plain process transcript is
available in [the live review demonstration](live-agent-review-demo.md).

## Interactive controls

Common default controls:

```text
q / Ctrl-C     quit
?              help
j / Down       scroll down or move focus; stops on a mark so you can act on it
k / Up         scroll up or move focus; lands on a mark before its code line
d              half-page down
u              half-page up
PgDn           full-page down
PgUp           full-page up
g              top
G              bottom
H / M / L      viewport top / middle / bottom
]              next hunk
[              previous hunk
Shift-Tab/Tab  previous / next file
, / .          expand context up / down
c              collapse expanded context
e              toggle full file / hunks
f              file filter
/              grep filter
n / p          next / previous grep match
r              reload
m              diff type selector
o              settings menu
Enter          annotate the selected line, hunk, or file
A              annotate and advance
x              remove the mark under the cursor; otherwise lock horizontal scroll
X              clear all marks (confirm)
n              annotation search menu
b              file browser
s              toggle split/unified layout
Ctrl-G         open the viewport line (full file) or focused hunk in the editor
y              copy marks to the terminal clipboard
Ctrl-U         clear filters
{ / }          previous / next annotation
R              toggle reviewed state for the focused hunk or file
Ctrl-A/Ctrl-R  set approve / request-changes verdict
Ctrl-V/Ctrl-D  set comment verdict / clear verdict
Ctrl-Shift-C   copy the error log pane to the terminal clipboard
```

Drag the left mouse button across diff code to select it. Mark highlights only the
code cells—not line-number or sign gutters—and automatically copies the selected
text to the terminal clipboard when the button is released. In split view, the
selection stays in the pane where the drag started.

Editor commands are shell-word parsed and open on the line focused in the diff.
Mark knows the location syntax used by Vim/Neovim, Helix, Kakoune, Emacs,
nano/pico, micro/vis, VS Code/Codium/Cursor, Sublime Text, and Zed. For another
editor or a wrapper script, put `{file}`, `{line}`, and optionally `{column}` in
the command, for example `EDITOR='my-editor --location {file}:{line}:{column}'`.

The annotation row highlight is active in the diff by default. Move it with
`j` / `k` or Up / Down; prefix cursor motions with a count to repeat them (`3j`,
`5k`, `4l`). `d` / `u` move half a viewport, Page Up / Page Down move a full
viewport, and `g` / `G` jump to the ends. Like Vim with `scrolloff=8`, the
highlight moves freely until it is eight rows from the top or bottom, then stays
there while the viewport scrolls. The margin shrinks for short viewports and at
diff boundaries. If a hunk is taller than the viewport, its `@@` header stays pinned at the top
after it scrolls off until the next hunk. When Git supplies no optional hunk
label, Mark shows the first nonblank changed line as a fallback label.

Press `Enter` on a code line, hunk header, or file header to annotate that line,
entire hunk, or entire file. Notes render as compact inline blocks without
altering the surrounding diff gutters. In the annotation menu, use
`Ctrl-A`, `Ctrl-D`, `Ctrl-B`, `Ctrl-O`, or `Ctrl-F` to accept, dismiss, mark
blocking, mark non-blocking, or mark fixed all agent findings in the selected
card. Human notes are not modified by these actions.

Press `A` to annotate and advance. After saving the draft, the selection moves
to the next row. Press Esc while writing to cancel the draft.

The previous label-jump targeting mode is still available with
`annotations.targeting = "hints"`; see
[configuration](configuration.md#annotation-targeting).

Selector panes keep focus in the filter input: type to filter, Enter selects or
toggles, Esc closes, and arrows, Tab / Shift-Tab, or Ctrl-N / Ctrl-P move the
highlighted row. Settings with multiple values can also cycle with Left / Right.

Keybindings can be customized in the user config file. See
[configuration](configuration.md#keybindings).

## Syntax languages

Mark uses the Rust-native Syntaxmate TextMate engine with 264 bundled language
IDs. Syntaxmate emits exact TextMate scopes; Mark's adapter derives compact
terminal syntax classes while retaining those scopes for theme resolution:

```sh
mark syntax add ruby elixir
mark syntax update --all
mark syntax available --installed
mark syntax rm ruby
mark syntax list
mark syntax doctor
mark syntax clean
mark syntax path
```

`mark syntax available --installed` prints the bundled catalog and
`mark syntax doctor` validates grammar readiness and custom mappings.
