# TextMate grammar assets

Vendored TextMate grammar source assets for the in-house engine migration.

Phase 4 packages a **core-30** public language catalog from recovered assets:

`rust`, `c`, `cpp`, `csharp`, `go`, `python`, `java`, `kotlin`, `swift`,
`ruby`, `php`, `lua`, `javascript`, `jsx`, `typescript`, `tsx`, `bash`,
`powershell`, `html`, `css`, `scss`, `json`, `yaml`, `toml`, `markdown`, `sql`,
`dockerfile`, `make`, `nix`, `terraform`.

Private dependency grammars are also embedded without becoming public catalog
languages. They include `cpp-macro` and every external root scope referenced by
the Markdown grammar, so fenced blocks retain their exact embedded grammar
instead of silently falling back to plain Markdown.

Asset filename remaps:

- `bash` → `shellscript.tmLanguage.json`
- `dockerfile` → `docker.tmLanguage.json`

The files under `languages/` are real grammar JSON objects, primarily imported
from the pinned `@shikijs/langs` package recorded in `SOURCE.toml`; the two VS
Code dependencies and YANG source are recorded there as additional MIT
sources. They are committed as text so diffs remain reviewable. The generated
runtime bundle (`bundle.bin`) is not committed; it is produced by `build.rs` /
`grammar-compile`.

`licenses.json` records source package, version, license, scope name, module, and
path for every vendored grammar. `coverage.toml` records the public keep/remap
list for the core-30 catalog and its private dependency blobs. Regenerate the
Markdown dependencies with `node tools/vendor-markdown-grammars.mjs`.
