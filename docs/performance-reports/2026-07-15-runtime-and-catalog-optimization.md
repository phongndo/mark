# Runtime and catalog optimization round 9 — 2026-07-15

Focus: end-to-end responsiveness and memory, with catalog-wide syntax
highlighting as the primary gate.

- Commit: working tree on top of `f9a1b52`.
- Host: arm64 macOS, 16 logical CPUs, 64 GiB RAM; rustc/cargo 1.88.0.
- Raw artifacts: `target/perf-research/` (ignored).
- Syntax protocol: release `profile-cold`; catalog process-cold sweep over all
  264 committed stress corpora; five-rep line-cold medians for focused Rust and
  Fennel checks. Process-cold keeps the shared grammar/pattern caches disabled.
- TUI protocol: release `mark-bench measure`, `syntax-many-small-rust`, 160x40
  viewport, 200 scroll positions, five baseline and seven final samples.

## What changed

1. **Result construction no longer allocates a scope table per line.** A single
   process-wide empty table is used while a result is assembled and replaced
   by its final table. The old `Arc::default()` path allocated an outer table,
   stack storage, and style storage for every source line, then immediately
   discarded all of them.
2. **Ordered scanner hot path.** Regular candidate sets now index start-byte
   hints with bitmaps and compact ordered buckets instead of linearly testing
   every entry at every byte. Direct consuming transitions bypass the epsilon
   work stack; the small wrapper is inlined; epsilon work stores only program
   counters rather than complete thread records. This preserves exact
   first-match priority and capture replay while materially helping languages
   whose grammars stay in the regular scanner.
3. **Compact candidate-set storage.** The 256 retained `Vec` headers per
   `PatternSetMatcher` (6 KiB before any indexes) became a 1,028-byte offset
   table plus one contiguous index slice. Candidate blueprints and pattern sets
   now share one `Arc` matcher slice instead of cloning matcher arrays and two
   copies of translated pattern text per state.
4. **Catalog lookup is indexed.** Canonical IDs/aliases, basenames, extensions,
   and compound suffixes are built once. Path detection now examines the file's
   own dot suffixes rather than scanning all 264 language records. A parity test
   checks every public ID, alias, extension, basename, and compound basename
   against the bundle's reference lookup.
5. **Scope data is shared.** Scope names use one `Arc<str>` allocation across
   the interner and exported tables. Equal result scope tables are weak-interned
   per tokenizer, and theme cache entries are shared with the interned table.
6. **Warm rendering avoids deep copies and locks.** The TUI cache stores an
   `Arc<HighlightedSide>` and returns an owned line guard, replacing a deep
   `Vec<SyntaxSegment>` clone for every visible line and frame. Resolved theme
   cache hits use generation-validated atomic reads instead of taking an
   `RwLock`; only generation installation and miss publication lock. ASCII
   engine output skips a redundant second structural segment walk in release
   builds while debug builds retain the full invariant assertion.
7. **Worker setup is shared.** A per-language `OnceLock` shares parsed grammar
   closures across syntax workers without serializing unrelated languages.
   Closure discovery also reuses its decoded bytes for grammar loading instead
   of inflating each blob twice.
8. **Viewport and source paths do less work.** Viewport highlighting streams
   `LineChunks` instead of collecting the full source twice, checkpoint lookup
   is binary rather than reverse-linear, valid UTF-8 file buffers become
   `String` without a copy, and source validation and line counting share one
   pass.

Rejected trials included delayed line-cache admission, a pending-line output
representation, and copy-on-write wrappers around the small `GrammarSet`
indexes. The last trial reduced a three-sweep catalog median from 15.963 to
15.630 MB/s despite cheaper clones, so only the high-value compiled grammar
payload remains `Arc`-shared.

## Syntax results

Catalog process-cold throughput (MB/s):

| Metric | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Aggregate | 15.297 | 15.963 median (15.963 / 16.013 / 15.932) | **+4.4%** |
| Median per-language speedup | — | 1.086x | **+8.6%** |
| Geometric-mean per-language speedup | — | 1.102x | **+10.2%** |
| Languages at least 10% faster | — | 116 / 264 | — |
| Languages at least 20% faster | — | 43 / 264 | — |
| Languages below the 2 MB/s floor | 0 | 0 | pass |

Emacs Lisp was effectively flat in the final sweep (2.230 MB/s median versus
the original 2.244 MB/s sample) and is a high-variance ~40 ms case. A separate
15-process check measured 2.282 MB/s median, so there is no reproduced
regression.

Focused warm-matcher line-cold medians:

| Corpus | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Rust stress | 79.19 MB/s | 98.58 MB/s | **+24.5%** |
| Fennel stress (scanner pass) | 3.798 MB/s | 4.782 MB/s | **+25.9%** |

The largest catalog process-cold gains included asm (2.20x), ignore (1.68x),
JSSM (1.56x), Fennel (1.45x), Rego and Ada (1.41x), and PL/SQL (1.39x).
Fallback/VM-heavy PHP, C++, MDX, AsciiDoc, and Objective-C++ remain the main
throughput tail and were approximately neutral in this scanner-focused pass.

## TUI and memory results

Default-theme synthetic syntax run:

| Metric | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Initial render | 275 µs | 131 µs | **-52%** |
| Cold scroll total (200) | 26.99 ms | 19.04 ms | **-29%** |
| Warm scroll total (200) | 11.43 ms | 10.21 ms | **-11%** |
| Random scroll total (200) | 15.21 ms | 12.19 ms | **-20%** |
| Random-scroll max p50 | 380 µs | 153 µs | **-60%** |
| Counted scope-table storage | 639,564 B | 5,352 B | **-99.2%** |
| Max sampled RSS delta | 27.6 MB | 25.8 MB | -6.5% (noisy) |

The catalog index produced most of the cold/random-scroll tail reduction.
Scope-table interning reduced 478 hunk-side results to the small set of unique
tables they actually used. Cache diagnostics now deduplicate shared table
pointers rather than counting the same allocation once per result.

On an exact GitHub Dark theme run, the isolated lock-free style-cache plus
ASCII validation changes reduced warm scroll 12.897 ms -> 11.961 ms (-7.3%)
and random scroll 16.772 ms -> 15.922 ms (-5.1%), with byte-identical rendered
text and style tests green.

## Correctness and gates

- `cargo test -p mark-syntax --release --locked`: green (334 unit, 5 capture
  quality, 21 TextMate golden, 2 theme golden).
- All 264 catalog corpora remain above the 2 MB/s floor.
- Exact TextMate goldens remain byte-for-byte green with zero degradation.
- `cargo test -p mark-tui --all-targets --all-features --locked`: 504 green.
- Existing concurrent-theme cache test passes with lock-free warm reads.
- New tests cover scanner start-bucket priority, indexed catalog parity, and
  reused output-table identity.

## Follow-ups

- Profile the fallback/bytecode tail (PHP, C++, MDX, AsciiDoc,
  Objective-C++) independently; scanner work should not be used to hide those
  remaining costs.
- Add explicit engine-cache byte accounting. The compact candidate indexes
  remove at least ~5 KiB per retained multi-pattern set, but current public
  diagnostics report output/cache memory rather than all tokenizer internals.
- Keep full-file syntax caps. Line-cache entries still retain source text for
  collision-safe equality and are entry-bounded rather than byte-bounded.
