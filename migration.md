# In-house TextMate engine migration

This file tracks the hard migration from the syntect/two-face hybrid (grammar
fallback + hand-rolled fast-path lexers) to a single in-house TextMate engine
living entirely in `crates/mark-syntax`.

The migration is intentionally a cutover, not an optional backend: when it is
complete there is one syntax engine, one grammar catalog, and one source of
truth for language detection/highlighting.

## Why

The current system is structurally inconsistent, not merely buggy:

- ~130 `FAST_ONLY_LANGUAGES` are highlighted by coarse lexers built for a
  *different* language (Gleam/Ballerina/Cairo use the C lexer, Raku/Prolog/
  Mermaid use the "compiler IR" lexer, etc.). Keyword tables are unions across
  dozens of languages, so `vec3`, `orelse`, and `chan` highlight as keywords in
  every C-like file.
- syntect-backed languages get real scope-driven highlighting while fast-path
  languages get approximations, so quality varies per language and per
  construct within the same diff.
- The binary carries three overlapping engines: oniguruma (C), syntect's
  parser, and the fast lexers.

## Glossary

TextMate terms used throughout this plan, so the doc reads cold:

- **Scope** — dot-separated token label (`string.quoted.double.json`). Each
  token carries a **scope stack**: outermost grammar scope to innermost rule
  scope.
- **Rule** — a grammar entry. `match` rules consume within one line;
  `begin`/`end` rules push a frame that can span lines; `begin`/`while` rules
  continue only while `while` matches at each continuation line's start.
- **Capture** — per-regex-group scope assignment (`captures`,
  `beginCaptures`, `endCaptures`, `whileCaptures`); a capture entry may host
  nested `patterns` that re-tokenize the captured text.
- **`name` / `contentName`** — scope applied to the whole rule match vs. only
  the text between begin and end matches.
- **Repository** — a grammar's named rule table, referenced as `#name`.
- **Include** — rule reference: `#name` (repository), `$self` (this grammar's
  top level), `$base` (outermost grammar in an embedding chain), or an
  external scope (`source.js`).
- **Injection** — rules a grammar contributes into contexts selected by a
  **scope selector** (e.g. markdown injecting fenced-code grammars); `L:`/`R:`
  prefixes control priority relative to the host rules.
- **Anchor context** — whether `\A` (file start), `\G` (continuation
  position), and `^` may match for a given call; depends on where
  tokenization resumes, not on the pattern alone.
- **First-match-wins** — at each position all candidate rules are searched;
  the leftmost match wins, ties broken by rule order.

## Locked decisions

