# Usage

`mark` reviews Git diffs in an interactive terminal UI when stdout is a terminal.
When stdout is not a terminal it streams rendered diff output instead. When
`--stat` is requested it streams diff statistics instead of opening the UI.

Run `mark --help` for the authoritative command list.

## Diff sources

`mark` is a shortcut for `mark diff`:

```sh
mark
mark diff
```

Common local review modes:

```sh
mark diff --no-untracked
mark main                  # current branch against main
mark main feature          # revision range
```

The explicit forms `mark diff --base main` and `mark diff main feature` are
equivalent when preferred for scripts or discoverability.

Use `--repo` when running from outside the target repository:

```sh
mark diff --repo ../project
mark show --repo ../project HEAD~1
```

Use `--no-watch` to disable local worktree reloads for one run, and
`--no-syntax` to disable syntax highlighting for one run:

```sh
mark diff --no-watch
mark diff --no-syntax
```

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

## Interactive controls

Common default controls:

```text
q / Ctrl-C     quit
?              help
j / Down       scroll down or move focus (accepts a count, e.g. 3j)
k / Up         scroll up or move focus (accepts a count, e.g. 3k)
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
v / V          enter / leave linewise Visual mode
Enter          annotate the selected line, range, hunk, or file
A              annotate and advance
n              annotation search menu
b              file browser
s              toggle split/unified layout
Ctrl-G         open the viewport line (full file) or focused hunk in the editor
y              copy marks to the terminal clipboard
Ctrl-U         clear filters
{ / }          previous / next annotation
Ctrl-Shift-C   copy the error log pane to the terminal clipboard
```

Editor commands are shell-word parsed and open on the line focused in the diff.
Mark knows the location syntax used by Vim/Neovim, Helix, Kakoune, Emacs,
nano/pico, micro/vis, VS Code/Codium/Cursor, Sublime Text, and Zed. For another
editor or a wrapper script, put `{file}`, `{line}`, and optionally `{column}` in
the command, for example `EDITOR='my-editor --location {file}:{line}:{column}'`.

The annotation row highlight is active in the diff by default. Move it with
`j` / `k` or Up / Down; prefix cursor motions with a count to repeat them (`3j`,
`5k`, `4l`). `d` / `u` move half a viewport, Page Up / Page Down move a full
viewport, and `g` / `G` jump to the ends. Like Vim with `scrolloff=8`, the
selection moves freely until it is eight rows from the top or bottom, then stays
there while the viewport scrolls. The margin shrinks for short viewports and at diff boundaries.

Press `v` or `V` for linewise Visual mode, extend the selection with the same
motions, and press `Enter` to annotate the range. The selection is clamped to
the current hunk (or contiguous full-file context block), so motions cannot land
on context controls or another file. The status line shows the selected side and
source lines, such as `VISUAL +121–123 · 3 lines`. `Esc`, `v`, or `V` cancels the
selection.

Visual mode shows only the selected-line tint. After `Enter` turns the selection
into a draft, line and range notes use a square gutter rail connected to an
inline card after the final target line. In split view, targeting is
row-deterministic rather than tied to the initiating pane: rows use the new/right
side when present and otherwise the old/left side. A selection that would form
disjoint source ranges cannot be stored as one note and must be shortened.

Outside Visual mode, press `Enter` on a code line, hunk header, or file header to
annotate that line, entire hunk, or entire file.

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
