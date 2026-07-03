# TextMate syntax migration

This file tracks the hard migration from native parser based syntax highlighting
to a bundled TextMate grammar engine.

## Baseline

- [x] Captured legacy syntax benchmark before deleting the old backend:
  `target/legacy-native-syntax-bench.json`
- [x] Captured Shiki benchmark for the same Rust syntax fixtures:
  `target/shiki-syntax-bench.json`
- [x] Captured post-migration benchmark for the new TextMate backend:
  `target/textmate-syntax-bench-release.json`
- [x] Captured range-only syntax segment experiment:
  `target/range-only-syntax-bench-release.json`
- [x] Captured final optimized Rust benchmark:
  `target/final-rust-syntax-release.json`
- [x] Captured TypeScript syntect baseline and fast-path benchmark:
  `target/textmate-typescript-syntax-baseline-release.json`,
  `target/final-typescript-syntax-release.json`
- [x] Added raw apples-to-apples Shiki comparator to `mark-bench syntax-compare`
- [x] Captured raw Mark-vs-Shiki full-source comparisons:
  `target/raw-compare-rust-many-small.json`,
  `target/raw-compare-rust-large.json`,
  `target/raw-compare-typescript-many-small.json`,
  `target/raw-compare-bun-rust-pr-32mb-3x.json`
- [x] Captured Mark's integrated 1M-row Bun PR workload:
  `target/mark-bench-bun-rust-pr.json`
- [x] Captured candidate-language raw Shiki comparisons on the Bun PR:
  `target/raw-compare-bun-zig-candidate.json`,
  `target/raw-compare-bun-c-cpp-candidate.json`,
  `target/raw-compare-bun-config-candidate.json`,
  `target/raw-compare-bun-doc-web-script-candidate.json`
- [x] Captured C-like fast-path additions:
  `target/raw-compare-bun-zig-fast.json`,
  `target/raw-compare-bun-c-like-fast.json`

## Release benchmark summary

`mark-bench` Rust syntax fixtures, release profile:

| Scenario | Legacy settle µs | Shiki tokenize µs | Optimized settle µs |
| --- | ---: | ---: | ---: |
| syntax-many-small-rust | 46,765 | 635,355 | 6 |
| syntax-large-rust | 229,039 | 1,252,660 | 0 |
| syntax-minified-rust | 0 | 204,699 | 0 |

Mark integrated syntax work, measured as cold scroll + settle + warm scroll:

| Scenario | Prior TextMate µs | Optimized µs | Prior RSS delta | Optimized RSS delta |
| --- | ---: | ---: | ---: | ---: |
| syntax-many-small-rust | 54,061 | 36,390 | 11.5MB | 9.0MB |
| syntax-large-rust | 46,937 | 45,148 | 53.6MB | 24.3MB |
| syntax-minified-rust | 3 | 2 | 0 | 16KB |

TypeScript fast-path validation against the syntect/TextMate fallback:

| Scenario | Syntect fallback µs | Fast path µs | Syntect settle µs | Fast settle µs |
| --- | ---: | ---: | ---: | ---: |
| many-small-files | 304,846 | 36,791 | 275,187 | 7 |
| balanced-changeset | 353,610 | 43,380 | 317,685 | 0 |

Binary size (`target/release/mark`): legacy 26M, TextMate migration 5.8M after
broad language expansion.

Shiki comparison is stored in `target/shiki-syntax-bench.json` for the same
fixture files. The Rust TextMate fast path keeps Mark's visible syntax work below
the measured Shiki full-file tokenization times.

### Apples-to-apples Shiki comparison

`mark-bench syntax-compare` compares cached Shiki `codeToTokens` against
`mark-textmate` on the same full source files. File reads, npm install, and
highlighter construction are outside the highlight timer; Shiki setup is reported
separately in the JSON artifacts.

| Input | Files | Bytes | Mark highlight µs | Shiki highlight µs | Mark speedup |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust many-small fixture | 240 | 937,008 | 4,572 | 614,923 | 134.5x |
| Rust large fixture | 1 | 1,897,788 | 11,019 | 1,227,393 | 111.4x |
| TypeScript many-small fixture | 240 | 1,144,368 | 4,931 | 797,148 | 161.7x |
| `~/sandbox/bun-rust-pr` sample, 3 iterations | 512 | 13,742,698 | 156,317 | 20,957,174 | 134.1x |

