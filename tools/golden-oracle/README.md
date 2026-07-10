# Golden oracle dependencies

Development-only package used by `../golden-dump.mjs`, `../generate-goldens.mjs`,
and `../regex-conformance.mjs`. Versions are pinned exactly (no ranges) so
oracle output stays reproducible.

These dependencies are **not** used by release builds and are intentionally kept
out of the Rust workspace. Install only when regenerating goldens or running
regex conformance.

## Install

```sh
npm install --prefix tools/golden-oracle
```

## Regenerate TextMate goldens

From the repository root, with the pinned grammar assets under
`assets/tm-grammars/`:

```sh
# all cases in the manifest
node tools/generate-goldens.mjs

# one language id (matches [[case]].language)
node tools/generate-goldens.mjs --case rust
node tools/generate-goldens.mjs --case java

# fail if committed goldens differ (CI-friendly)
node tools/generate-goldens.mjs --check
```

Ad-hoc single file:

```sh
node tools/golden-dump.mjs \
  --language rust \
  --scope source.rust \
  --grammar assets/tm-grammars/languages/rust.tmLanguage.json \
  --file crates/mark-syntax/tests/fixtures/textmate/rust/basic.rs \
  --out crates/mark-syntax/tests/fixtures/textmate/rust/basic.golden.jsonl
```

## Regex conformance helper

```sh
node tools/regex-conformance.mjs
# optional: --out target/regex-conformance-phase2.json
```

This compares a small set of patterns against `vscode-oniguruma` by driving the
`mark-syntax` `regex-parse` example. It requires a working `cargo` toolchain and
is also dev-only.

## Pins

| Package | Version | Role |
| --- | --- | --- |
| `vscode-textmate` | `9.2.0` | TextMate line tokenizer reference |
| `vscode-oniguruma` | `1.7.0` | Oniguruma WASM used by the reference |

Bump both together, reinstall with the lockfile, then regenerate goldens and
review the diff.
