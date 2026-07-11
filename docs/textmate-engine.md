# In-house TextMate engine

Status document for the native TextMate syntax engine in `crates/mark-syntax`.
This is the migration from the removed syntect/two-face hybrid to a single
in-house engine with vendored TextMate grammars.

Production highlighting is switched to the bundled native backend. The engine,
grammar bundle path, oracle harness, and core-30 catalog are in-tree. The full
checked-in core-30 corpus now passes exact scope-stack and coarse class parity
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
    registry.rs     # core-30 dev/test asset table (raw JSON via include_str!)
    bundle.rs       # MRKB embedded bundle reader
    catalog.rs      # aliases / extensions / basenames for the bundle
  highlight.rs, language.rs, storage.rs, types.rs  # public config / API surface

assets/tm-grammars/           # committed TextMate JSON (core-30 + cpp-macro)
  SOURCE.toml                 # pin: @shikijs/langs@3.23.0
  licenses.json               # per-grammar license manifest
  coverage.toml               # historical keep/drop notes from the full catalog era

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

## Core-30 languages

The initial native catalog is the **core-30** set vendored under
`assets/tm-grammars/languages/`. Support grammar `cpp-macro` is included for
C++ preprocessor embeddings and is not a user-facing language id.

| Language id | Grammar asset | Root scope |
| --- | --- | --- |
| `bash` | `shellscript.tmLanguage.json` | `source.shell` |
| `c` | `c.tmLanguage.json` | `source.c` |
| `cpp` | `cpp.tmLanguage.json` | `source.cpp` |
| `csharp` | `csharp.tmLanguage.json` | `source.cs` |
| `css` | `css.tmLanguage.json` | `source.css` |
| `dockerfile` | `docker.tmLanguage.json` | `source.dockerfile` |
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

Earlier recovery work also carried a `zig` stress fixture; zig is **not** in
core-30 and was removed from the golden corpus. Expanding beyond core-30 is an
asset decision (vendor a grammar + add fixtures), not an engine fork.

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
- Coverage keep/drop notes (`coverage.toml`; historical full-catalog work).
- Golden-token oracle tools and fixture corpus.
- A divergence file that must stay empty while the core-30 corpus is exact.

**Current state.** Core-30 assets are vendored. Oracle tools and a full-language
smoke/stress corpus exist. Production still does not depend on Node.

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

Measured on the generated 1,897,788-byte / 32,000-line Rust fixture (release,
one cold iteration, all 32,001 tokenizer lines, no degradation), candidate
search improvements reduced Mark from about 3.21 s / 0.59 MB/s to about
0.91–0.93 s / 2.04–2.09 MB/s. This is a roughly 3.5x throughput improvement
and is ahead of both measured JavaScript implementations on this corpus:
pinned Shiki took about 1.55 s / 1.22 MB/s, while the direct pinned
`vscode-textmate` oracle took
about 1.46–1.48 s / 1.28–1.30 MB/s. A separate 3.24 MB repeated checked-in
fixture set spanning 29 detected languages improved by about 5% over the
pre-optimization binary, guarding against
retaining a Rust-only micro-optimization. It does **not** yet meet the original
12 MB/s acceptance target or historical Tree-sitter throughput; scanner/VM
optimization remains open.

The latest changes were evaluated as alternating-order, separate-process A/B
runs. Retained changes had repeatable paired-median wins: inline zero/one VM
states (~5.3%), candidate-index results and deferred ownership (~4.5%
combined), per-line unchanged-state candidate reuse (~10.7%), pre-sized VM
fanout (~5% across the tested capacity steps), direct bounded-lookbehind
positions (~1.4%), and reduced token/capture/stack cloning and allocation
(several additional 0.5–2% steps). Avoiding winner replay when a rule cannot
observe captures yielded another ~8.2% on the large Rust corpus and ~1% on the
repeated multi-language set. Experiments that were neutral or slower
were reverted, including leading-word-boundary gates, mandatory-prefix checks,
compact line-cache keys, possessive-repeat
specialization, iterator-based lookbehind positions, ASCII class branches,
and source line pre-counting.

The next cold-pass profiling round added representative TypeScript, Markdown,
and libc++ C++ corpora. Regex subroutine calls now carry a parse-time-resolved
path to their target capture instead of recursively searching the complete AST
for every call. On the 184,514-byte libc++ `<string>` corpus this reduced a
single cold pass from about 5.6 seconds to 3.34 seconds (roughly 41%), while the
large Rust and TypeScript runs remained within about 1% of their prior medians.
Winning capture replay is also anchored at the already-known match start rather
than repeating an unanchored search; paired medians improved about 1.3% on the
TypeScript corpus and 1.1% on Markdown and were neutral to slightly positive on
Rust. C++ remains pathologically slow despite the subroutine improvement, so
this is not a substitute for the planned iterative VM.