The Bun sample used changed Rust/TypeScript/JavaScript files from the 1M-row PR,
capped at 512 files and 32MB. The selected input was 13.7MB across 336,966 lines.
The Mark-only 20-iteration profile over the same sample measured 263MB/s and
`/usr/bin/time -l` reported 95MB max RSS for the whole benchmark process.

Integrated Mark TUI benchmark on `~/sandbox/bun-rust-pr`:

| Mode | Patch bytes | Rows | Files | Load µs | Open µs | Cold µs | Settle µs | Warm µs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| plain | 43,306,246 | 1,023,220 | 2,188 | 926,096 | 151,376 | 2,946 | - | 2,640 |
| syntax | 43,306,246 | 1,023,220 | 2,188 | 817,941 | 149,305 | 5,952 | 0 | 5,383 |

### Benchmark interpretation

- These are real local release-profile measurements captured into `target/` JSON
  artifacts, not synthetic numbers in this file.
- They are one-shot harness runs, not statistical criterion-style distributions.
- `mark-bench` measures Mark's interactive diff workload: build the TUI model,
  render a cold scroll pass, wait for queued syntax work to become idle, then
  render a warm scroll pass.
- `TextMate settle µs` is the remaining async syntax time after the cold scroll
  pass. `0` means there was no remaining queued work by the time settle was
  measured, not that parsing an entire file literally took zero time.
- The minified Rust fixture exceeds the default per-line highlight limit, so both
  backends skip it; this guards the UI from pathological single-line files.
- The Shiki column is full-file Shiki tokenization of the same fixture sources,
  not an integrated Mark TUI run, so it is a useful external reference but not an
  exact apples-to-apples UI measurement.
- The optimized results use range-only syntax segments plus fast paths for Rust,
  TypeScript/JavaScript/TSX, Zig, and C-like development languages. syntect/TextMate
  remains the fallback for other bundled grammars.
- Raw Shiki comparisons are now apples-to-apples at the highlighter level, but
  Mark still intentionally emits coarse syntax classes while Shiki emits richer
  VS Code-style token metadata.

### Optimization experiments

- [x] Kept: range-only syntax segments with per-line fingerprints. This removes
  duplicated source text from syntax cache entries while preserving stale-text
  fallback behavior.
- [x] Kept: coarse fast scanners for the highest-value common languages. The
  TypeScript benchmark improved by ~7-8x for integrated Mark syntax work.
- [x] Scrapped: extra unbenchmarked fast scanners. Go/JSON-style scanners were
  removed because they added duplicate language logic without local empirical
  workload data proving the complexity was worthwhile.
- [x] Scrapped: syntax worker pool. It was measured in
  `target/fast-typescript-workerpool-release.json` and
  `target/optimized-rust-syntax-release.json`; results were small/mixed compared
  with the extra concurrency and per-worker highlighter complexity, so the runtime
  remains single-worker with visible-job priority.
- [x] Scrapped: hunk-first normal diff highlighting. It was measured in
  `target/hunk-first-rust-syntax-release.json` and
  `target/hunk-first-typescript-syntax-release.json`; the measured fixture source
  bytes were unchanged for the patch workload and totals regressed/noised upward,
  so the existing full-file-when-available behavior was restored.

### Future optimization research notes

- Line-state checkpoints are the next plausible fallback optimization for
  syntect/TextMate-only languages: cache parser state periodically and resume from
  the nearest checkpoint instead of replaying from line zero.
- Viewport-only raw comparison would be a useful next benchmark mode, but the TUI
  benchmark already validates viewport-first behavior on real diffs.
- Per-language fast paths should require a local corpus and raw Shiki comparison
  artifact before being added. Unbenchmarked scanners are treated as redundancy.
- Slow-rule profiling for syntect fallback grammars remains future work; current
  hot measured workloads are dominated by fast-path Rust/TS/JS where Mark is over
  100x faster than cached Shiki tokenization.

### Fast-path language backlog

Candidate languages are prioritized by observed workload size, whether Mark is
currently slower than cached Shiki, implementation complexity, and general diff
review value. A fast path should only be kept after a before/after artifact proves
the complexity is worthwhile.

