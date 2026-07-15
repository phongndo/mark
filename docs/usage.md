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
mark diff --base main
mark diff main feature
```

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

Static diff output reuses mark's renderer, colorscheme, and layout. It falls
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
j / Down       scroll down or move focus
k / Up         scroll up or move focus
d / Ctrl-D / PgDn page down
u / PageUp     page up
g              top
G              bottom
]              next hunk
[              previous hunk
( / )          previous / next file
, / .          expand context up / down
c              collapse expanded context
f              file filter
/              grep filter
n / p          next / previous grep match
r              reload
m              diff type selector
o              settings menu
a              annotate a visible line
A              batch annotate visible lines
n              annotation search menu
b              file browser
s              toggle split/unified layout
Ctrl-G         open the focused hunk in `$VISUAL`, `$GIT_EDITOR`, or `$EDITOR`
y              copy marks to the terminal clipboard
Ctrl-U         clear filters
{ / }          previous / next annotation
Ctrl-Shift-C   copy the error log pane to the terminal clipboard
Tab            next diff type
Shift-Tab      previous diff type
```

Press `a` to label every annotatable line in the viewport, then type a displayed
hint to open an inline annotation draft. Lines in the focused hunk receive the
easier hints first. Hints temporarily replace the target-side line number while
preserving the `+`/`-` sign. In split layout, replacement and context hints
appear on the right/current side; deletion-only hints appear on the left/old
side. After each typed character, matching labels show only their remaining
suffix and nonmatching lines restore their line numbers. Existing annotations
use an underlined annotation accent. Backspace removes a partial hint; Esc
cancels targeting. Arrow and page keys cancel targeting and continue navigation.

Press `A` for sticky review mode. After selecting and saving an annotation, the
hints reappear so another visible line can be selected. Press Esc while writing
or targeting to leave the batch workflow.

Selector panes keep focus in the filter input: type to filter, Enter selects or
toggles, Esc closes, and arrows, Tab / Shift-Tab, or Ctrl-N / Ctrl-P move the
highlighted row. Settings with multiple values can also cycle with Left / Right.

Keybindings can be customized in the user config file. See
[configuration](configuration.md#keybindings).

## Syntax languages

Mark includes a Rust-native TextMate engine with 30 bundled languages. The
engine uses the same grammar model as VS Code while emitting Mark's compact
terminal syntax classes:

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

## Pi package

The `pi-mark` package adds a `/mark` slash command to Pi. It shells out to
`mark`, so install the CLI first and keep it on `PATH`, or set `PI_MARK_BIN` to
the executable path.

See [`../pi-mark/README.md`](../pi-mark/README.md).
