# In-house TextMate engine

Status document for the native TextMate syntax engine in `crates/mark-syntax`.
This is the migration from the removed syntect/two-face hybrid to a single
in-house engine with vendored TextMate grammars.

Production highlighting is switched to the bundled native backend. The engine,
grammar bundle path, oracle harness, and full public catalog are in-tree. The
checked-in fixture corpus passes exact scope-stack and coarse class parity
without divergence allowlists.

## Goals

- One syntax engine, one grammar catalog, one source of truth for detection.
- Correctness measured against a pinned `vscode-textmate` + `vscode-oniguruma`
  oracle (dev-only Node tooling), not against the old hybrid.
- Coverage is an **asset** problem: add or drop real grammars; never map a
  language onto another language's hand-written lexer.
- Release builds stay offline and **Node-free**. Node is only for regenerating
  goldens / regex conformance during development.

## Architecture

```text
crates/mark-syntax/src/
  engine/
    grammar.rs      # rule model (match / begin-end / begin-while, includes, injections)
    regex/          # hybrid matcher: DFA path + budgeted backtracking fallback
    tokenizer.rs    # line-oriented TextMate stack machine
    state.rs        # interned parser state / checkpoints
    scopes.rs       # scope interning + scope → SyntaxClass
    line.rs         # line splitting / UTF-8 boundary helpers
  grammars/
    registry.rs     # curated dev/test asset table (raw JSON via include_str!)
    bundle.rs       # MRKB embedded bundle reader
    catalog.rs      # aliases / extensions / basenames for the bundle
  highlight.rs, language.rs, storage.rs, types.rs  # public config / API surface

assets/tm-grammars/           # committed TextMate JSON (full public catalog + private deps)
  SOURCE.toml                 # pin: @shikijs/langs@3.23.0
  licenses.json               # per-grammar license manifest
  coverage.toml               # active public/private grammar coverage manifest

tools/                        # dev-only Node oracle (not linked into the binary)
  golden-oracle/              # pinned vscode-textmate@9.2.0, vscode-oniguruma@1.7.0
  golden-dump.mjs             # single-file oracle dumper
  generate-goldens.mjs        # regenerate all cases from cases.toml
  regex-conformance.mjs       # small Oniguruma vs mark-syntax pattern checks
  grammar-stats.mjs           # pattern feature inventory over vendored JSON

crates/mark-syntax/tests/
  textmate_golden.rs          # exact + coarse parity harness + property tests
  fixtures/textmate/          # sources, goldens, cases.toml, divergences.toml
```

### Matching strategy (hybrid, Rust-native)

1. Parse and classify grammar patterns into Mark's ordered regular scanner when
   possible, with literal/start-byte/required-literal filtering.
2. Route lookaround / backreferences / `\G`-heavy patterns to an in-house
   backtracker with a hard step budget.
3. Scan all candidates together by input position and grammar order, then
   replay only the winner when captures are required.
4. Keep the VM's common zero/one-state result inline, allocate only for real
   backtracking fanout, and reuse an unchanged candidate set within a line.
5. On budget kill: degrade the affected range, keep the line moving (no hang).

There is no production regex crate or Oniguruma dependency. Hot rules in
TypeScript/C++/Ruby still exercise the fallback path, so fallback cost is a
first-class concern, not a rare escape hatch.

### Public API shape

External `mark-syntax` types stay shape-compatible with the previous highlighter
so TUI queue/LRU layers do not need a redesign:

- `SyntaxClass`, `SyntaxSegment`, `HighlightedLine`, `HighlightedText`
- language detection / config management (`language.rs`, `storage.rs`)
- `SyntaxHighlighter` must remain `Send` (worker-thread highlighting)

## Public language catalog

The native catalog is the full pinned Shiki language set plus the MLIR grammar
imported from LLVM, vendored under `assets/tm-grammars/languages/`. The active
public ids are listed in `assets/tm-grammars/coverage.toml`; private dependency
blobs such as `yang` and `twig-source` are embedded but hidden from user-facing
language selection.

