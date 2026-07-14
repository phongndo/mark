# Configuration

`mark` works without a config file. Create one only when you want to override
syntax behavior, colors, diff rendering, highlight performance limits, or
keybindings.

Print the exact config path for the current machine:

```sh
mark config
```

On XDG systems this is usually `~/.config/mark/config.toml`. `XDG_CONFIG_HOME` is
honored. Windows uses `APPDATA` when `XDG_CONFIG_HOME` is unset.

Syntax language state is stored separately as `syntax.json` under the same
`mark` config directory. Mark includes a Rust-native TextMate backend and a
curated core grammar pack.
Inspect syntax mappings, config, and colorscheme paths with:

```sh
mark syntax path
```

## Example

```toml
mode = "builtin"
colorscheme = "system"
transparent_background = false
layout = "dynamic"
live_reload = true
syntax_highlighting = true
line_wrapping = false

[decorations]
mode = "auto"
empty_fill = true
no_borders = false

[notifications]
mode = "default"
corner = "top-right"
timeout_ms = 1500
max_visible = 3

[diff]
line_background = "subtle"
gutter_background = "delta"
inline_background = "strong"
sign_style = "bold"

[limits]
max_source_kib = 1024
max_line_kib = 8
cache_entries = 512
cache_kib = 65536
queue_entries = 512
queue_kib = 65536
prefetch_viewports = 1
worker_threads = 4

[keymap.global]
help = "?"
reload = "r"
file_filter = "f"
grep = "/"
diff_menu = "m m"
review_target = "m r"
head_branch = "m h"
base_branch = "m b"
commit_picker = "m c"
options_menu = "o"
annotation_menu = "n"
file_browser = "b"
previous_file = "shift-tab"
next_file = "tab"
previous_hunk = "["
next_hunk = "]"
expand_context_up = ","
expand_context_down = "."
collapse_context_all = "c"
quit = "q"
layout = "s"
line_wrapping = "w"
horizontal_scroll_lock = "x"
edit_hunk = "ctrl-g"
save_mark = "ctrl-s"
cancel_mark = "esc"
copy_marks = "y"
copy_error_log = "ctrl-shift-c"
clear_filters = "ctrl-u"
next_diff_type = []
previous_diff_type = []
next_annotation = "}"
previous_annotation = "{"

[keymap.annotation_menu]
jump = "enter"
edit_external = "ctrl-g"
remove = "ctrl-x"

[keymap.menu]
up = ["up", "shift-tab", "ctrl-p"]
down = ["down", "tab", "ctrl-n"]
select = []
confirm = "enter"
close = "esc"
```

## Syntax mode

`mode` controls which languages are eligible for syntax highlighting:

```toml
mode = "builtin"
```

Supported values:

- `builtin` - all languages bundled by the active syntax backend. This is the default.
- `enabled` - languages explicitly selected with `mark syntax add`, plus installed core languages.
- `all` - currently equivalent to `builtin`; kept for config compatibility.

<!-- BEGIN GENERATED: language-counts -->
The bundled native backend supports **256 public language IDs**. **256 are validated** by the complete generated contract; **0 more are supported** by real bundled grammars and the catalog-wide smoke/budget gate. See [`language-status.md`](language-status.md) for the generated per-language ledger, or run `mark syntax available --installed` for the runtime catalog.
<!-- END GENERATED: language-counts -->

Use `mark --no-syntax`, `mark diff --no-syntax`, `mark show --no-syntax`, or
`mark patch --no-syntax changes.diff` to disable syntax highlighting for one run.

## Colorschemes and colors

Use a built-in colorscheme by name:

```toml
colorscheme = "system"
```

Built-in colorschemes are `system`, `catppuccin-latte`,
`catppuccin-frappe`, `catppuccin-macchiato`, `catppuccin-mocha`,
`gruvbox-dark`, `gruvbox-light`, `github-dark`,
`github-dark-high-contrast`, `github-light`, `github-light-high-contrast`,
and `tokyonight`.

Changing Colorscheme in the interactive settings menu updates the
`colorscheme` config value.

Base16 themes can be loaded from a file:

```toml
[colorscheme]
source = "base16"
path = "~/themes/example.yaml"
```

Individual colors can be overridden at the top level or in `[colors]`:

```toml
bg = "#111315"
fg = "#d8dee9"

[colors]
addition_bg = "#1f3025"
deletion_bg = "#372526"
keyword = "ansi-5"
string = "green"
```

Color values support hex colors, ANSI indexes such as `ansi-5`, and named
terminal colors.

