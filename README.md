# mark

[![Quality](https://github.com/phongndo/mark/actions/workflows/quality.yml/badge.svg?branch=main)](https://github.com/phongndo/mark/actions/workflows/quality.yml)
[![Latest release](https://img.shields.io/github/v/release/phongndo/mark)](https://github.com/phongndo/mark/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A fast, keyboard-first Git diff reviewer for the terminal.

<p align="center">
  <img src="docs/assets/mark-demo.png" alt="Mark reviewing a syntax-highlighted diff" width="100%">
</p>

Review local changes, commits, patches, pager input, difftool pairs, and pull
requests in one focused interface.

## Install

The installer supports macOS and Linux on `aarch64` and `x86_64`.

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | sh
```

Update in place:

```sh
mark update
```

## Quick start

```sh
mark diff                    # local changes
mark compare main            # workspace against main
mark compare main feature    # two revisions
mark show HEAD~1             # one commit
mark review 123              # pull request
mark patch changes.diff      # patch file
git diff | mark pager        # pager input
mark diff --watch            # live reload
```

Run a review command directly; bare `mark` currently exits without opening the
interface.

## Built for review

- Split and unified layouts with syntax highlighting.
- File filtering, diff search, expandable context, and full-file view.
- Inline line, hunk, and file annotations with reviewed-state tracking.
- Stable snapshots by default; continuous reload only with `--watch`.
- Custom themes, keybindings, and editor integration.
- Viewport-bounded rendering for very large diffs.
- Private local sessions for review automation, with no daemon or hosted state.

## Controls

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up |
| `[` / `]` | Previous / next hunk |
| `Shift-Tab` / `Tab` | Previous / next file |
| `f` / `/` | Filter files / search the diff |
| `Enter` | Add an annotation |
| `s` | Toggle split / unified layout |
| `r` | Reload |
| `?` | Show all controls |
| `q` | Quit |

Motions accept counts such as `3j`.

## Git integration

Use Mark as a pager:

```sh
git config --global core.pager "mark pager"
```

Use Mark as a difftool:

```sh
git config --global diff.tool mark
git config --global difftool.mark.cmd 'mark difftool -- "$LOCAL" "$REMOTE" "$MERGED"'
```

## Documentation

Mark works without configuration. Run `mark config` to find the config file and
`mark --help` for the complete command list.

[Usage](docs/usage.md) · [Configuration](docs/configuration.md) ·
[Development](docs/development.md) · [Contributing](CONTRIBUTING.md)

## License

[MIT](LICENSE)