The original core regression set remains covered by fixtures and includes:

| Language id | Grammar asset | Root scope |
| --- | --- | --- |
| `bash` | `shellscript.tmLanguage.json` | `source.shell` |
| `c` | `c.tmLanguage.json` | `source.c` |
| `cpp` | `cpp.tmLanguage.json` | `source.cpp` |
| `csharp` | `csharp.tmLanguage.json` | `source.cs` |
| `css` | `css.tmLanguage.json` | `source.css` |
| `docker` | `docker.tmLanguage.json` | `source.dockerfile` |
| `go` | `go.tmLanguage.json` | `source.go` |
| `html` | `html.tmLanguage.json` | `text.html.basic` |
| `java` | `java.tmLanguage.json` | `source.java` |
| `javascript` | `javascript.tmLanguage.json` | `source.js` |
| `json` | `json.tmLanguage.json` | `source.json` |
| `jsx` | `jsx.tmLanguage.json` | `source.js.jsx` |
| `kotlin` | `kotlin.tmLanguage.json` | `source.kotlin` |
| `lua` | `lua.tmLanguage.json` | `source.lua` |
| `make` | `make.tmLanguage.json` | `source.makefile` |
| `markdown` | `markdown.tmLanguage.json` | `text.html.markdown` |
| `nix` | `nix.tmLanguage.json` | `source.nix` |
| `php` | `php.tmLanguage.json` | `source.php` |
| `powershell` | `powershell.tmLanguage.json` | `source.powershell` |
| `python` | `python.tmLanguage.json` | `source.python` |
| `ruby` | `ruby.tmLanguage.json` | `source.ruby` |
| `rust` | `rust.tmLanguage.json` | `source.rust` |
| `scss` | `scss.tmLanguage.json` | `source.css.scss` |
| `sql` | `sql.tmLanguage.json` | `source.sql` |
| `swift` | `swift.tmLanguage.json` | `source.swift` |
| `terraform` | `terraform.tmLanguage.json` | `source.hcl.terraform` |
| `toml` | `toml.tmLanguage.json` | `source.toml` |
| `tsx` | `tsx.tmLanguage.json` | `source.tsx` |
| `typescript` | `typescript.tmLanguage.json` | `source.ts` |
| `yaml` | `yaml.tmLanguage.json` | `source.yaml` |

Grammar pin: `@shikijs/langs@3.23.0` (see `assets/tm-grammars/SOURCE.toml`).

The first-class extended fixtures include `zig`, `llvm`, `riscv`, `mipsasm`,
`odin`, `asm`, `mojo`, `ocaml`, and `mlir`; adding more coverage is an asset and
fixture decision, not an engine fork.

### Adding or updating a grammar (checklist)

1. Vendor the compact JSON from the pinned package via
   `tools/vendor-shiki-grammars.mjs` (or `[[additional_sources]]` in
   `SOURCE.toml` for non-Shiki grammars like `mlir`), stable key order.
2. `licenses.json` entry from the package's per-grammar license metadata.
3. `coverage.toml` public entry; aliases/extensions merged into `catalog.rs`
   with ids matching Shiki's `bundledLanguagesInfo`.
4. `tools/grammar-stats.mjs` inventory first — any regex construct not in
   `tools/regex-conformance.mjs`'s proving set gets a conformance case
   **before** the grammar lands.
5. Fixtures + oracle goldens (`stoppedEarly: false`), exact + coarse parity,
   `divergences.toml` stays empty, zero degraded lines.
6. Perf: process-cold stress ≥ 2 MB/s floor per language; a floor breach
   gets a counters audit + tracked issue, never a silent merge. Add a sweep
   corpus member so the aggregate tracks the language forever.

## Dependency policy