That C++ run also exposed that the source-wide fallback allowance of 128 VM
steps per source byte was too low for complex but valid grammars: the separate
diagnostic pass exhausted it and source-skipped 2,339 of 4,039 tokenizer lines.
The allowance is now 512 steps per byte. On the same corpus the diagnostic pass
source-skips zero lines (about 91.1 million fallback steps); per-match and
per-line hard limits still intentionally degrade 222 pathological lines. The
timed cold pass remained about 3.32 seconds, so removing the source-wide skips
did not create a measured throughput regression, but the remaining per-line
degradation is explicitly part of the unresolved C++ VM work.

Simply raising the per-match and per-line limits by 10x was also rejected. It
reduced libc++ oracle-mismatched lines only from 1,488 to 1,417 while slowing
the cold pass from about 3.33 seconds to 4.63 seconds. The remaining quality
gap needs more efficient execution, not merely a larger backtracking budget.

A semantics-safe per-candidate next-match memo was prototyped with invalidation
for tokenizer state and anchor-context changes. It was reverted: independently
searching every candidate up front was about 2.14x slower than the unified
ordered pattern-set scan on the large Rust corpus. Any future memoized scanner
must preserve the unified scan's grammar-order laziness rather than reintroduce
per-pattern whole-line searches.

A conservative deterministic bytecode slice for linear literal/class/dot/
anchor/group patterns also passed strict parity, but was reverted after cold
A/B runs: it was about 0.5% slower on Rust, 0.9% slower on TypeScript, neutral
on Markdown, and only about 0.7% faster on the already-pathological C++ case.
The useful bytecode cutover needs to cover ordered alternation and repetition,
where recursive state fanout dominates, rather than only the uncommon
single-path subset.

The first dedicated position-only selection VM was retained. For patterns
whose captures cannot affect matching (no backreference, subroutine, conditional,
unsupported construct, or atomic/possessive feature),
selection now propagates inline `usize` positions rather than full `VmState`
values with capture vectors. Positive/negative lookahead and bounded or
unbounded lookbehind run on this path and collapse assertion results back to
the asserted position. Full capture
extraction remains on the existing VM and is replayed only for the winner.
Strict goldens and focused ambiguous repeat/alternation/lookahead/UTF-8
differential tests pass. The regular subset improved the large Rust corpus by
about 3.5% and Markdown by 1.3%; enabling lookahead added about 2.4% and 1.4%
respectively. Position-only lookbehind then added about 1.1% on Rust and 6.1%
on TypeScript and was neutral to slightly positive on Markdown and C++.
The complete 4,039-line libc++ scope stream was also byte-for-byte identical
to the capture-aware VM output (not merely equal token counts), while both were
compared separately with the pinned `vscode-textmate` oracle.
The route is limited to regexes that actually declare captures; capture-free
regexes already carry an allocation-free empty vector, and routing those too
was slightly worse overall. This gate was neutral on the large Rust,
TypeScript, and Markdown corpora and turned the 3.24 MB repeated multi-language
corpus into a repeatable ~0.9% win over the pre-position-VM binary.
Position-only recursive subroutines were faster (about 30% on libc++), but were
reverted because full-corpus scope comparison against `vscode-textmate` showed
that they increased mismatched C++ lines from 1,488 to 1,804. Subroutine calls
therefore remain capture-aware. Compiling the safe position evaluator to
iterative bytecode remains follow-up work.
Routing atomic/possessive patterns through the position VM also preserved the
tested outputs but was reverted after regressions of about 0.8% on both Rust
and TypeScript and 1.7% on C++.

The stripped release `mark` binary is 6,728,128 bytes on the measured macOS
build, inside the 8 MB core-30 target.

### After phase 5 (not detailed here)

Phase 6+ covers broadening conformance beyond the proving corpus, raising
scanner throughput, and expanding the catalog beyond core-30. The
unavailable-backend shim has already been removed from the production path.
The concrete performance sequence and acceptance gates are in
[`textmate-performance-plan.md`](textmate-performance-plan.md).

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

1. **Corpus parity is not universal proof.** The checked-in core-30 fixtures are
   100% exact, but more adversarial and real-world fixtures are still needed.
2. **Broader Oniguruma conformance.** Recursive subroutines, nested classes,
   dynamic ends, lookaround captures, anchors, and backreferences are covered
   by the proving set; the full Oniguruma surface is larger.
3. **Hot fallback path.** Lookaround-heavy rules (especially TS/C++/Ruby) still
   rely on the budgeted backtracker; translator widening may be required if
   fallback dominates hot matches.
4. **Performance target remains open.** The measured 2.1x scanner improvement
   is substantial, but cold full-file throughput is still well below 12 MB/s.
5. **Core-30 only.** The product catalog for this migration is the 30 languages
   above. Broader Shiki/tm-grammars coverage is deferred; `coverage.toml` still
   records older full-catalog keep/drop notes and may disagree with the on-disk
   core-30 asset tree until production catalog code is realigned.
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
- Performance continuation plan: `docs/textmate-performance-plan.md`
