# In-house TextMate engine migration

This file tracks the hard migration from the syntect/two-face hybrid (grammar
fallback + hand-rolled fast-path lexers) to a single in-house TextMate engine
living entirely in `crates/mark-syntax`.

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

## Locked decisions

1. **Regex strategy: hybrid.** Grammar patterns are translated offline; the
   majority compile to `regex-automata` (lazy DFA, no backtracking blowups).
   Patterns needing lookaround/backreferences/`\G`-dependent behavior fall back
   to a small in-house backtracking matcher with a hard step budget. This is
   the primary performance lever versus both Shiki and onig.
2. **Language coverage is an asset problem, not an engine problem.** Grammars
   come from the `tm-grammars` collection (Shiki's source, MIT with per-grammar
   licenses tracked), compiled offline and embedded in the CLI. Coverage grows
   by adding assets, never by mapping a language onto another language's lexer.
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

## Target architecture

```
crates/mark-syntax/src/
  engine/
    grammar.rs     # rule model: match/begin/end/while, captures, repository,
                   # includes ($self/$base/#name), injections
    regex/
      translate.rs # oniguruma-syntax -> regex-automata translation + feature
                   # detection (lookaround, backrefs, \G, \h, possessive)
      dfa.rs       # regex-automata wrapper, anchored/unanchored search
      backtrack.rs # fallback matcher with step budget
      prefilter.rs # required-literal extraction, memchr/Aho-Corasick line skip
    tokenizer.rs   # line-oriented begin/end stack machine, first-match-wins,
                   # capture scope application, while-rule continuation
    state.rs       # interned/hash-consed parser state (StateId), checkpoints
    scopes.rs      # scope interning + scope -> SyntaxClass classifier
                   # (ported from mark-textmate's classify_scope_text)
  grammars/
    registry.rs    # lazy per-language decode of the embedded bundle
    bundle.bin     # compiled grammar bundle (generated, committed)
  detect.rs        # canonical names, aliases, extensions, basenames
                   # (ported from mark-textmate catalog tables)
  highlight.rs     # public API (existing shapes preserved)
  language.rs, paths.rs, storage.rs, types.rs   # existing config/settings mgmt
tools (mark-bench subcommand or scripts/):
  grammar-compile  # tm-grammars JSON -> validated -> translated -> bundle.bin
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

`mark-bench` retargets its `mark-textmate` dependency to `mark-syntax`.

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

## Baseline

- [ ] Re-capture current-system benchmarks on this machine before deleting
      anything: `mark-bench measure` syntax fixtures (Rust, TypeScript) and
      the integrated `~/sandbox/bun-rust-pr` workload, saved to
      `target/pre-engine-*.json`
- [ ] Capture raw `syntax-compare --shiki` artifacts for rust, typescript,
      zig, c/cpp on the Bun corpus: `target/pre-engine-raw-compare-*.json`
- [ ] Record current release binary size and `mark syntax list` count

Prior-system reference numbers (from the syntect migration): integrated
many-small-rust 36,390µs / large-rust 45,148µs; raw Mark-vs-Shiki speedups
111–162x on fast-path languages; binary 5.8M; 346/346 languages listed.
The new engine must not regress the integrated numbers and must beat the
*syntect fallback* numbers (e.g. TypeScript fallback 304,846µs vs fast path
36,791µs) with full-grammar fidelity.

## Correctness gate

The old system never had a token-level oracle. Add one:

- [ ] Golden-token harness: a dev-only Node script runs `vscode-textmate` +
      `vscode-oniguruma` over a fixture corpus and dumps per-line scope spans;
      a Rust test asserts our tokenizer produces matching scopes (modulo
      documented, enumerated divergences)
- [ ] Fixture corpus: at minimum rust, typescript, tsx, javascript, json,
      yaml, toml, markdown, html, css, python, go, c, cpp, bash, zig —
      markdown/html exercise injections and embedded grammars hardest
- [ ] Fuzz/property tests: tokenizer never panics, never loops (step budget),
      segments are monotonic, char-boundary-aligned, and cover each line
- [ ] Backtracker step-budget kill switch test on pathological patterns

## Implementation phases

### Phase 1 — engine core (in `mark-syntax::engine`, behind the old backend)

- [ ] Grammar model: deserialize raw tmLanguage JSON (dev path) including
      repository, includes, injections, while-rules, capture sub-patterns
- [ ] Regex translation + feature detection; classify every pattern in the
      target grammar set as DFA-able or fallback; emit stats
- [ ] Backtracking fallback matcher with `\G`, lookaround, backrefs
      (including begin-capture substitution into end/while patterns), step
      budget
- [ ] Tokenizer stack machine passing the golden harness on the core corpus
- [ ] Scope interning + `SyntaxClass` classifier ported from
      `classify_scope_text` (Tag/Attribute priority behavior preserved)

### Phase 2 — grammar pipeline + bundle

- [ ] `grammar-compile` tool: fetch/vendor tm-grammars, validate, pre-translate
      patterns, intern scopes, emit compact binary bundle + license manifest
- [ ] Compile-time rejection report for unsupported patterns (fix or document
      per grammar; no silent runtime failures)
- [ ] Lazy per-language registry decode; measure cold decode cost per grammar
- [ ] Rebuild detection catalog (`LANGUAGE_ALIASES`, `EXTENSION_ALIASES`,
      `BASENAME_ALIASES`, code-fence tokens) against bundle metadata; port the
      curated override tables from `mark-textmate` (e.g. `*.v` → verilog while
      `v` → v; `*.h` → c; jsx/tsx react grammars)
- [ ] Embed bundle; verify binary size and startup cost

### Phase 3 — performance layer

- [ ] Interned parser state + `(StateId, fingerprint)` line cache integration
- [ ] Line-state checkpoints with resume-from-nearest for viewport-first work
- [ ] Per-state candidate rule lists + leftmost single-pass matching
- [ ] Literal prefilters
- [ ] Per-line budget + plain-text degradation (minified guard parity)
- [ ] Benchmark each optimization with before/after artifacts in `target/`;
      scrap anything that doesn't pay for itself (same discipline as before)

### Phase 4 — hard cutover

- [ ] `SyntaxHighlighter` in `types.rs` switches to the in-house engine
- [ ] Delete all fast-path lexers (Rust, C-like, CompilerIr, LispLike, Markup)
      and `FAST_ONLY_LANGUAGES`
- [ ] Move surviving `mark-textmate` code (API types, classifier, catalog)
      into `mark-syntax`; delete `crates/mark-textmate`
- [ ] Retarget `mark-bench` to `mark-syntax`
- [ ] Remove `syntect` and `two-face` from workspace `Cargo.toml`; verify
      `onig`/`onig_sys` gone from `Cargo.lock` and `cargo tree -p mark-cli`
- [ ] Update `mark syntax doctor`/status wording that references bundled
      grammar internals; `TEXTMATE_BUNDLE_VERSION` sourced from the bundle

### Phase 5 — measurement + docs

- [ ] Re-run integrated `mark-bench measure` fixtures and the Bun PR workload;
      record tables here alongside the pre-engine baseline
- [ ] Re-run `syntax-compare --shiki` for rust, typescript, zig, c/cpp, and at
      least three previously-syntect-only languages (e.g. yaml, markdown,
      bash) to prove the fallback-class languages improved
- [ ] Record binary size, RSS deltas, `mark syntax list` count, and Shiki
      language-ID audit result
- [ ] Update README/docs syntax sections

## Acceptance criteria

- Integrated benchmark: no regression vs pre-engine baseline on rust/ts
  fixtures and the Bun PR workload (fast-path parity or better, now with real
  grammar fidelity)
- Previously syntect-only languages: ≥5x faster than the recorded syntect
  fallback numbers on the same fixtures
- Golden-token harness passes on the full fixture corpus
- Zero languages highlighted by another language's rules
- `syntect`, `two-face`, `onig`, `onig_sys` absent from `Cargo.lock`
- `cargo fmt` / `cargo check` / `cargo test --workspace` /
  `cargo build --release -p mark-cli` clean

## Verification

- [ ] `cargo fmt`
- [ ] `cargo check`
- [ ] `cargo test --workspace --quiet`
- [ ] `cargo build --release -p mark-cli`
- [ ] `cargo run --release -p mark-bench -- measure ...` (fixtures + Bun PR)
- [ ] `cargo run --release -p mark-bench -- syntax-compare --shiki ...`
- [ ] `cargo tree -p mark-cli | rg -i 'syntect|two.face|onig'` returns nothing
- [ ] No `mark-textmate` references remain in Cargo/docs/crates/README