| Layer | Allowed | Forbidden in release |
| --- | --- | --- |
| Product binary (`mark-syntax` / CLI) | Pure Rust; vendored grammar JSON; build-time `bundle.bin` | Node, npm, network at build/runtime for highlighting |
| Dev oracle (`tools/golden-oracle`) | Pinned `vscode-textmate@9.2.0`, `vscode-oniguruma@1.7.0` | Version ranges (`^`/`~`); resolving packages from unrelated trees |
| Tests | Checked-in `.golden.jsonl`, optional Node for regen | Requiring Node in default `cargo test` |

Install oracle deps only when regenerating goldens:

```sh
npm install --prefix tools/golden-oracle
```

The package is `"private": true` and lives outside the Rust workspace.

## Phases 0–5

### Phase 0 — baseline, inventory, guardrails

**Goal.** Freeze measurable behavior and know what the engine must replace.

**Deliverables.**

- Grammar source pin (`SOURCE.toml`) and license manifest.
- Pattern-feature inventory (`tools/grammar-stats.mjs` over vendored JSON).
- Coverage keep/drop notes (`coverage.toml`; active full catalog).
- Golden-token oracle tools and fixture corpus.
- A divergence file that must stay empty while committed fixtures are exact.

**Current state.** Full-catalog assets are vendored. Oracle tools and a
full-language smoke/stress corpus exist. Production still does not depend on
Node.

### Phase 1 — grammar model and dev loader

**Goal.** Represent enough TextMate semantics to run the tokenizer on raw JSON.

**Deliverables.**

- Rule model: `match`, `begin`/`end`, `begin`/`while`, includes (`$self`,
  `$base`, `#repo`, external scopes), captures, `name`/`contentName`,
  injections / selectors.
- Dev loader for raw `tmLanguage.json` (used by tests and examples).

**Current state.** In-tree under `engine/grammar.rs` and related modules; core
fixture grammars deserialize. Full injection fidelity is still a parity work
item (see limitations).

### Phase 2 — regex translation and matching

**Goal.** Correct hybrid matching with a budgeted fallback.

**Deliverables.**

- Regex AST + feature classification (DFA vs fallback).
- DFA/PikeVM path for supported patterns; fallback VM for lookaround/backrefs.
- Anchor-context handling (`^` / `\A` / `\G` depend on resume position).
- Prefilters (required literals).
- Conformance helper: `tools/regex-conformance.mjs`.

**Current state.** Native regular and fallback matchers, ordered pattern-set
selection, anchor context, prefilters, and budget kills are implemented. The
conformance script covers a proving set; broader Oniguruma parity remains open.

### Phase 3 — tokenizer and scope classification

**Goal.** Line tokenization that can pass golden fixtures.

**Deliverables.**

- Stack machine with first-match-wins, while-continuation, captures, zero-width
  guards.
- Cross-grammar includes and injection candidate flattening.
- Scope → `SyntaxClass` classification and `HighlightedText` conversion.
- Exact + coarse golden harness (`textmate_golden.rs`).

**Current state.** Tokenizer + harness land; all basic, stress, and smoke cases
are exact gates. `divergences.toml` contains no exceptions.

### Phase 4 — grammar compiler and embedded bundle

**Goal.** Deterministic embedded `bundle.bin` with lazy per-language decode.

**Deliverables.**

- `grammar-compile` / `build.rs` pipeline → `MRKB` bundle.
- Registry APIs: available / canonical / detect-from-path.
- License table and bundle version stamp.
- No network/Node in release builds.

**Current state.** Bundle format and compile path are in production. The runtime
decodes only the selected grammar and its embedding dependencies; compact JSON
is stored once in the bundle rather than duplicated through the dev asset table.

### Phase 5 — performance layer

**Goal.** Meet interactive latency without reintroducing hand lexers.

**Deliverables.**

- Interned `StateId`, line cache, checkpoints, candidate/prefilter caches.
- Per-line degradation budgets.
- Instrumentation counters and before/after measurements.

