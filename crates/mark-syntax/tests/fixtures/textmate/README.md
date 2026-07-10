# TextMate golden fixtures

Source fixtures and checked-in `vscode-textmate` oracle output for the core-30
native engine migration. The harness lives in
`crates/mark-syntax/tests/textmate_golden.rs`.

## Layout

| Path | Role |
| --- | --- |
| `cases.toml` | Manifest of oracle cases (`[[case]]` + optional `[[case.embedded]]`) |
| `divergences.toml` | Explicit allowlist for known engine ≠ oracle ranges |
| `<lang>/basic.*` | Small exact-parity gates (json, rust, yaml, python) |
| `<lang>/stress.*` | Broader constructs for the original core fixture set |
| `<lang>/smoke.*` | Small smoke fixtures for the remaining core-30 languages |
| `<lang>/*.golden.jsonl` | Generated oracle output — do not hand-edit |

## Core-30 language ids

These match the vendored grammar assets under `assets/tm-grammars/languages/`
(plus `cpp-macro` as an embedded support grammar for C++):

`bash`, `c`, `cpp`, `csharp`, `css`, `docker`, `go`, `html`, `java`,
`javascript`, `json`, `jsx`, `kotlin`, `lua`, `make`, `markdown`, `nix`, `php`,
`powershell`, `python`, `ruby`, `rust`, `scss`, `sql`, `swift`, `terraform`,
`toml`, `tsx`, `typescript`, `yaml`

Notes:

- Language id `bash` uses the `shellscript.tmLanguage.json` asset (`source.shell`).
- Language id `docker` uses `docker.tmLanguage.json` (`source.dockerfile`).
- Markdown, HTML, SCSS, PHP, and C++ cases load embedded grammars via
  `[[case.embedded]]` so oracle output includes injected/include scopes.

## Regenerate oracle goldens

Install the pinned Node oracle (dev-only):

```sh
npm install --prefix tools/golden-oracle
```

From the repository root:

```sh
node tools/generate-goldens.mjs
node tools/generate-goldens.mjs --check
node tools/generate-goldens.mjs --case rust
```

Ad-hoc single dump:

```sh
node tools/golden-dump.mjs \
  --language rust \
  --scope source.rust \
  --grammar assets/tm-grammars/languages/rust.tmLanguage.json \
  --file crates/mark-syntax/tests/fixtures/textmate/rust/basic.rs \
  --out crates/mark-syntax/tests/fixtures/textmate/rust/basic.golden.jsonl
```

## Comparison modes

The Rust harness compares:

1. **Exact** scope stacks (UTF-16 oracle offsets converted to UTF-8 byte ranges).
2. **Coarse** `SyntaxClass` segments after scope classification.

`divergences.toml` can skip `exact`, `coarse`, or `any` for a 1-based line
range. Unused allowlist entries fail the harness (stale-divergence detection).

Today:

- Basic json/rust/yaml/python cases have **no** allowlist (exact gates).
- Stress and smoke cases are allowlisted with `mode = "any"` while the in-house
  tokenizer is still reaching exact TextMate parity. Oracle goldens remain the
  correctness reference; remove allowlist entries only when the engine matches.

## Property tests

`textmate_golden.rs` also covers non-oracle properties:

- no panic on generated UTF-8 inputs
- zero-width matches advance / do not loop
- checkpoint replay matches full replay
- fallback step-budget kills pathological patterns without hanging the line