| Priority | Language(s) | Bun PR sample | Current raw comparison | Decision |
| --- | --- | ---: | ---: | --- |
| P0 | Zig | 308 files / 8.1MB | Mark 3,041,943µs vs Shiki 2,538,689µs | Add first; biggest unoptimized corpus and Mark loses today |
| P1 | C/C++/headers | 23 files / 704KB | Mark 386,411µs vs Shiki 1,264,467µs | Add after Zig only if a simple C-like scanner gives a large win |
| P1 | TOML + JSON | 135 files / 180KB | Mark 28,805µs vs Shiki 50,269µs | Simple scanner candidate; keep only if it materially lowers TUI/cache cost |
| P2 | Markdown | 6 files / 57KB | included in doc/web/script group where Mark loses overall | Consider a lightweight headings/code-span/list scanner |
| P2 | Bash | 5 files / 16KB | included in doc/web/script group where Mark loses overall | Useful but grammar is tricky; benchmark before keeping |
| P2 | CSS + HTML | 17 files / 49KB | included in doc/web/script group where Mark loses overall | Web diffs common; scanner can be coarse |
| P3 | YAML | 1 file / 4KB | included in doc/web/script group where Mark loses overall | Low Bun value; only add with broader corpus evidence |

Fast paths now added: Rust; TypeScript/JavaScript/TSX/JSX; Zig; C/C++/Objective-C;
C#; Java/Kotlin/Scala; Go; Swift; Dart; PHP; Solidity/Vyper. Zig moved from
slower-than-Shiki to substantially faster on the Bun PR sample:

| Language group | Before Mark µs | After Mark µs | Shiki µs | After speedup vs Shiki |
| --- | ---: | ---: | ---: | ---: |
| Zig | 3,041,943 | 38,786 | 2,588,244 | 66.7x |
| C/C++ | 386,411 | 3,166 | 1,285,498 | 406.0x |

Default enabled language coverage was expanded beyond the original small core set
to broad development-language coverage, including LaTeX/TeX/BibTeX, Nix,
Dockerfile, Terraform/HCL, Vue/Svelte, JVM/.NET languages, scripting languages,
compiler-engineering languages, data/config formats, query/schema languages,
academic languages, data-science languages, and docs/science/contract languages.
Shiki-compatible language aliases were also added for common alternate IDs such
as `actionscript-3`, `asciidoc`, `bat`, `coffee`, `common-lisp`, `csv`,
`fortran-free-form`, `fsharp`, `gdscript`, `git-rebase`, `hcl`, `jsonc`, `json5`,
`mdx`, `objective-cpp`, `proto`, `rst`, `shellscript`, `system-verilog`, and
`vue`.

Additional requested breadth added:

- Compiler/low-level: MLIR, LLVM IR, TableGen, WebAssembly text, HLSL, GLSL,
  WGSL, Metal/OpenCL/CUDA shader-like sources, assembly families, MIPS assembly,
  Verilog/SystemVerilog/VHDL, CMake, Make, Ninja, GN, Meson, Starlark/Bazel.
- Lisp family: Common Lisp, Emacs Lisp, Scheme, Racket, Clojure, Fennel, Hy.
- Academic/PL: Haskell, OCaml family, Elixir/Erlang, Idris, Lean, Coq, Agda,
  Prolog, F#, SML, Crystal, Nim, Smalltalk, Raku, Pony, MoonBit, Mojo, Move-like
  C-family experimental languages.
- Data science/analytics: Python, R, Julia, MATLAB, SAS, Stata, Wolfram,
  DAX, PowerQuery, SQL, notebooks via JSON/IPYNB mapping.
- App/docs/templates/infra: Angular, Astro, MDX/MDC, Marko, QML, Pug, Razor,
  Handlebars, Liquid, Twig, EJS/ERB, Mermaid, Wikitext, CODEOWNERS, Bicep, Cue,
  Dhall, Pkl, Prisma, systemd units, Nginx, Justfile, Nushell, Kusto, Cypher,
  SPARQL, SurrealQL.