**Current state.** State interning, bounded line/candidate caches, checkpoints,
budgets, counters, and lazy per-language tokenizer instances are implemented.
The existing TUI worker, file/hunk cache, and render integration now use the
native backend.

Current throughput numbers, hotspot diagnoses, and the remaining optimization
backlog live in [`performance-plan.md`](performance-plan.md) — the interim
measurement history that used to live here is superseded by that document's
§1 tables.

**Retained optimizations** (each validated with alternating-order,
separate-process A/B runs, paired medians, and byte-exact scope streams):
candidate-index results with deferred ownership, per-line unchanged-state
candidate reuse, pre-sized VM fanout, inline zero/one VM states,
parse-time-resolved subroutine target paths (~41% on libc++), anchored winner
capture replay, capture-observability gating (position-only selection VM for
lookahead/lookbehind with winner-only capture replay), the ordered
alternation/repetition bytecode path with the deterministic C-family
comment-or-space separator instruction, compact backreference capture
layouts, reduced token/capture/stack cloning, and a 512 steps-per-byte
source-wide fallback allowance (128 was too low for valid complex grammars).
`profile-cold` configures production syntax limits so its line-cache behavior
matches the runtime.

**Reverted experiments — do not retry as-is** (measured deltas in the git
history of this file):

- Per-candidate next-match memoization: independent per-pattern searches were
  2.14× slower than the unified ordered grammar-order scan; any future memo
  must preserve the unified scan's laziness.
- Linear-only bytecode slice (literal/class/dot/anchor/group): neutral-to-
  slower; bytecode must cover ordered alternation/repetition where recursive
  fanout dominates.
- Position-only recursive subroutines: ~30% faster on libc++ but increased
  mismatched C++ lines 1,488 → 1,804; subroutine calls stay capture-aware.
- Routing atomic/possessive patterns through the position VM: 0.8–1.7%
  regressions.
- 10× per-match/per-line budgets: 1.4× slower for only ~5% fewer mismatches —
  quality needs cheaper execution, not bigger budgets.
- Also neutral/slower: leading-word-boundary gates, mandatory-prefix checks,
  compact line-cache keys, possessive-repeat specialization, iterator-based
  lookbehind positions, ASCII class branches, source line pre-counting,
  routing capture-free regexes through the position VM.

### After phase 5 (not detailed here)

Phase 6+ covers broadening conformance beyond the proving corpus and raising
scanner throughput. The unavailable-backend shim has already been removed from
the production path.
The concrete performance sequence and acceptance gates are in
[`performance-plan.md`](performance-plan.md).

## Reproducible oracle commands

```sh
# 1. Install pinned Node oracle (once per machine / lockfile change)
npm install --prefix tools/golden-oracle

# 2. Regenerate every case in the manifest
node tools/generate-goldens.mjs

# 3. Check committed goldens without rewriting
node tools/generate-goldens.mjs --check

# 4. One language id
node tools/generate-goldens.mjs --case typescript
node tools/generate-goldens.mjs --case java

# 5. Ad-hoc dump
node tools/golden-dump.mjs \
  --language rust \
  --scope source.rust \
  --grammar assets/tm-grammars/languages/rust.tmLanguage.json \
  --file crates/mark-syntax/tests/fixtures/textmate/rust/basic.rs \
  --out crates/mark-syntax/tests/fixtures/textmate/rust/basic.golden.jsonl

# 6. Pattern feature inventory over vendored grammars
node tools/grammar-stats.mjs assets/tm-grammars/languages

# 7. Small regex conformance report (needs cargo + oracle install)
node tools/regex-conformance.mjs
# → target/regex-conformance-phase2.json

# 8. Engine golden harness (when the crate builds)
cargo test -p mark-syntax --test textmate_golden --locked
```

Oracle record shape (one JSON object per source line):

