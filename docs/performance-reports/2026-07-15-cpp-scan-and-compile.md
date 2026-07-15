# Engine optimization round 8 — 2026-07-15

Focus: C-family highlighting (cpp and its TextMate grammar shapes), with
universal candidate-scan and compile-path wins.

- Host: arm64 macOS, same reference machine as round 7; rustc 1.88.0.
- Method: `profile-cold` (line-cold for warm-matcher tokenize throughput,
  process-cold for fresh-tokenizer cost), 10 iterations, 3 reps where noted,
  mimalloc build. CPU attribution via `sample` on 400–600-iteration loops.
- **Cross-round caution**: the perf corpora were regenerated on 2026-07-14
  for the 264-ID catalog (markdown's embedded closure grew, cpp stress
  content changed), so round-7 absolute numbers are not comparable. All
  deltas below are same-day, same-corpus A/Bs.

## What changed

1. **Word-context start-class gate** (`engine/regex/start_class.rs`).
   Each pattern gets an AST-derived mask over the four (previous, current)
   word-character classes where a match can possibly start; the ordered
   reference scan computes one class bit per position and skips masked-out
   candidates. Kills mid-identifier anchored attempts for `(?<!\w)keyword`,
   `\b…`, and separator-prefixed rules. Masks are consulted only between
   ASCII neighbors, so the analysis needs ASCII-word soundness only.
   Env: `MARK_TEXTMATE_START_CLASS=off`.
2. **Skip-prefix gate** (`engine/regex/skip_prefix.rs`). Rules shaped
   `<comment-or-whitespace separator> <token>` (the dominant cpp grammar
   shape, recognized with the existing `is_cpp_space_comment_separator`, plus
   plain `\s*+` prefixes) precompute the token's first-byte set; the scan
   attempts them only when that byte appears at the position itself or at
   the end of the shared whitespace run, with `/*` comment paths gated by a
   per-line block-comment flag cached in `BytecodeScratch`.
   Env: `MARK_TEXTMATE_SKIP_GATE=off`.
3. **Atom-derived ASCII class bitmaps** (`engine/regex/bytecode.rs`).
   `CompiledClass` construction ran `class_contains` 128 × 2 times per class
   with Unicode case conversions per probe (`atom_contains` +
   `to_lower/to_upper` ≈ 15% of cpp process-cold samples); it now derives
   both bitmaps per atom with closed-form ASCII case handling and
   flag-invariant Perl/POSIX/Unicode passes. A `debug_assert` keeps the fast
   path byte-equal to the evaluation path on every compile in dev/test.
4. **Process-wide compiled-pattern cache** (`engine/regex/mod.rs`).
   Grammar-static patterns (keyed by spelling + live-capture request) share
   one `CompiledPattern` across tokenizer instances. Dynamic begin/end
   substitutions stay per-tokenizer. Env: `MARK_TEXTMATE_PATTERN_CACHE=off`.
5. **Arc-shared grammars + shared rule contexts** (`engine/tokenizer.rs`).
   `GrammarSet` stores `Arc<CompiledGrammar>` (set clones stop deep-copying
   every grammar), and the flattened injection selectors + rule repository
   contexts are cached process-wide by grammar identity. Every TUI syntax
   worker owns a `SyntaxHighlighter`, so per-language setup used to repeat
   the full closure flattening per worker. Env: `MARK_TEXTMATE_GRAMMAR_CACHE=off`.
6. `HighlightScopeTable::default()` no longer calls `getenv` per tokenized
   source (process-global lock in the samples).

`profile-cold --mode process-cold` now disables the two shared caches by
default so the strict fresh-tokenizer measurement semantics are preserved;
pass `MARK_TEXTMATE_PATTERN_CACHE=on MARK_TEXTMATE_GRAMMAR_CACHE=on` to
measure production warm-process semantics.

## Results (plain release + mimalloc, same corpus, before → after)

Line-cold (matcher-warm tokenize throughput, MB/s):

| Corpus | Before | After | Delta |
| --- | ---: | ---: | ---: |
| libc++ `<string>` (real-world C++) | 2.87 | 3.43 | +19% |
| cpp repeated stress | 14.7 | 16.2 | +10% |
| markdown stress | 24.7 | 28.1 | +14% |
| php stress | 5.4 | 5.7 | +5% |
| nix stress | 7.8 | 8.05 | +3% |
| rust / ts / c | — | — | neutral to +3% |

Process-cold (fresh tokenizer per iteration, strict, MB/s):

| Corpus | Before | After | Delta |
| --- | ---: | ---: | ---: |
| cpp repeated stress | 5.23 | 6.12 | +17% |
| libc++ `<string>` | 2.39 | 2.86 | +20% |

Production warm-process semantics (shared caches on):

- cpp stress process-cold: **11.5 MB/s** (2.2× the old fresh-tokenizer cost);
  an additional cpp tokenizer in a warm process costs ~0.3 ms instead of
  ~3.5 ms (tiny-file probe), so parallel syntax workers stop paying the
  per-worker grammar/regex compile.
- `mark-syntax` test suite wall time dropped 7.5 s → 5.0 s (golden target)
  and 1.8 s → 0.3 s (lib target) as a side effect.

PGO (`scripts/build-pgo` + profile-use example build) on top: libc++ 3.61,
cpp stress line-cold 18.4, nix 8.64, markdown line-cold 35.1, rust 89.9.

## Correctness

- Token streams byte-identical on every measured corpus (equal token counts
  throughout; goldens exact).
- `cargo test -p mark-syntax --release` green (331 lib + 21 golden + 7);
  full workspace release tests green.
- Debug-assert cross-checks (mask fast path vs evaluation) exercised via
  debug-mode lib tests and one debug golden shard.
- `node tools/generate-goldens.mjs --check` clean.
- `python3 tools/check-textmate-catalog-performance.py` passes (all 264
  floors met).
- New differential unit tests: gated vs ungated `PatternSetMatcher`
  selection equality over adversarial pattern/text grids; skip-gate and
  start-class analysis unit tests (including `\h` = hex digit and
  `[[:punct:]]` containing `_`).

## CPU attribution notes (next round)

- libc++ warm tokenize is still ~55% bytecode VM (`execute_inner` +
  `backtrack_or_resolve`), but the remaining executions are mostly winners
  and heavy matching patterns (the cpp scope-resolution rule with its
  ~50-keyword negative lookahead), not wasted anchored attempts. Next lever
  is making those executions cheaper, not fewer.
- Strict process-cold cpp is now ~27% tokenize / the rest compile: AST
  parse + clone churn (`parse_concat`, `Ast::clone`, alloc ≈ 25%),
  `flatten_refs`/context building (8%), literal tries (6%). The shared
  caches make this a first-tokenizer-only cost in production.
- Frontier-forced runs (`MARK_TEXTMATE_FRONTIER=on`) remain slower than the
  bucketed reference scan on libc++/php (2.9 → 2.0, 5.4 → 1.9 MB/s):
  the E4 "lazy ordered frontier" direction stays unattractive versus
  narrowing the reference scan, which is what this round did.