- Shiki parity additions: ABAP, Apex, Ara, Berry, BIRD, BSL/1C, Clarity, COBOL,
  Dream Maker, Fluent, GDScript resource/shader formats, Genie, Gherkin, Gleam,
  Glimmer JS/TS, Haxe, Hurl/HXML, Imba, Jison, JSSM, KDL, Logo, Luau, Narrat,
  Nextflow, OpenSCAD, PO, Polar, RISC-V assembly, RON, ROS messages, SDBL,
  ShaderLab, Soy, Splunk, TalonScript, TASL, TS tags, Turtle, Wenyan, WIT, XSL,
  and ZenScript.

Implementation approach: if a grammar is bundled, the language is enabled through
the TextMate catalog. If a requested language is not bundled but has high diff
review value, it is registered as a fast-only language with a coarse scanner
(`CompilerIr`/Lisp-like/C-like) so Mark can still highlight it without pulling a
large grammar/runtime dependency.

Current release binary after this breadth expansion: `target/release/mark` is
5.8M and `target/release/mark-bench` is 4.4M.

Default syntax mode is now `builtin`, so all bundled/fast-only languages are
eligible out of the box. `mark syntax list` currently reports 346/346 languages
ready with no disabled entries. A Shiki 4.3.0 language audit reports 235/235
Shiki language IDs supported through canonical names or aliases. Users can still
choose `mode = "enabled"` for an explicit allow-list. A `mark-textmate` smoke
test now highlights every available language to catch catalog entries whose
canonical name does not resolve to a working grammar/fast path.

Full dev-language candidate list to evaluate with `syntax-compare` before adding
any fast path:

| Tier | Languages |
| --- | --- |
| Systems/native | Zig, C, C++, Objective-C, Swift, Go, Rust, Fortran, Assembly |
| Web/app | TypeScript, JavaScript, TSX/JSX, Vue, Svelte, HTML, CSS, PHP, Dart |
| JVM/.NET | Java, Kotlin, Scala, C# |
| Scripting | Python, Ruby, Bash, Fish, PowerShell, Perl, Lua, R |
| Functional/BEAM | Haskell, OCaml, Elm, Erlang, Elixir, Clojure, Lisp/Scheme/Racket |
| Data/config/infra | JSON, YAML, TOML, Nix, Dockerfile, Terraform/HCL, CMake, Make, INI, dotenv |
| Query/API/schema | SQL, GraphQL, Protocol Buffers, JSONNet, Rego |
| Docs/science/contracts | Markdown, reStructuredText, AsciiDoc, LaTeX/TeX/BibTeX, Solidity, Vyper, MATLAB, Julia |

Requested additions now explicitly in backlog: LaTeX/TeX/BibTeX, Nix,
Dockerfile, and broad development-language coverage. `mark-bench syntax-compare`
has Shiki language mappings for the common candidates so each can be measured on
real corpora before implementation.

Benchmark command template:

```sh
target/release/mark-bench syntax-compare \
  --repo ~/sandbox/bun-rust-pr \
  --language <language> \
  --max-files 512 \
  --max-bytes 32000000 \
  --shiki \
  --json > target/raw-compare-bun-<language>-candidate.json
```

## Implementation

- [x] Add extraction-ready `crates/mark-textmate`
- [x] Add `mark-bench syntax-compare` for raw Mark-vs-Shiki comparison
- [x] Replace `mark-syntax` highlighting internals with `mark-textmate`
- [x] Remove native parser download/cache/trust/validation code
- [x] Remove native parser CLI flags and docs
- [x] Remove native parser dependencies from `Cargo.toml`/`Cargo.lock`
- [x] Update tests for TextMate semantics
- [x] Optimize syntax cache representation to ranges plus fingerprints
- [x] Add validated fast paths for common coarse highlighting workloads
- [x] Expand default enabled language coverage to broad development languages
- [x] Add validated C-like fast paths, including Zig
- [x] Test and reject worker-pool optimization due marginal/mixed value

## Verification

- [x] `cargo fmt`
- [x] `cargo check`
- [x] `cargo test --workspace --quiet`
- [x] `cargo build --release -p mark-cli`
- [x] `cargo run --release -p mark-bench -- measure ...`
- [x] Verify no native parser dependency remains in `cargo tree -p mark-cli`
- [x] Verify no removed backend terminology remains in Cargo/docs/crates/README