- `language`, `scopeName`, `file`, `lineNumber`, `line`
- `tokens[]`: UTF-16 `startIndex` / `endIndex` + full `scopes` stack
- `ruleStack` (debug string), `ruleStackHash` (sha256 of that string)
- `stoppedEarly` (must be false for committed goldens; dumper uses time limit 0)

## Fixture corpus policy

| Kind | Languages | Engine comparison |
| --- | --- | --- |
| `basic` | json, rust, yaml, python | Exact + coarse |
| `stress` | bash, c, cpp, css, go, html, javascript, json, markdown, python, rust, toml, tsx, typescript, yaml | Exact + coarse |
| `smoke` | csharp, docker, java, jsx, kotlin, lua, make, nix, php, powershell, ruby, scss, sql, swift, terraform | Exact + coarse |

Embedded grammars in the manifest (non-exhaustive): markdown→rust/js,
html→js/css, scss→css, php→html/js/css/sql, cpp→cpp-macro.

Do not hand-edit `*.golden.jsonl`. Update sources, then regenerate.

## Known current limitations (honest)

### VS Code screenshots are not a TextMate-only oracle

VS Code normally overlays language-server semantic tokens on TextMate tokens.
For Rust, rust-analyzer can therefore color parameters, local declarations,
fields, and inferred types differently even when the underlying TextMate scope
stream is identical. Inline type hints in a screenshot are another indication
that semantic analysis is active. Mark currently promises parity with pinned
`vscode-textmate`, not rust-analyzer semantic-token parity.

For example, the `execute_inner` function in
`engine/regex/bytecode.rs`—including its parameters and `let mut pc` /
`let mut position` declarations—was compared token-by-token and has the exact
same TextMate scopes in Mark and `vscode-textmate`. A visual difference there
comes from semantic-token overlays and/or theme scope-color mappings, not a
native tokenizer divergence. Visual investigations must first run VS Code with
semantic highlighting disabled, then compare the full scope stacks before
changing tokenizer behavior.

1. **Corpus parity is not universal proof.** The checked-in fixtures are 100%
   exact, but more adversarial and real-world fixtures are still needed.
2. **Broader Oniguruma conformance.** Recursive subroutines, nested classes,
   dynamic ends, lookaround captures, anchors, and backreferences are covered
   by the proving set; the full Oniguruma surface is larger.
3. **Hot fallback path.** Lookaround-heavy rules (especially TS/C++/Ruby) still
   rely on the budgeted backtracker; translator widening may be required if
   fallback dominates hot matches.
4. **Performance target remains open.** The measured 2.1x scanner improvement
   is substantial, but cold full-file throughput is still well below 12 MB/s.
5. **Catalog breadth.** The product catalog now follows the full pinned Shiki
   import plus MLIR. Full Shiki parity still relies on smoke/stress fixtures and
   the budget guard; adversarial coverage remains incremental.
6. **Oracle is Node-only.** Correctness regen requires Node ≥ 20 and the pinned
   packages under `tools/golden-oracle`. CI that only runs `cargo test` does not
   re-derive goldens unless a separate job runs `generate-goldens.mjs --check`.
7. **UTF-16 vs UTF-8.** Oracle offsets are UTF-16 (JS); the harness converts to
   UTF-8 byte ranges. Non-ASCII fixtures exist to catch boundary mistakes; any
   new fixture with astral-plane characters should stay in the corpus.
8. **No silent oracle truncation.** `golden-dump.mjs` passes time limit `0` so
    long minified lines are not stopped early by vscode-textmate's wall clock.
    Committed goldens must keep `stoppedEarly: false`.

## Related paths

- Fixtures: `crates/mark-syntax/tests/fixtures/textmate/`
- Harness: `crates/mark-syntax/tests/textmate_golden.rs`
- Oracle: `tools/golden-dump.mjs`, `tools/generate-goldens.mjs`, `tools/golden-oracle/`
- Assets: `assets/tm-grammars/`
- Engine: `crates/mark-syntax/src/engine/`
- Performance continuation plan: `docs/performance-plan.md`
