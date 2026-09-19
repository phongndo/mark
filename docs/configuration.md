# Configuration

Mark works without a config file. Run `mark config` for the path to edit and
`mark syntax path` for language-state and custom-theme paths. Prefer the
interactive settings menu (`o`) for supported appearance changes; it writes
those choices back to the config.

The snippets below are independent examples of overrides, not a complete
configuration or a copy of defaults. Omit settings you do not need. For the
full accepted fields and aliases, see `StoredSyntaxSettings` and related types
in [types.rs](../crates/mark-syntax/src/types.rs); resolution and defaults live
in [storage.rs](../crates/mark-syntax/src/storage.rs) and those types.

## Appearance

```toml
theme = "nord"
layout = "unified"
line_wrapping = true

[decorations]
mode = "minimal"
```

Choose built-in themes from the settings menu rather than maintaining a copied
catalog. `layout` accepts `dynamic`, `split`, or `unified`; dynamic follows the
terminal width. Decorations accept `auto`, `fancy`, or `minimal`. `mark diff
--minimal` overrides decorations for a run.

For diff emphasis:

```toml
[diff]
line_background = "subtle"    # none, subtle, strong
gutter_background = "delta"  # base, delta
inline_background = "strong" # none, subtle, strong
sign_style = "bold"          # normal, bold
```

`full_file = true` at the top level starts with unchanged source context where
available. It cannot recover source absent from a patch. Watch behavior is
selected when opening a review; see [snapshots and reloads](usage.md#snapshots-and-reloads).

### Custom themes

Put `my-theme.toml` in the theme directory reported by `mark syntax path`:

```toml
extends = "nord"

[colors]
bg = "#101419"
fg = "#d8dee9"
addition_fg = "#a3be8c"
keyword = "#b48ead"
```

Then select it in the main config with `theme = "my-theme"`. Inheritance keeps
the parent's syntax rules; override only what you need. Main-config `[colors]`
can also override theme colors. Values accept hex, ANSI indexes such as
`ansi-5`, and named terminal colors. `ColorOverrides` in
[types.rs](../crates/mark-syntax/src/types.rs) defines available color keys.

For an existing Base16 palette, use a path instead:

```toml
[theme]
source = "base16"
path = "~/themes/example.yaml"
```

## Keybindings

Use `?` in the TUI to see the current bindings. For all configurable action names
and their defaults, see the action specifications in
[keymap.rs](../crates/mark-tui/src/keymap.rs).

```toml
[keymap.global]
help = ["?", "h ?"]
file_filter = "ctrl-f"
copy_marks = []

[keymap.menu]
up = ["up", "ctrl-p"]
down = ["down", "ctrl-n"]
```

A string binds one sequence; a list gives alternatives; `[]` unbinds an action.
Global actions generally accept one or two keys, but a one-key binding cannot
also be a sequence prefix. Actions constrained to a single key declare that in
the action specifications. Menu bindings are single-key; printable keys consume
filter input, so prefer non-printing keys there. Invalid keymaps fall back to
defaults with a TUI notice.

## Annotation targeting

Cursor targeting is the default. To use label-jump hints instead:

```toml
[annotations]
targeting = "hints"
hint_keys = "asdfghjklqwertyuiopzxcvbnm"
uppercase_hints = false
```

Hint keys must contain at least two printable, single-cell characters, unique
ignoring ASCII case. Uppercase display still accepts either case.

## Syntax and resource limits

`mode = "builtin"` enables bundled languages. With `mode = "enabled"`, only
explicitly selected languages and the core set are enabled. Use the syntax CLI
to manage language selection and path mappings, for example:

```sh
mark syntax add ruby --ext rake --filename Rakefile
mark syntax available --enabled
```

Use `--no-syntax` on a review command to disable highlighting for that run.
To limit highlighting work, override only the relevant budgets:

```toml
[limits]
max_source_kib = 512
max_line_kib = 4
worker_threads = 2
```

Queue, cache, prefetch, and worker settings are defined by `StoredSyntaxLimits`
and `SyntaxLimits` in [types.rs](../crates/mark-syntax/src/types.rs). Raising a
source limit can increase both latency and memory; it does not merely enable
another language.

Diff ingestion and full-file context reads have separate process-level limits.
For example, reject patches larger than 64 MiB with:

```sh
MARK_MAX_PATCH_BYTES=67108864 mark patch changes.diff
```

For all ingress overrides, see `DiffLimits::from_env` in
[mark-diff types.rs](../crates/mark-diff/src/types.rs). Full-file read limits
live in [syntax/source.rs](../crates/mark-tui/src/syntax/source.rs). They protect
different allocations from the highlighting budgets above.
