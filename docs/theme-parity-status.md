# TextMate theme status

Syntaxmate supplies Mark's exact TextMate token ranges and scope stacks. Mark
keeps product-specific theme selection, TUI colors, coarse syntax fallbacks,
user overrides, and terminal rendering in `crates/mark-syntax` and `mark-tui`.

| Theme family | Scope input | Asset source | Status |
|---|---|---|---|
| GitHub Dark/Light and high-contrast | Syntaxmate exact scopes | `github-vscode-themes@6.3.4` | Enabled |
| Catppuccin, Gruvbox, Tokyo Night, Ayu, Kanagawa, Everforest, Nord | Syntaxmate exact scopes | `@shikijs/themes@4.4.3` | Enabled |
| Zenbones, Token, Gruvbox Material, MFD, Origin | Syntaxmate exact scopes | pinned upstream repositories | Enabled |
| System, ANSI, user Base16 | Mark coarse fallback and terminal palette | user/terminal-defined | Intentional fallback |

Mark's Rust and TUI regression suites verify exact-scope resolution, modifiers,
custom selector overrides, fallback behavior, and every named built-in theme.
Syntaxmate independently validates tokenizer and selector compatibility against
pinned `vscode-textmate` and `vscode-oniguruma` versions in its own release CI.

Theme source hashes and license records remain under `assets/themes/`; validate
them with:

```sh
npm ci --prefix tools/theme-assets --ignore-scripts
python3 tools/check-textmate-theme-assets.py
node tools/vendor-textmate-themes.mjs --check
```