1. **Regex strategy: hybrid.** Grammar patterns are translated offline; the
   majority compile to `regex-automata` (lazy DFA, no backtracking blowups).
   Patterns needing lookaround/backreferences/`\G`-dependent behavior fall back
   to a small in-house backtracking matcher with a hard step budget. This is
   the primary performance lever versus both Shiki and onig.
   Caveat: by rule *hotness* the fallback is not rare — TypeScript, C++, and
   Ruby lean on lookaround in exactly their hottest rules (e.g. the TS
   grammar's function/arrow-detection patterns) — so the fallback matcher
   carries its own performance acceptance (Phase 2) and the Phase 0 inventory
   is an explicit go/no-go gate on this strategy. `regex-automata` and
   `aho-corasick` are new workspace dependencies (pure Rust; `memchr` is
   already a workspace dependency).
2. **Language coverage is an asset problem, not an engine problem.** Grammars
   come from the `tm-grammars` collection (Shiki's source, MIT with per-grammar
   licenses tracked), compiled offline and embedded in the CLI. Coverage grows
   by adding assets, never by mapping a language onto another language's lexer.
   The coverage target is an explicit keep/drop list produced in Phase 0, not
   the current raw count: today's 346 listed languages come from two-face's
   Sublime syntax set plus `FAST_ONLY_LANGUAGES`, while `tm-grammars` has
   roughly 220 grammars — raw-count parity is not achievable and is not the
   acceptance metric.
3. **No binary size ceiling.** The previous tree-sitter build was 26M; current
   is 5.8M. Embedding the full grammar set is acceptable; lazy per-language
   decode keeps startup and RSS flat.
4. **All fast-path lexers are deleted.** Rust, C-like, CompilerIr, LispLike,
   and Markup scanners in `mark-textmate` are removed in the cutover. No
   first-frame lexer path remains; the engine must win on its own via caching,
   checkpoints, and prefilters.
5. **One crate.** All syntax code moves into `mark-syntax`. The
   `mark-textmate` crate is deleted. `syntect`, `two-face`, and transitively
   `onig`/`onig_sys` are removed from the workspace.
6. **Grammar sources are committed as text; the bundle is generated.** The
   pinned `tm-grammars` JSON is vendored in-repo (text deltas compress well in
   git history); `bundle.bin` is produced deterministically at build time by
   `grammar-compile` (invoked via `build.rs` with output caching) and is never
   committed — committing it would add a multi-megabyte binary blob to history
   on every regeneration. `licenses.json` is committed for reviewability.
   Release builds stay offline and Node-free either way.

## Open decisions

Decisions deliberately deferred, tracked here so none of them defaults
silently mid-implementation. Update the Status column inline when resolved;
promote to Locked decisions if load-bearing.

| # | Decision | Options | Decided in | Status |
| --- | --- | --- | --- | --- |
| O1 | Regex strategy holds after inventory | proceed hybrid / widen translator (bounded lookahead lowering) / rethink | Phase 0 gate | Open |
| O2 | Coverage keep/drop list contents | per-language keep, drop, or alias | Phase 0 | Open |
| O3 | Anchor-context mechanism per pattern class | variant matrix / pattern rewrite / fallback routing | Phase 2 | Open |
| O4 | Bundle grammar-blob compression codec | none / lz4 / zstd | Phase 4 | Open |
| O5 | Build-time bundle generation confirmed vs committing `bundle.bin` | keep `build.rs` / commit binary | Phase 4, with build-cost numbers | Open (default: build-time, locked decision 6) |
| O6 | Checkpoint interval default | 64 / 128 / 256 lines | Phase 5, from measurements | Open |
| O7 | Engine line-cache sizing | fixed / derived from existing syntax settings | Phase 5 | Open |

## Current coupling map

These are the concrete seams to preserve or replace during the migration:

- `crates/mark-syntax/src/types.rs`
  - Re-exports `mark_textmate::{HighlightedLine, HighlightedText,
    LineTextFingerprint, SyntaxClass, SyntaxSegment}`.
  - `SyntaxHighlighter` wraps `mark_textmate::TextMateHighlighter`.
- `crates/mark-syntax/src/storage.rs`
  - Language availability/canonicalization/detection call into
    `mark_textmate::{available_languages, canonical_language,
    detect_language_from_path, has_language}`.
  - Owns Mark-specific config, aliases, filename overrides, and core-language
    policy that must survive unchanged.
- `crates/mark-syntax/src/highlight.rs`
  - Public `detect_language_from_path` delegates through storage.
  - Test-only `syntax_class` calls `mark_textmate::classify_scope_name`.
- `crates/mark-bench`
  - Depends directly on `mark-textmate` for raw Mark-vs-Shiki comparisons.
- Workspace dependencies
  - Root `Cargo.toml` exposes `syntect`, `two-face`, and `mark-textmate`.
  - `crates/mark-textmate/src/lib.rs` contains both the syntect wrapper and all
    fast-path lexers/catalog/classifier code.

Cutover is complete only when every item above points at in-crate
`mark-syntax` modules and no workspace target depends on `mark-textmate`.

## Target architecture

```
crates/mark-syntax/src/
  engine/
    grammar.rs     # rule model: match/begin/end/while, captures, repository,
                   # includes ($self/$base/#name), injections
    regex/
      ast.rs       # parsed Oniguruma-ish regex AST used by translator/fallback
      translate.rs # oniguruma-syntax -> regex-automata translation + feature
                   # detection (lookaround, backrefs, \G, \h, possessive)
      dfa.rs       # regex-automata wrapper, anchored/unanchored search
      captures.rs  # capture-span extraction for selected DFA/PikeVM matches
      backtrack.rs # fallback matcher with step budget
      prefilter.rs # required-literal extraction, memchr/Aho-Corasick line skip
    tokenizer.rs   # line-oriented begin/end stack machine, first-match-wins,
                   # capture scope application, while-rule continuation
    state.rs       # interned/hash-consed parser state (StateId), checkpoints
    scopes.rs      # scope interning + scope -> SyntaxClass classifier
                   # (ported from mark-textmate's classify_scope_text)
    line.rs        # current LineChunks behavior and byte/UTF-8 boundary helpers
  grammars/
    registry.rs    # lazy per-language decode of the embedded bundle
    bundle.rs      # include_bytes! + generated bundle metadata constants
    bundle.bin     # compiled grammar bundle (build-time generated, gitignored)
    licenses.json  # generated source/license manifest (committed)

assets/tm-grammars/  # pinned grammar JSON + per-grammar licenses
                     # (vendored text, committed; locked decision 6)
  detect.rs        # canonical names, aliases, extensions, basenames
                   # (ported from mark-textmate catalog tables)
  highlight.rs     # public API (existing shapes preserved)
  language.rs, paths.rs, storage.rs, types.rs   # existing config/settings mgmt

tools/ or src/bin/:
  grammar-compile  # tm-grammars JSON -> validated -> translated -> bundle.bin
  grammar-stats    # optional diagnostics over bundle/pattern feature coverage
  golden-dump.mjs  # dev-only vscode-textmate oracle dumper
```

### Public API contract

The external API of `mark-syntax` stays shape-compatible so `mark-tui`'s
runtime/queue/LRU layers and `mark-command`/`mark-cli` do not change:

- `SyntaxClass`, `SyntaxSegment`, `HighlightedLine` (+ fingerprints),
  `HighlightedText`, `SyntaxHighlighter::highlight(language, source)`
- `detect_language_from_path`, `canonical_language`, `has_language`,
  `available_languages`, `classify_scope_name`
- `language.rs` config management (add/remove/update/clean/doctor) unchanged;
  `TEXTMATE_BUNDLE_VERSION` becomes the compiled bundle's version stamp.
- Threading: `SyntaxHighlighter` must be `Send` (mark-tui highlights on a
  worker thread). Engine caches and interners are per-instance; the only
  shared state is the immutable embedded bundle.

`mark-bench` retargets its `mark-textmate` dependency to `mark-syntax`.

### Core types and seam interfaces

The signatures the workstreams build against. These freeze the A→C (assets →
tokenizer) and B→C (regex → tokenizer) seams so the parallel workstreams do
not serialize on each other; changing them after Phase 1 means updating this
section first.

```rust
// Interned identifiers (engine/state.rs, engine/scopes.rs)
struct GrammarId(u16);
struct RuleId(u32);   // stable order = grammar order (first-match-wins ties)
struct ScopeId(u32);  // interned scope name
struct StateId(u32);  // hash-consed (rule stack, scope stack, end substitutions)

// Rule model (engine/grammar.rs) — A→C seam
enum RuleBody {
    Match { pattern: PatternId, captures: CaptureSpec },
    BeginEnd {
        begin: PatternId, end: PatternId,
        begin_captures: CaptureSpec, end_captures: CaptureSpec,
        content_name: Option<ScopeId>, apply_end_pattern_last: bool,
        patterns: Vec<RuleRef>,
    },
    BeginWhile { /* begin/while analogues of BeginEnd */ },
    IncludeOnly { patterns: Vec<RuleRef> },
}
enum RuleRef { Rule(RuleId), SelfRef, BaseRef, External(ScopeId) }

// Matching (engine/regex/) — B→C seam
struct AnchorContext { allow_a: bool, allow_g: bool, g_pos: usize }
struct MatchResult {
    start: usize, end: usize,                  // UTF-8 byte offsets
    captures: Vec<Option<Range<usize>>>,       // by group number, 0 = whole
}
trait Matcher {
    /// Leftmost match at or after `from`, or None if the line cannot match.
    fn find(&self, line: &str, from: usize, ctx: AnchorContext)
        -> Option<MatchResult>;
}
// A compiled pattern is DFA-translated, fallback-routed, or a literal.
// Candidate sets additionally expose "earliest match among these patterns"
// as one call so the tokenizer never loops N regex searches per position.

// Tokenizer (engine/tokenizer.rs) — C→D seam
fn tokenize_line(&mut self, line: &str, entry: StateId) -> LineTokens;
struct LineTokens {
    tokens: Vec<(Range<usize>, ScopeStackId)>,
    exit: StateId,
}
// highlight.rs converts ScopeStackId -> SyntaxClass -> SyntaxSegment.

// Grammar access (grammars/registry.rs) — A→C seam
fn grammar(&self, language: &str) -> Result<&CompiledGrammar, RegistryError>;
// Lazy decode; external includes resolve through the registry at first use.
```

Sketches, not final code: field names may shift, but arity, ownership (who
allocates, who interns), and the UTF-8 byte-offset convention are fixed.

### Worked example: one line through the pipeline

The reference trace for the whole plan. Grammar: JSON (`source.json`), whose
string rule is small but exercises begin/end, captures, nested patterns, and
repository includes:

```json
"string": {
  "begin": "\"", "end": "\"",
  "name": "string.quoted.double.json",
  "beginCaptures": { "0": { "name": "punctuation.definition.string.begin.json" } },
  "endCaptures":   { "0": { "name": "punctuation.definition.string.end.json" } },
  "patterns": [ { "include": "#stringcontent" } ]
},
"stringcontent": {
  "patterns": [
    { "match": "\\\\(?:[\"\\\\/bfnrt]|u[0-9a-fA-F]{4})",
      "name": "constant.character.escape.json" },
    { "match": "\\\\.",
      "name": "invalid.illegal.unrecognized-string-escape.json" }
  ]
}
```

Input line: `"a\n"` — five bytes `"`, `a`, `\`, `n`, `"` — entered at the
top-level state of `source.json`.

1. **Phase 1 (grammar model).** The string rule deserializes to
   `RuleBody::BeginEnd` whose `patterns` hold one `RuleRef` to the repository
   rule `#stringcontent`, itself `IncludeOnly` over two `Match` rules. Rule
   ids are assigned in grammar order.
2. **Phase 2 (regex layer).** `\"` compiles to the DFA path with single-byte
   prefilter literal `"`. The escape pattern compiles to the DFA path
   (alternation plus bounded repetition, no lookaround), prefilter literal
   `\`. Nothing here is anchor-sensitive, so no `allowA`/`allowG` variants
   are stored.
3. **Phase 3 (tokenizer).** From position 0 the top-level state's candidates
   are searched; the string rule's `begin` wins at 0..1 and pushes a frame.
   The begin capture applies `punctuation.definition.string.begin.json` over
   0..1. Inside the frame the candidates are `#stringcontent`'s two rules
   plus the active `end`. At position 2 the escape rule matches 2..4
   (leftmost-wins against `end`, which first matches at 4), with stack
   `[source.json, string.quoted.double.json,
   constant.character.escape.json]`. At 4 the `end` matches 4..5, applies its
   capture, and pops the frame. The exit state equals the entry state, so
   both hash-cons to the same `StateId`.
4. **Classification/output.** Each token's innermost classifiable scope maps
   through the ported `classify_scope_text`:

   | Bytes | Innermost classified scope | `SyntaxClass` |
   | --- | --- | --- |
   | 0..1 | `punctuation.definition.string.begin.json` | `Punctuation` |
   | 1..2 | `string.quoted.double.json` | `String` |
   | 2..4 | `constant.character.escape.json` | `Constant` |
   | 4..5 | `punctuation.definition.string.end.json` | `Punctuation` |

   Output: four `SyntaxSegment`s (no adjacent merges — classes differ) plus
   the line's `LineTextFingerprint`. The golden-token test for this line
   asserts the exact scope stacks of step 3; the public-API test asserts only
   this table.

### Engine performance design (built in from day one)

- **Interned parser state.** Equal (rule stack, scope stack) states hash-cons
  to a `StateId`. The line cache can key on `(StateId, line fingerprint)` and
  skip re-tokenizing unchanged lines — the dominant case in diff review where
  most lines are context.
- **Line-state checkpoints** every N lines per file so viewport-first
  highlighting resumes from the nearest checkpoint instead of replaying from
  line zero (previously listed as future work; now a requirement).
- **Per-state candidate rule compilation.** Each unique state caches its
  flattened candidate rule list; matching scans all candidates leftmost-first
  in one pass rather than N sequential regex searches.
- **Prefilters.** Rules with required literals are guarded by memchr /
  Aho-Corasick scans; lines that cannot match skip regex work entirely.
- **Per-line step/time budget** with graceful plain-text degradation,
  preserving the minified-file guard behavior.
- **Range-only segments + fingerprints** (existing optimization) retained
  unchanged.

## Execution strategy

### Migration principles

- Build the new engine behind `mark-syntax` first; keep the old backend only as
  an oracle/comparison path until hard cutover.
- Every phase must leave the workspace compiling.
- Every correctness increment gets a fixture and either a golden-token test or a
  property test before adding the next engine feature.
- Every performance optimization gets a before/after artifact in `target/` and
  can be deleted if it does not move measured workloads.
- No silent grammar loss: unsupported patterns are reported by the compiler,
  not discovered by users at runtime.

### Parallel workstreams

- **A. Assets/catalog:** grammar source pinning, license manifest, bundle format,
  lazy registry, language aliases/detection.
- **B. Regex:** Oniguruma syntax parser/translator, regex-automata integration,
  fallback matcher, required-literal extraction.
- **C. Tokenizer/correctness:** TextMate stack machine, captures, injections,
  golden harness, fixture corpus.
- **D. Integration/performance:** public API swap, line cache/checkpoints,
  benchmark artifacts, deletion of old dependencies.

Critical path: B must provide enough matching for C; A must provide enough
compiled grammars for C's fixture corpus; D can begin with a dev-JSON grammar
loader but cannot cut over until A+B+C are accepted.

## Baseline

Capture these before deleting or refactoring `mark-textmate`. Store the exact
JSON artifacts under `target/` and paste the summary table into this file.

Suggested commands:

```sh
cargo run --release -p mark-bench -- fixtures \
  --out target/bench-fixtures --syntax --force

cargo run --release -p mark-bench -- measure \
  --fixtures target/bench-fixtures --syntax \
  --syntax-language rust --syntax-language typescript \
  --json > target/pre-engine-measure-fixtures.json

cargo run --release -p mark-bench -- measure-repo \
  --repo ~/sandbox/bun-rust-pr \
  --syntax-language rust --syntax-language typescript \
  --json > target/pre-engine-measure-bun-rust-pr.json

cargo run --release -p mark-bench -- syntax-compare \
  --repo ~/sandbox/bun-rust-pr \
  --language rust --language typescript --language zig --language c --language cpp \
  --shiki --json > target/pre-engine-raw-compare-bun-core.json

cargo build --release -p mark-cli
ls -lh target/release/mark
cargo run --release -p mark-cli -- syntax list | wc -l
```

Checklist:

- [ ] Re-capture current-system benchmarks on this machine: `mark-bench
      measure` syntax fixtures and integrated `~/sandbox/bun-rust-pr` workload,
      saved to `target/pre-engine-*.json`
- [ ] Capture raw `syntax-compare --shiki` artifacts for rust, typescript, zig,
      c/cpp on the Bun corpus: `target/pre-engine-raw-compare-*.json`
- [ ] Record current release binary size; save the full `mark syntax list`
      output to `target/pre-engine-syntax-list.txt` (input to the Phase 0
      coverage keep/drop list) and record the count
- [ ] Record `cargo tree -p mark-cli | rg -i 'syntect|two.face|onig'` output as
      the dependency baseline

Prior-system reference numbers (from the syntect migration): integrated
many-small-rust 36,390µs / large-rust 45,148µs; raw Mark-vs-Shiki speedups
111–162x on fast-path languages; binary 5.8M; 346/346 languages listed.
The new engine must not regress the integrated numbers and must beat the
*syntect fallback* numbers (e.g. TypeScript fallback 304,846µs vs fast path
36,791µs) with full-grammar fidelity.

### Baseline results table

| Artifact | Scenario/languages | Current result | Notes |
| --- | --- | ---: | --- |
| `target/pre-engine-measure-fixtures.json` | syntax fixtures | TBD | Fill after capture |
| `target/pre-engine-measure-bun-rust-pr.json` | Bun PR integrated | TBD | Fill after capture |
| `target/pre-engine-raw-compare-bun-core.json` | Mark vs Shiki raw | TBD | Fill after capture |
| `target/release/mark` | release binary size | TBD | Fill after capture |
| `mark syntax list` | installed languages | TBD | Fill after capture |

## Correctness gate

The old system never had a token-level oracle. Add one before trusting the new
engine.

### Reference implementation map

When the golden harness diverges, first check what the reference does. The
behaviors being replicated live in `vscode-textmate` (`src/`); pin its
revision next to the tm-grammars revision so citations and oracle output stay
stable.

| Behavior | Reference location |
| --- | --- |
| Main line-tokenization loop, first-match-wins, zero-width guards | `grammar/tokenizeString.ts` (`_tokenizeString`) |
| Rule compilation, pattern lists, `allowA`/`allowG` variant compilation | `rule.ts` (`RegExpSource`, `RegExpSourceList`, `compileAG`) |
| Begin-capture substitution into `end`/`while` | `rule.ts` (`RegExpSource.resolveBackReferences`) |
| `while` continuation checks at line start | `grammar/tokenizeString.ts` (`_checkWhileConditions`) |
| Capture application incl. nested capture patterns | `grammar/tokenizeString.ts` (`handleCaptures`) |
| Injection collection and `L:`/`R:` ordering | `grammar/grammar.ts` (`_collectInjections`), `grammar/basicScopesAttributeProvider.ts` |
| Scope selector matching | `matcher.ts` |
| Scope stack representation | `grammar/grammar.ts` (`StateStackImpl`, `AttributedScopeStack`) |
| Oniguruma flags/string handling at the FFI edge | `vscode-oniguruma` `main.ts` |

File paths are per current upstream layout; adjust to the pinned revision if
they moved, and record the pinned commit here.

### Golden-token oracle

- [ ] `tools/golden-dump.mjs`: runs `vscode-textmate` + `vscode-oniguruma` over
      a fixture corpus and writes JSONL containing:
      - grammar scope name and language id
      - source line text
      - per-line tokens with UTF-16 start/end offsets and full scope stacks
      - final rule stack serialization/hash for continuation debugging
      - the dumper must pin/disable vscode-textmate's default max-line-length
        guard (~20k chars) so minified fixtures compare against our own budget
        behavior, not the oracle's silent truncation
- [ ] Rust golden loader converts UTF-16 offsets to UTF-8 byte offsets and
      compares:
      - exact scope span boundaries for engine-tokenizer tests
      - coarse `SyntaxClass` spans for public highlighter tests
- [ ] Divergences must be explicit in a checked-in allowlist (for example
      `crates/mark-syntax/tests/fixtures/textmate/divergences.toml`) with a
      reason, affected grammar, and fixture line range.

### Fixture corpus

Minimum corpus:

- [ ] rust
- [ ] typescript
- [ ] tsx
- [ ] javascript
- [ ] json
- [ ] yaml
- [ ] toml
- [ ] markdown
- [ ] html
- [ ] css
- [ ] python
- [ ] go
- [ ] c
- [ ] cpp
- [ ] bash
- [ ] zig

Required stress snippets:

- [ ] Nested block comments and nested begin/end rules
- [ ] Strings/comments spanning multiple lines
- [ ] Regex literals vs division in JavaScript/TypeScript
- [ ] JSX/TSX tags, attributes, and embedded expressions
- [ ] Markdown fenced code blocks with embedded grammars
- [ ] HTML `<script>`/`<style>` embedded grammars
- [ ] YAML/TOML bare keys, numbers, strings, comments
- [ ] Shell heredocs and command substitutions
- [ ] C/C++ preprocessor lines and raw strings
- [ ] Minified one-line file exceeding line-budget guard
- [ ] Non-ASCII text to prove byte offsets remain char-boundary aligned

### Property/fuzz tests

- [ ] Tokenizer never panics on arbitrary UTF-8 input
- [ ] Tokenizer never loops: zero-width matches advance or are budget-killed
- [ ] Segments are monotonic, char-boundary-aligned, non-overlapping, and cover
      each line when expanded with `None` spans
- [ ] Parser state after re-tokenizing from the nearest checkpoint equals state
      after tokenizing from line zero
- [ ] Backtracker step-budget kill switch trips on pathological patterns and
      degrades only the affected line/range

## Detailed implementation plan

### Phase 0 — baseline, inventory, and guardrails

Goal: freeze measurable behavior and know exactly what the engine must replace.

Deliverables:

- Baseline artifacts listed above.
- Pattern-feature inventory from the chosen `tm-grammars` revision.
- A feature flag or dev-only constructor that lets tests run old and new engines
  side-by-side without exposing a user-facing backend switch.

Tasks:

- [ ] Pin the grammar source revision (Shiki `tm-grammars` snapshot) and record
      the commit/version in this file.
- [ ] Generate an initial grammar inventory:
      - grammar count
      - pattern count
      - include graph edges
      - injection count
      - regex feature histogram (`lookahead`, `lookbehind`, backrefs, `\G`,
        conditionals, named groups, possessive/atomic groups, inline flags,
        Unicode/POSIX classes)
      - estimated DFA-able vs fallback-able percentages, both raw and weighted
        by rule hotness (patterns reachable from the top-level contexts of the
        core fixture languages)
      - anchor-context census: patterns whose behavior depends on `\A`, `\G`,
        or `^`-at-resume, to size the compiled-variant cost decided in Phase 2
- [ ] Coverage keep/drop list: diff `target/pre-engine-syntax-list.txt`
      against the pinned tm-grammars catalog (grammars plus aliases) and
      record in this file which currently listed languages are kept
      (grammar-backed), dropped (no tm-grammars grammar, previously
      Sublime-only or fast-only), or remapped via alias. All coverage
      acceptance criteria are defined against this list, not the raw count.
- [ ] Decision gate on locked decision 1: review the hotness-weighted
      histogram. If fallback-routed patterns dominate the hot rules of core
      languages (working threshold: >40% of projected hot-rule match attempts
      hitting fallback), revisit the regex strategy before building Phase 2 —
      e.g. widen translator coverage with bounded lookahead lowering rather
      than accepting a backtracking hot path.
- [ ] Add a temporary `mark-syntax` internal API for comparing:
      - old `mark_textmate::TextMateHighlighter`
      - new engine against raw dev-loaded grammars
      - `vscode-textmate` golden output
- [ ] Add CI-safe smoke fixtures small enough to run in normal `cargo test`.
      Larger corpus comparisons can be ignored-by-default until stable.
- [ ] Implement generated-file policy (locked decision 6):
      - pinned tm-grammars JSON and per-grammar licenses vendored under
        `assets/tm-grammars/` (text, committed)
      - `bundle.bin` generated at build time by `grammar-compile` via
        `build.rs` with output caching; deterministic, gitignored
      - `licenses.json` committed and regenerated alongside the bundle
      - release builds require no network access and no Node
      - record expected `bundle.bin` size and clean-build compile cost; if
        build-time generation proves too slow, the fallback is committing
        `bundle.bin` and accepting history churn — decide with numbers

Definition of done:

- Workspace still uses old engine for production.
- Baseline numbers are recorded.
- Grammar inventory can be regenerated deterministically.
- The coverage keep/drop list is recorded in this file and the regex-strategy
  decision gate has an explicit pass/revisit verdict.

### Phase 1 — grammar model and dev loader

Goal: represent enough TextMate grammar semantics to run the tokenizer against
raw JSON before optimizing or bundling.

Deliverables:

- `engine::grammar` model with stable IDs and normalized includes.
- Dev-only loader for raw tmLanguage JSON fixtures.
- Unit tests for grammar parsing independent of regex/tokenization.

Tasks:

- [ ] Define IDs and interners:
      - `GrammarId`, `RuleId`, `ScopeId`, `StringId`
      - stable rule order for TextMate first-match-wins semantics
- [ ] Model rule variants:
      - `match`
      - `begin`/`end`
      - `begin`/`while`
      - pure include/repository references
      - nested `patterns`
      - `captures`, `beginCaptures`, `endCaptures`, `whileCaptures`
      - `name` and `contentName`
      - `applyEndPatternLast` if present in source grammars
- [ ] Resolve includes:
      - `$self`
      - `$base`
      - `#repository-name`
      - external grammar scope includes (`source.js`, `text.html.markdown`, etc.)
      - missing/optional includes reported with grammar context
- [ ] Parse and store injection metadata:
      - `injections` tables
      - injection selectors
      - left/right priority markers where used (`L:`/`R:`)
- [ ] Preserve detection metadata:
      - grammar `scopeName`
      - aliases/name
      - file extensions/fileTypes
      - first-line matches (tracked for later even if not initially used)
- [ ] Add round-trip/debug dumps for a single grammar so failures are inspectable.

Definition of done:

- The core fixture grammars deserialize and their include graphs resolve.
- Grammar parse failures point to grammar file, rule path, and offending field.

### Phase 2 — regex translation and matching layer

Goal: make pattern matching correct enough for TextMate tokenization while
separating fast deterministic search from rare fallback behavior.

Deliverables:

- Regex feature classifier with per-pattern stats.
- DFA/PikeVM path for supported patterns.
- In-house fallback matcher with step budget for unsupported constructs.
- Required-literal extraction for prefilters.
- Anchor-context (`allowA`/`allowG`) strategy decided and implemented, since
  it shapes the bundle format.
- Fallback-matcher benchmarks on hot patterns with their own acceptance
  numbers.

Tasks:

**Milestone 2a — regex AST parser.** Exit artifact: `cargo run --example
regex-parse -- '<pattern>'` prints the parsed AST and the feature
classification (DFA-able vs fallback, with reasons).

- [ ] Parse Oniguruma-ish regex syntax into an internal AST before lowering.
      Required coverage:
      - literals/escapes and inline flags
      - character classes, POSIX classes, Unicode-ish classes used by grammars
      - anchors: `^`, `$`, `\A`, `\z`, `\Z`, `\G`
      - alternation, groups, quantifiers, lazy quantifiers
      - lookahead/lookbehind
      - numbered/named captures and backreferences
      - atomic/possessive constructs classified even if fallback-only
**Milestone 2b — translator and DFA path, including anchor contexts.** Exit
artifact: `regex-parse --match '<pattern>' '<line>'` runs the translated
pattern with explicit `allowA`/`allowG`/position flags and prints match and
capture spans.

- [ ] Implement translator to `regex-automata`:
      - anchored and unanchored search modes
      - multi-pattern sets for candidate lists
      - case-insensitive and multiline flag handling matching TextMate line mode
      - two-stage capture extraction when DFA search finds a winner but capture
        spans are needed
- [ ] Anchor-context (`allowA`/`allowG`) handling:
      - `^`, `\A`, and `\G` validity depends on call-site context: `^` must
        not match when tokenization resumes mid-line after a begin match,
        `\A` only on the first line, `\G` only at the continuation position
      - decide per pattern class between a compiled variant matrix
        (vscode-textmate compiles up to 4 variants per pattern set), pattern
        rewriting, and fallback routing
      - the decision determines bundle-format variant storage and DFA memory;
        justify it with the Phase 0 anchor-context census numbers
      - test resumed-mid-line `^` non-matching explicitly (begin/end rules
        whose inner patterns start with `^`)
**Milestone 2c — fallback VM and end-pattern substitution.** Exit artifact:
the same example with `--engine fallback` printing step counts, plus a
demonstrated budget kill on a pathological pattern.

- [ ] Implement fallback matcher:
      - bounded backtracking VM over the same AST
      - support `\G` as current scan-position anchor
      - support lookaround and backrefs used by inventory
      - hard step budget, deterministic error/degradation result
      - no heap blowups on nested quantifiers
- [ ] Implement begin-capture substitution into `end`/`while` patterns:
      - cache compiled substituted end patterns per unique capture text tuple
      - cap cache size and pattern length
      - test with grammars that use `begin` captures in `end`
**Milestone 2d — prefilters, benchmarks, conformance.** Exit artifacts: the
conformance report against `vscode-oniguruma` and the fallback microbenchmark
table, both under `target/`.

- [ ] Required-literal/prefilter extraction:
      - find mandatory byte literals for common alternations/sequences
      - use `memchr` for one-byte literals
      - use Aho-Corasick or equivalent for multi-literal sets
      - disable prefilter when extraction is uncertain
- [ ] Fallback matcher benchmarks (deliverable, not optional — this is a hot
      path for core languages, not a rare escape hatch):
      - microbenchmark the hottest fallback-routed patterns from the Phase 0
        inventory (e.g. TypeScript function/arrow detection, C++ raw strings,
        Ruby heredocs) on representative source lines
      - acceptance: the fallback matcher meets or beats onig on these
        patterns/inputs; otherwise the pattern gets translator work or the
        strategy gate reopens
- [ ] Regex conformance tests:
      - direct unit tests for Oniguruma features from inventory
      - comparison tests against `vscode-oniguruma` for representative patterns
      - step-budget/pathological tests

Definition of done:

- Every regex in the core fixture corpus is either translated or deliberately
  routed to fallback with a reason.
- The matcher API returns match start/end and capture spans using UTF-8 byte
  offsets aligned to Rust string boundaries.

### Phase 3 — tokenizer and scope classification

Goal: implement TextMate line tokenization semantics and produce the existing
`HighlightedText` shape.

Deliverables:

- `engine::tokenizer` that passes golden fixtures for the core corpus.
- Scope interning/classification ported from `mark-textmate`.
- Public highlighter path still hidden behind old backend until performance and
  bundle work are ready.

Tasks:

**Milestone 3a — stack machine core.** Exit artifact: `cargo run --example
tokenize -- --grammar <path>.json <file>` dumps per-line scope stacks and
exit `StateId`s for injection-free grammars (json, toml) using minimal
in-grammar include resolution.

- [ ] Port the existing line splitting behavior:
      - output excludes trailing `\n`
      - parser sees the same line text/newline behavior as syntect currently did
      - final empty line behavior matches `HighlightedText` tests
- [ ] Implement stack machine:
      - frame stores rule id, scope stack delta, content scope, end/while matcher,
        begin capture texts needed for substitution, and injection context
      - active end/while rule participates with correct priority
      - `while` rules are checked at the beginning of continuation lines
      - `applyEndPatternLast` honored where supported
      - zero-width matches cannot loop
**Milestone 3b — candidate flattening, cross-grammar includes, injections.**
Exit artifact: markdown fenced-code and HTML `<script>`/`<style>` fixtures
tokenize with correct embedded scopes in the `tokenize` example.

- [ ] Implement candidate flattening for a parser state:
      - repository/includes resolved once
      - current grammar/base/self scopes represented explicitly
      - active injections selected by scope selector
      - stable order preserves TextMate first-match-wins after leftmost search
**Milestone 3c — captures.** Exit artifact: capture-heavy fixtures (JSON
strings, C preprocessor lines, shell substitutions) pass exact scope-stack
golden comparisons.

- [ ] Implement captures:
      - apply `captures`/`beginCaptures`/`endCaptures`/`whileCaptures`
      - nested capture patterns where grammars require them
      - overlapping captures resolved to TextMate-equivalent scope transitions
      - unmatched/empty captures ignored safely
**Milestone 3d — classification, output conversion, full golden harness.**
Exit artifact: `tokenize --classes` dumps `SyntaxSegment`s, and the golden
harness passes for the full core corpus.

- [ ] Implement scope stack to class conversion:
      - port `classify_scope_text`
      - preserve Tag/Attribute priority behavior
      - cache by `ScopeId`/stack fingerprint rather than string rebuilding per
        segment
- [ ] Output conversion:
      - produce `SyntaxSegment { byte_start, byte_end, class }`
      - merge adjacent same-class segments
      - retain `LineTextFingerprint`
      - keep range-only segments; do not copy source text
- [ ] Golden tests:
      - exact scope-stack comparisons for tokenizer internals
      - coarse-class comparisons for public API
      - documented divergence mechanism

Scope selector semantics (mini-spec; reference `matcher.ts`):

- Selector grammar: comma-separated alternatives (any alternative matching
  applies the injection); within an alternative, `|` (or), `&` (and), `-`
  (subtraction), parentheses, and space-separated scope-path sequences.
- A scope path (`text.html markup.fenced_code`) matches a scope stack iff its
  components match stack entries in order with gaps allowed (descendant
  combinator).
- A component matches a stack entry iff it equals the entry or is a
  dot-boundary prefix: `text.html` matches `text.html.markdown` but not
  `text.htmlx`.
- Injection priority: `L:`-prefixed selectors contribute candidates *before*
  the host state's own patterns; unprefixed and `R:` contribute after. Within
  a priority band, grammar registration order is preserved.
- Selectors are evaluated at candidate-flattening time against the frame's
  scope stack, and the result is cached on the flattened candidate list per
  `StateId`.

Capture application semantics (mini-spec; reference `handleCaptures`):

- Group 0 is the whole match; groups apply in ascending group-number order.
- A group that did not participate in the match, or matched an empty range,
  applies nothing.
- Group ranges from a single regex are always properly nested or disjoint —
  never partially overlapping — so capture scopes apply as pure nesting: an
  inner group's scope stacks on top of the enclosing group's scope.
- A capture entry with nested `patterns` re-tokenizes the captured text with
  those patterns; the resulting scopes stack inside the capture's own scope
  and cannot extend past the captured range.
- `beginCaptures`/`endCaptures`/`whileCaptures` number against their own
  pattern's groups; a plain `captures` key on a begin/end rule is shorthand
  for both begin and end captures.

Definition of done:

- Golden-token harness passes for at least rust, typescript, json, yaml, toml,
  markdown, html, css, python, go, c/cpp, bash, and zig fixture snippets.
- Public `SyntaxHighlighter` output is shape-compatible with existing callers
  (same types, segment invariants, and fingerprint behavior). Exact segment
  boundaries will legitimately differ from the old fast-path lexers — the
  correctness reference is the vscode-textmate oracle, never old-engine
  output.

### Phase 4 — grammar compiler and embedded bundle

Goal: move from dev JSON loading to a deterministic embedded asset bundle with
lazy per-language decode.

Deliverables:

- `grammar-compile` tool wired into `build.rs` with output caching.
- Vendored grammar sources committed under `assets/tm-grammars/`;
  `crates/mark-syntax/src/grammars/bundle.bin` generated at build time
  (locked decision 6).
- License/source manifest committed.
- Registry APIs replacing `mark_textmate` catalog functions.

Tasks:

- [ ] Bundle format:
      - magic + format version
      - source grammar revision/hash
      - bundle version exposed as `TEXTMATE_BUNDLE_VERSION`
      - string table / scope table / language table
      - per-language compressed or compact binary grammar blob
      - regex translation artifacts and feature flags
      - anchor-context variant artifacts where the Phase 2 decision requires
        compiled variants
      - license offsets/references
- [ ] Compiler pipeline:
      - read pinned grammar source
      - validate required fields
      - normalize aliases/extensions/basenames
      - resolve includes and injection targets
      - translate regexes and emit fallback reasons
      - fail on unsupported required features unless explicitly allowlisted
      - emit deterministic output (stable ordering, no timestamps)
- [ ] Registry:
      - `available_languages()` from bundle metadata
      - `canonical_language()` from bundle language/alias table
      - `has_language()` from successfully compiled grammar availability
      - `detect_language_from_path()` from basename + extension aliases
      - lazy grammar decode/cache with bounded memory
      - external grammar includes resolve lazily at first use: decoding
        markdown/html must not eagerly decode every fence/embedded grammar
        (markdown injects essentially all fence-able languages, which would
        otherwise defeat lazy decode for a very common case)
- [ ] Port curated catalog behavior:
      - `LANGUAGE_ALIASES`
      - `EXTENSION_ALIASES`
      - `BASENAME_ALIASES`
      - code-fence tokens
      - `*.v` -> Verilog while `v` remains V language
      - `*.h` -> C
      - JSX/TSX use React grammars
      - existing Mark-specific core languages remain enabled
- [ ] License compliance:
      - generated manifest includes language, grammar source path, upstream URL
        if available, license text/id, and source revision
      - `doctor`/debug command can report bundle version and grammar count
- [ ] Measurement:
      - cold decode time per core grammar
      - warm decode/highlight time
      - cold-open of a markdown file (transitively include-heavy worst case):
        decode time and RSS measured separately from single-grammar costs
      - binary size delta from old release; `bundle.bin` size and clean-build
        generation cost
      - startup cost for `mark --help` / `mark syntax list`

Bundle format layout (spec — elaborates the "Bundle format" task; final
byte-level truth lives in `grammar-compile` and `bundle.rs`, kept in sync
with this table):

All integers little-endian; offsets absolute from byte 0; sections 8-byte
aligned, in this order:

| # | Section | Contents |
| --- | --- | --- |
| 1 | Header | magic `MRKB`, `u16` format version, `u16` reserved, `u64` tm-grammars source revision hash, bundle version stamp (exposed as `TEXTMATE_BUNDLE_VERSION`) |
| 2 | Section table | per section: `u32` section id, `u64` offset, `u64` byte length |
| 3 | String table | `u32` count, offset array, UTF-8 bytes; all names/aliases/scopes reference it by `u32` index |
| 4 | Scope table | sorted string-table indices; `ScopeId` = position, binary-searchable |
| 5 | Language table | per language: canonical name, aliases, extensions, basenames, optional first-line pattern, grammar-blob reference; sorted by canonical name |
| 6 | Grammar blobs | per grammar, independently compressed (codec per O4): serialized rule model, translated pattern artifacts, feature flags, fallback programs, prefilter literals, anchor-variant data |
| 7 | License table | per grammar: source path, upstream URL, SPDX id, license text (string-table refs) |

Format rules:

- Format version bumps on any layout change; the bundle version stamp is a
  hash of sections 3–7 and changes on any content change.
- Sections 3–5 decode eagerly at first use (small); grammar blobs decode
  lazily per language (locked decision 3).
- Determinism: every table sorted by a stated key, no timestamps, compressor
  version and flags pinned.
- Readers reject unknown format versions but skip unknown section ids
  (forward-compatible additions).

Definition of done:

- Release build needs no network or Node.
- `mark syntax list` matches the Phase 0 coverage keep list exactly; every
  listed language has a real grammar-backed highlighter, and every removal is
  on the agreed drop list.
- Bundle can be regenerated with a single documented command and is
  byte-identical when inputs are unchanged.

### Phase 5 — performance layer

Goal: make the real grammar engine meet or beat current integrated benchmark
behavior without reintroducing hand-written language lexers.

Deliverables:

- Interned state and line-cache integration.
- Checkpointed viewport-first highlighting.
- Candidate-list and prefilter caches.
- Per-line degradation behavior.
- Performance reports before/after each optimization.

Tasks:

- [ ] Instrument the engine with opt-in counters:
      - grammar decode µs
      - lines tokenized/skipped
      - checkpoint replay lines
      - state cache hits/misses
      - candidate-list cache hits/misses
      - regex attempts by DFA/fallback
      - fallback steps and budget kills
      - degraded lines
- [ ] Interned parser state:
      - hash-cons `(rule stack, scope stack, substituted end ids)` to `StateId`
      - keep stable across a highlighter instance
      - expose enough debug data for golden/checkpoint tests
- [ ] Line cache:
      - ownership: this cache serves engine-internal reuse (checkpoint
        replay, repeated context lines within and across highlight calls);
        mark-tui's existing `HighlightedText` LRU remains the outer cache and
        does not change. Size the engine cache accordingly, do not
        double-cache full outputs, and account for both layers in RSS
        measurements
      - key by `(language, bundle_version, StateId, LineTextFingerprint)`
      - value stores segments and ending `StateId`
      - bounded LRU sized by existing syntax settings where possible
      - invalidates when bundle/settings change
- [ ] Checkpoints:
      - configurable interval, default small enough for viewport latency
      - resume from nearest prior checkpoint
      - update checkpoints after edits/re-highlights
      - property test equivalence to replay-from-zero
- [ ] Candidate matching optimization:
      - per-state flattened candidates
      - combine DFA-able rules into multi-pattern search
      - choose earliest match; tie-break by candidate order
      - run capture extraction only for winning pattern(s)
      - route fallback patterns without starving earlier DFA candidates
- [ ] Prefilters:
      - apply required-literal skip before regex work
      - collect skip/hit stats
      - benchmark on normal source, markdown/html, and minified files
- [ ] Budget/degradation:
      - line byte limit parity with current settings
      - fallback step budget per line and per highlighter call
      - degrade affected line/range to plain text, not the whole file, unless
        parser state is unsafe
      - surface debug counters but avoid noisy user errors
- [ ] Benchmark discipline:
      - run before/after artifacts for each optimization
      - document any optimization removed because it did not pay off

Definition of done:

- Integrated fixture and Bun PR numbers meet acceptance thresholds.
- Previously syntect-only languages are substantially faster while keeping
  golden fidelity.
- Minified/pathological files cannot hang the UI.

### Phase 6 — public API switch and hard cutover

Goal: make the in-house engine the only engine in the workspace.

Deliverables:

- `SyntaxHighlighter` uses `mark-syntax::engine`.
- `mark-textmate` crate deleted.
- `syntect`, `two-face`, `onig`, `onig_sys` absent from dependency graph.
- Docs/status wording updated.

Tasks:

- [ ] Move/own public types in `mark-syntax`:
      - `SyntaxClass`
      - `SyntaxSegment`
      - `HighlightedLine`
      - `LineTextFingerprint`
      - `HighlightedText`
      - `HighlightError` equivalent mapped to `MarkError`
- [ ] Replace storage/catalog calls:
      - `bundled_highlight_language_set()` -> bundle registry
      - `core_enabled_language_set()` -> bundle registry canonicalization
      - `normalize_language_name()` -> in-crate canonical/detect
      - `has_highlights()` -> bundle grammar readiness
- [ ] `SyntaxHighlighter` in `types.rs` switches to the in-house engine.
- [ ] Delete all fast-path lexers (Rust, C-like, CompilerIr, LispLike, Markup)
      and `FAST_ONLY_LANGUAGES`.
- [ ] Move surviving `mark-textmate` logic (API types, classifier, catalog
      overrides) into `mark-syntax`; delete `crates/mark-textmate`.
- [ ] Retarget `mark-bench` to `mark-syntax`; rename raw report engine label
      from `mark-textmate` to `mark-syntax`/`mark-textmate-engine` as desired.
- [ ] Remove `mark-textmate`, `syntect`, and `two-face` from workspace
      `Cargo.toml`; verify `onig`/`onig_sys` gone from `Cargo.lock` and
      `cargo tree -p mark-cli`.
- [ ] Update `mark syntax doctor`/status wording that references bundled
      grammar internals; `TEXTMATE_BUNDLE_VERSION` sourced from the bundle.
- [ ] Remove temporary old-vs-new comparison API and any feature flags that can
      accidentally ship dual backends.

Definition of done:

- `rg -n "mark-textmate|mark_textmate|syntect|two-face"` has no live code
  references except historical notes in this migration file if intentionally
  kept.
- `onig`/`onig_sys` are verified absent via `Cargo.lock` and
  `cargo tree -p mark-cli` only — do not grep bare `onig` over `crates/`,
  because the engine's regex translator legitimately references Oniguruma
  syntax in module names, comments, and docs.
- Workspace builds/tests without the old crate/dependencies.

### Phase 7 — final measurement, docs, and release readiness

Goal: prove the migration improved correctness and did not regress the product.

Deliverables:

- Post-engine benchmark artifacts beside pre-engine artifacts.
- Updated README/docs syntax sections.
- Completed acceptance checklist.

Tasks:

- [ ] Re-run integrated `mark-bench measure` fixtures and the Bun PR workload;
      record tables here alongside the pre-engine baseline.
- [ ] Re-run `syntax-compare --shiki` for rust, typescript, zig, c/cpp, and at
      least three previously-syntect-only languages (e.g. yaml, markdown,
      bash) to prove fallback-class languages improved.
- [ ] Record binary size, RSS deltas, `mark syntax list` count, and Shiki
      language-ID audit result.
- [ ] Update README/docs syntax sections.
- [ ] Add release notes explaining:
      - grammars are embedded
      - no external syntax downloads are required
      - language mappings remain configurable
      - highlighting quality is now grammar-backed for every listed language

Definition of done:

- Acceptance criteria below are checked.
- A fresh clone can build/test/release without hidden local assets.

## Post-engine results table

| Artifact | Scenario/languages | New result | Delta vs baseline | Notes |
| --- | --- | ---: | ---: | --- |
| `target/post-engine-measure-fixtures.json` | syntax fixtures | TBD | TBD | Fill after cutover |
| `target/post-engine-measure-bun-rust-pr.json` | Bun PR integrated | TBD | TBD | Fill after cutover |
| `target/post-engine-raw-compare-bun-core.json` | Mark vs Shiki raw | TBD | TBD | Fill after cutover |
| `target/release/mark` | release binary size | TBD | TBD | Fill after cutover |
| `mark syntax list` | installed languages | TBD | TBD | Fill after cutover |

## Risk register

| Risk | Why it matters | Mitigation |
| --- | --- | --- |
| Regex capture extraction slows DFA path | TextMate scopes depend on captures; DFA search alone is not enough | Use DFA for candidate location, extract captures only for winners; benchmark capture-heavy grammars |
| Oniguruma compatibility gaps | Grammars assume Oniguruma semantics | Inventory all patterns, compare representative patterns with `vscode-oniguruma`, route rare constructs to fallback |
| Injection selectors are subtly wrong | Markdown/HTML/JSX fidelity depends on injections | Implement selector parser early; keep markdown/html in the required golden corpus |
| Begin-capture end substitutions explode cache | Dynamic end patterns can vary by input | Cache by capture tuple with size limits; budget pattern length and fallback steps |
| UTF-16 vs UTF-8 offsets hide oracle mismatches | VS Code APIs report JS offsets, Mark stores byte ranges | Centralize offset conversion and test with non-ASCII fixtures |
| Bundle generation becomes non-deterministic | Committed binary would churn | Stable sort every table, include source hash, assert regenerate-without-diff in CI/manual verification |
| Performance parity fails after deleting fast lexers | Existing UI latency relies on fast paths | Keep old/new comparison until performance layer is accepted; checkpoints/cache/prefilters are mandatory |
| Binary/RSS grows unexpectedly | Full grammar set is embedded | Lazy decode, compact tables, measure startup/RSS separately from total binary size |
| License metadata is incomplete | Embedded third-party grammars require traceability | Generate manifest from source metadata and fail compiler when license is missing |
| Fallback matcher becomes the hot path | TS/C++/Ruby hot rules use lookaround; DFA-only speedups never materialize | Hotness-weighted Phase 0 inventory with explicit go/no-go gate; Phase 2 fallback microbenchmarks must meet or beat onig on hot patterns |
| Coverage shrinks vs the 346 currently listed languages | tm-grammars has ~220 grammars; raw-count parity is impossible | Phase 0 keep/drop list agreed up front; acceptance defined against the list, drops are deliberate and documented |
| Anchor-context variants inflate bundle/DFA memory | Up to 4 compiled variants per anchor-sensitive pattern set | Phase 0 anchor census sizes the cost; Phase 2 chooses variant matrix vs rewrite vs fallback per pattern class |
| Markdown/HTML defeat lazy decode | Their injections/includes transitively pull in dozens of grammars on first open | Registry resolves external includes lazily at first use; cold-open markdown is a required Phase 4 measurement |

## Performance targets

Hard thresholds are commit gates for Phases 5 and 7; aspirations are what
good looks like. Relative thresholds are against this machine's Phase 0
baseline (prior-system reference numbers shown until captured). Values marked
(p) are provisional absolute guardrails set before implementation: confirm or
tighten them when the Phase 0 baseline lands. Loosening any hard threshold
requires a written justification in this file.

| Metric | Baseline | Hard threshold | Aspiration |
| --- | ---: | ---: | ---: |
| Integrated many-small-rust fixture | TBD (prior 36,390µs) | ≤1.05× baseline | ≤1.0× |
| Integrated large-rust fixture | TBD (prior 45,148µs) | ≤1.05× baseline | ≤1.0× |
| Integrated Bun PR workload | TBD | ≤1.05× baseline | ≤1.0× |
| TypeScript raw highlight | TBD (fallback 304,846µs, old fast path 36,791µs) | ≥5× vs syntect fallback | ≤3× old fast path |
| Previously syntect-only languages (yaml, markdown, bash) | TBD fallback numbers | ≥5× vs recorded fallback | ≥10× |
| Cold decode, single core grammar | n/a | ≤10ms (p) | ≤3ms |
| Cold-open markdown, transitive includes | n/a | ≤80ms total decode (p) | ≤25ms |
| `mark --help` startup delta | n/a | ≤+2ms (p) | 0 |
| Release binary size | 5.8M current | ≤12M (p) | ≤9M |
| RSS delta reviewing Bun PR vs baseline | TBD | ≤+15MB (p) | ≤+5MB |

## Acceptance criteria

- Every hard threshold in the Performance targets table passes (integrated
  fixtures, Bun PR workload, previously syntect-only languages, and the
  decode/startup/size/RSS guardrails)
- Golden-token harness passes on the full fixture corpus
- Zero languages highlighted by another language's rules
- `mark syntax list` matches the Phase 0 coverage keep list; every removed
  language is on the agreed drop list
- `syntect`, `two-face`, `onig`, `onig_sys` absent from `Cargo.lock`
- `cargo fmt` / `cargo check` / `cargo test --workspace` /
  `cargo build --release -p mark-cli` clean

## Verification

- [ ] `cargo fmt`
- [ ] `cargo check`
- [ ] `cargo test --workspace --quiet`
- [ ] `cargo build --release -p mark-cli`
- [ ] `cargo run --release -p mark-bench -- measure --fixtures target/bench-fixtures --syntax --syntax-language rust --syntax-language typescript --json > target/post-engine-measure-fixtures.json`
- [ ] `cargo run --release -p mark-bench -- measure-repo --repo ~/sandbox/bun-rust-pr --syntax-language rust --syntax-language typescript --json > target/post-engine-measure-bun-rust-pr.json`
- [ ] `cargo run --release -p mark-bench -- syntax-compare --repo ~/sandbox/bun-rust-pr --language rust --language typescript --language zig --language c --language cpp --shiki --json > target/post-engine-raw-compare-bun-core.json`
- [ ] `cargo tree -p mark-cli | rg -i 'syntect|two.face|onig'` returns nothing
- [ ] `rg -n 'syntect|two-face|two_face' Cargo.toml Cargo.lock` returns nothing; `rg -n '"onig"|"onig_sys"|name = "onig' Cargo.lock` returns nothing
- [ ] `rg -n 'mark-textmate|mark_textmate|syntect|two-face' Cargo.toml Cargo.lock crates README.md docs migration.md` reviewed; only intentional migration/history references remain (bare `onig` deliberately excluded — the engine's Oniguruma-syntax translator references it legitimately)
- [ ] No `mark-textmate` references remain in live Cargo/docs/crates/README
