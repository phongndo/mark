# mark

[![Quality](https://github.com/phongndo/mark/actions/workflows/quality.yml/badge.svg?branch=main)](https://github.com/phongndo/mark/actions/workflows/quality.yml)
[![Latest release](https://img.shields.io/github/v/release/phongndo/mark)](https://github.com/phongndo/mark/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`mark` is a fast, keyboard-first terminal Git diff reviewer. It opens local
changes, commits, patches, pager input, difftool pairs, and GitHub pull requests
in the same focused review UI.

<p align="center">
  <img src="docs/assets/mark-demo.png" alt="Mark reviewing a focused, syntax-highlighted Rust change in the terminal" width="100%">
</p>

<p align="center">
  <sub>A focused Rust change in Mark, rendered in Catppuccin Mocha. Reproduce it with <a href="docs/assets/readme-demo.tape">this VHS tape</a>.</sub>
</p>

## What it does

- **Review any changeset.** Open the worktree, revision ranges, commits, patch
  files, stdin, difftool pairs, or a GitHub pull request by number or URL.
- **Move without waiting.** Jump between files, hunks, matches, the top, or the
  bottom while rendering only the visible viewport.
- **Find the relevant change.** Filter files, grep the diff, expand context, or
  switch from hunks to the full file without leaving the reviewer.
- **Leave review context.** Move the highlighted annotation row across the diff
  to add inline annotations one at a time or in sticky batch mode.
- **Work the way you want.** Toggle split and unified layouts, choose a built-in
  or custom theme, customize keybindings, and open the focused code in your
  editor.
- **Fit into Git.** Use Mark directly, as `core.pager`, or as a Git difftool.
  Reviews stay stable by default, report source changes, and reload continuously
  only with `--watch`.
- **Review with an external agent.** A private local session lets an existing
  shell-driven agent inspect bounded patches and place inline comments while
  the human keeps control of the TUI.

## Install

The supported install path is the shell installer for macOS and Linux on
`aarch64` and `x86_64`:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | sh
```

Homebrew, mise, Cargo, and other package-manager installs are deprecated for
now. Reinstall with the command above if you used one of those paths before.

Installer environment variables use the `MARK_` prefix:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | MARK_VERSION=0.11.0 sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | MARK_INSTALL_DIR=/usr/local/bin sh
```

Update a curl-installed binary in place:

```sh
mark update
mark update --target-version 0.11.0
```

Nightly builds are published from `main` as a prerelease channel. Switch the
installed `mark` binary to nightly, then back to stable, with:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | MARK_VERSION=nightly sh
mark update
```

Once on stable, switch to nightly again with:

```sh
mark update --target-version nightly
```

Nightly binaries report their channel and build commit in `mark --version`.

## Quick start

```sh
mark diff                    # review all local changes
mark compare main            # compare main with the current workspace
mark compare main feature    # compare two branches or commits
mark show HEAD~1             # review one commit
mark review 123              # review GitHub PR #123 from the current repo
mark patch changes.diff      # review an existing patch file
git diff | mark pager        # use mark as a diff pager
mark diff --watch            # opt into continuous worktree reload
```

Bare `mark` is reserved for the upcoming dashboard and currently exits without
output. While reviewing, move the cursor with Vim-style motions and counts such
as `3j`. Press `v` or `V` for linewise
Visual mode, select a range, and press `Enter` to annotate it; `Enter` also
annotates a single line, hunk header, or file header. Use `y` to copy annotations and
`Shift-Q` to copy them and quit (`q` quits without submitting them).

## Live agent review

Open Mark yourself, then let an external agent use the bundled local workflow:

```sh
mark skill path
mark session list --json
mark session review --repo . --json
mark session patch --repo . --file src/lib.rs --hunk 1 --json
```

Agents can apply atomic, source-anchored comment batches that appear inline in
the open review. While the TUI remains open, review passes retain comments and
reviewed progress, report changed files, and conservatively re-anchor
unambiguous findings while the human owns dispositions and the final verdict.
Closing Mark discards the review state. The interface uses a private local Unix
socket, bounded JSON frames, and no daemon, model integration, telemetry, or
network service. See
[Usage](docs/usage.md#live-agent-review-sessions) for the full command flow.

## Built for huge diffs

Mark keeps the hot path viewport-bounded instead of rebuilding the entire
screen model for every action. That matters on ordinary reviews, and it keeps
navigation responsive when a generated change or pull request becomes enormous.

A committed Apple Silicon reference run measured the synthetic one-million-row
diff fixture as follows:

| Diff | Load | Open | Grep | Random-scroll max | RSS increase |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1,000,000 rows / 74.0 MB | 6.4 ms | 11.9 ms | 3.2 ms | 274 µs | 59 MB |

These are reference-machine benchmark results, not latency promises for every
machine or repository. The fixture, commands, allocation accounting, and
10-million-row local run are documented in the
[allocation performance report](docs/performance-reports/2026-08-08-allocation-profile.md).

## How it compares

These tools solve related but different problems. Mark,
[Tuicr](https://github.com/agavra/tuicr), and
[Hunk](https://github.com/modem-dev/hunk) are interactive reviewers,
[delta](https://github.com/dandavison/delta) is primarily a syntax-highlighting
pager, and [Difftastic](https://github.com/Wilfred/difftastic) is a structural
diff engine.

| Built-in capability | Mark | Tuicr | Hunk | delta | Difftastic |
| --- | :---: | :---: | :---: | :---: | :---: |
| Interactive multi-file review UI | Yes | Yes | Yes | — | — |
| Split / side-by-side view | Yes | Yes | Yes | Yes | Yes |
| Runtime layout switching | Yes | Yes | Yes | — | — |
| Inline review annotations | Yes | Yes | Yes | — | — |
| Persistent review sessions | — | Yes | — | — | — |
| GitHub review submission | — | Yes | — | — | — |
| GitLab merge request review | — | Yes | — | — | — |
| Live worktree or file reload | Yes | — | Yes | — | — |
| File filtering | Yes | Yes | Yes | — | — |
| In-diff text search | Yes | Yes | — | — | — |
| Direct GitHub pull request review | Yes | Yes | — | — | — |
| Git pager workflow | Yes | — | Yes | Yes | — |
| Git difftool / external-diff workflow | Yes | — | Yes | — | Yes |
| Native non-Git VCS support | — | Jujutsu, Mercurial | Jujutsu, Sapling | — | — |
| Structural, syntax-aware diff algorithm | — | — | — | — | Yes |
| Syntax highlighting | Yes | Yes | Yes | Yes | Yes |

The table compares documented, built-in workflows. Each tool can be composed
with other Git and shell commands beyond what is listed here.

## Git integrations

Use `mark pager` as a Git pager for `git diff` and `git show` output:

```sh
git config --global core.pager "mark pager"
```

Use `mark difftool` as a Git difftool for Git-provided file pairs:

```sh
git config --global diff.tool mark
git config --global difftool.mark.cmd 'mark difftool -- "$LOCAL" "$REMOTE" "$MERGED"'
```

## Documentation

- [Usage](docs/usage.md) - commands, diff sources, pager, difftool, and GitHub
  reviews.
- [Configuration](docs/configuration.md) - config paths, syntax settings,
  colors, diff rendering, and keybindings.
- [Development](docs/development.md) - setup, checks, and release flow.
- [Contributing](CONTRIBUTING.md) - repository standard and PR expectations.

## Development

Use the Nix shell when available:

```sh
nix develop
just check
```

Without Nix, install the Rust toolchain from `rust-toolchain.toml` and run:

```sh
cargo fetch --locked
cargo build -p mark-cli --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
```

## Workspace layout

```text
crates/mark-cli       command parsing, update, and CLI UX
crates/mark-command   command facade shared by CLI and future integrations
crates/mark-core      shared errors and path helpers
crates/mark-git       low-level Git process boundary
crates/mark-diff      diff loading, parsing, and plain rendering
crates/mark-session   local session protocol, registry, and Unix transport
crates/mark-syntax    thin Mark settings/rendering adapter over syntaxmate
crates/mark-tui       ratatui/crossterm diff review UI
crates/mark-bench     local benchmark fixture generation
```

## License

MIT. See [LICENSE](LICENSE).