`cursor` colors the block caret in all text inputs (filter bar, menus, review ID, and
annotation compose). Built-in schemes usually set it to the palette foreground (often the
scheme's white or brightest text), similar to Neovim's normal-mode cursor. Override with
`cursor` in config or `[colors]`. `cursor_line_bg` colors the mouse-hover highlight on diff
code columns (Neovim-style cursorline). With `colorscheme = "system"`, `cursor` uses the
terminal default foreground (`reset`) and `cursor_line_bg` uses ANSI palette index 237 so
both follow the emulator theme unless overridden in `[colors]`.

Common override keys include:

```text
bg, fg, header, file, hunk, notice, cursor, cursor_line_bg, muted, gutter_bg, empty_diff,
search_match_fg, search_match_bg,
statusline_fg, statusline_bg, statusline_accent_fg, statusline_accent_bg,
statusline_info_fg, statusline_info_bg,
addition_fg, addition_gutter_bg, addition_bg, addition_inline_bg,
deletion_fg, deletion_gutter_bg, deletion_bg, deletion_inline_bg,
attribute, comment, constant, constructor, function, keyword, label, module,
number, operator, property, punctuation, string, tag, type, variable
```

## Diff rendering

Top-level UI settings are read at startup and mirror the interactive settings
menu:

```toml
layout = "dynamic"           # dynamic, unified, split
live_reload = true
syntax_highlighting = true
line_wrapping = false

[decorations]
mode = "auto"                 # auto, fancy, minimal
empty_fill = true              # fancy mode draws the diagonal empty-cell fill
no_borders = false             # true removes pane borders even in fancy mode
```

`layout = "dynamic"` uses split when the terminal is wide enough and unified
when it is narrow.
`[decorations] mode = "auto"` uses Mark's fancy UI on capable UTF-8 terminals and a
minimal low-chrome UI on constrained terminals such as `TERM=dumb` or non-UTF-8
locales. Set `mode = "fancy"` or `mode = "minimal"` to force a mode. Minimal
mode avoids decorative glyphs and borders; it uses spacing, labels, and
background panes instead of ASCII-art replacements. `empty_fill` is enabled by
default for fancy mode and suppressed by minimal mode.
`MARK_DECORATIONS=minimal` and `MARK_ASCII=1` also request minimal decorations
for one process.
For captured/static pager output, `MARK_STATIC_RAW_FALLBACK_BYTES` controls the
patch-size threshold where Mark skips formatted static rendering and prints the
sanitized raw diff instead. The default is 128 MiB.
The settings menu can change the decoration mode for the current session;
`empty_fill` is config/CLI-controlled, and `no_borders` is config-only.
Only Colorscheme changes are written back to config.

## Notifications

Toasts are separate from the error pane. The error pane is still used for
errors and longer diagnostic text; toasts are for short, transient feedback.

```toml
[notifications]
mode = "default"      # default, debug
corner = "top-right"  # top-left, top-right, bottom-left, bottom-right
timeout_ms = 1500
max_visible = 3
```

`mode = "default"` stays quiet except for feedback that would otherwise be hard
to infer, such as a copy action completing or a requested action having no
matching target. `mode = "debug"` also emits a toast for each terminal event to
make UI behavior easier to trace.

The configured `timeout_ms` is clamped to 10,000 ms.

These notification settings can also be changed from the interactive settings
menu for the current session.

`[diff]` controls visual emphasis:

```toml
[diff]
line_background = "subtle"   # none, subtle, strong
gutter_background = "delta"  # base, delta
inline_background = "strong" # none, subtle, strong
sign_style = "bold"          # normal, bold
```

`word_background` and `word_diff_background` are accepted aliases for
`inline_background`. Legacy `[diff] empty_fill` and `empty_diff_fill` are
accepted as aliases for `[decorations] empty_fill`. Collapsed unchanged context expands fully when clicked.
Legacy `context_lines`, `context_expand`, and `expand_context` settings are
accepted for compatibility but no longer limit interactive context expansion.

## Highlight limits

Large files and very long lines are intentionally bounded:

```toml
[limits]
max_source_kib = 1024
max_line_kib = 8
cache_entries = 512
cache_kib = 65536
queue_entries = 512
queue_kib = 65536
prefetch_viewports = 1
worker_threads = 4
```

`worker_threads` defaults to half of available CPUs, capped at 4. Each worker
owns its own highlighter cache; queue and cache byte budgets still apply.

Diff input safety limits can be set per process with environment variables.
When set, Mark stops loading and reports the exceeded limit instead of trying to
materialize an unbounded diff:

```sh
MARK_MAX_PATCH_BYTES=1073741824
MARK_MAX_DIFF_ROWS=10000000
MARK_MAX_DIFF_FILES=100000
MARK_MAX_DIFF_HUNKS=100000
MARK_MAX_DIFF_LINE_BYTES=1048576
```

Large multi-file patches are parsed on the lazy process-wide CPU pool. The
default pool size is the physical-core count capped at eight. Override the
shared parse/search pool for benchmarking or force serial work with:

```sh
MARK_CPU_THREADS=4   # capped at 8; set 0 or 1 to force sequential work
```

## Keybindings

Global bindings can be one-key or two-key bindings. A one-key global binding
cannot also be used as a two-key prefix. Menu bindings are single-key and apply
to searchable menus. Printable menu bindings override text input, so prefer
non-printing keys to keep type-to-filter behavior. `edit_hunk`, `save_mark`, and
`cancel_mark` must be single-key bindings.

Bindings can be a string or a list of strings:

```toml
[keymap.global]
help = ["?", "h ?"]
file_filter = "ctrl-f"
diff_menu = "m m"
review_target = "m r"
head_branch = "m h"
base_branch = "m b"
commit_picker = "m c"
copy_marks = "y"
save_mark = "ctrl-s"
cancel_mark = "esc"
quit = "q"

[keymap.menu]
up = ["up", "ctrl-p"]
down = ["down", "ctrl-n"]
select = []
confirm = "enter"
close = "esc"
```

Use an empty list, such as `copy_marks = []`, or an empty string, such as
`copy_marks = ""`, to unbind an action.

Key names include printable characters plus names such as `space`, `enter`,
`esc`, `tab`, `shift-tab`, `up`, `down`, `left`, `right`, and modified keys such
as `ctrl-s`.

If the keymap cannot be parsed, `mark` ignores it for that run and shows a notice
inside the TUI instead of failing the diff review.
