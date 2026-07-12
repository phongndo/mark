# Engine optimization round 7 — 2026-07-12

- Commits: `7bd00e6` (tokenizer state maintenance), `36ad492` (injection
  outcome memoization), plus mimalloc adoption and `scripts/build-pgo` in the
  working tree on top of `c6a176b`.
- Host: arm64 macOS 26.5.2, 16 physical cores, 64 GiB RAM; rustc 1.88.0.
- Method: `profile-cold --mode process-cold`, separate process per
  measurement, 3 reps, per §7 of `docs/performance-plan.md`. CPU attribution
  via `sample` on 300–1200-iteration loops.
- Raw artifacts: `target/textmate-performance/reports/2026-07-12-e-phase/`.

## What changed

1. **E1 — tokenizer state maintenance** (`7bd00e6`). Frame stacks are now
   parent-linked single-frame nodes (push = 1 node alloc, pop = parent step,
   zero frame clones even when shared); grammars without `while` patterns
   skip the per-line continuation walk via a cumulative while-frame count
   (nix stacks run ~1,000 frames deep — the walk was O(depth)/line);
   candidates share `Arc`'d capture specs/pattern lists; fully static frames
   cache their interned identity so repeat pushes skip capture substitution,
   string hashing, and the global intern-table mutex; per-tokenizer edge and
   node caches reuse whole immutable frame nodes for repeated transitions;
   engine-internal maps moved off SipHash; the state interner keys on
   interned frame-stack ids.
2. **Injection outcome memoization** (`36ad492`). Injection selector
   evaluation (a per-selector expression interpreter over resolved scope
   strings) ran on every candidate-cache miss; it is a pure function of the
   interned scope stack id and is now cached per id (php: ~13% of tokenize
   time plus a scope-stack string resolution per miss).
3. **mimalloc as the global allocator** for `mark` and `mark-bench`.
   Allocator traffic was the #1 CPU cost of the cold tokenize path (~20–27%
   of samples).
4. **Profile-guided release builds** (`scripts/build-pgo`): instrument →
   train on committed corpora + bench fixtures → merge → rebuild. PGO must be
   trained on the same allocator configuration it ships with; a profile
   trained against the system allocator loses most of its effect on the
   mimalloc build (measured: large-rust 15.6 → 12.0 MB/s with a stale
   profile).

## Engine gate results (process-cold MB/s)

"Release" = plain `cargo build --release` at `36ad492` + mimalloc;
"PGO" = same code built by the `scripts/build-pgo` recipe.

| Corpus | Gate | Round 6 | Release | PGO | Gate status |
| --- | ---: | ---: | ---: | ---: | --- |
| nix repeated | ≥ 8 | 2.2 | 4.9 | **7.0** | ✗ (3.2× improved, 12% short) |
| php repeated | ≥ 6 | 2.7 | 4.1 | **5.8** | ✗ (2.1× improved, 3% short) |
| cpp repeated | ≥ 15 | 10.6 | 11.4 | **16.7** | ✅ |
| markdown | ≥ 15 | 9.8 | 16.5 | **21.8** | ✅ |
| typescript (cache on) | — | 18.0 | 18.9 | **24.6** | — |
| TS cache-off | ≥ 4 | 2.09 | — | **3.8–3.9** | ✗ (1.8× improved, borderline) |
| large-rust | ≥ 12 | 10.05 | 11.6 | **17.0** | ✅ |
| libc++ `<string>` | ≥ 2.5 | 2.32 | 2.4 | **3.17** | ✅ |

Correctness: token streams byte-identical throughout (same token counts per
corpus, `cargo test -p mark-syntax --release` green — 258 + 5 + 9 including
`textmate_golden`; `node tools/generate-goldens.mjs --check` clean, 53
fixtures ok).

## Whole-binary results (mimalloc, plain release, 10M mega-diff tier)

| Metric | Runtime-integration report | Now | Gate |
| --- | ---: | ---: | --- |
| load (parse) | 89.3 ms | 95 ms | ≤ 1 s ✅ |
| open model | 121.7 ms | **82 ms** | ✅ |
| first grep | 30.5 ms | 30.8 ms | ≤ 250 ms ✅ |
| peak RSS delta / patch | 1.31× | 1.43× | ≤ 3× ✅ |
| startup (adjusted median) | 1.24 ms | 1.31 ms | ≤ 2 ms ✅ |
| non-TTY 750 MB stream | 8 MB RSS | 8.5 MB RSS | O(buffer) ✅ |

mimalloc trades +9% RSS on the 10M tier for −37% open time and the engine
gains above; the RSS gate retains >2× headroom.

## CPU attribution notes (for the next round)

- nix (7.0 vs 8): remaining time is the bytecode VM (`Program::execute_inner`),
  `PatternSetMatcher` scan, backtrack fallback, and residual allocation in
  `apply_candidate`/line-cache inserts. The frame-stack identity cost that
  motivated E1 is no longer visible in samples.
- php (5.8 vs 6): `PatternSetMatcher::find_with_context_and_scratch` self
  time is ~33% — per-position bucket iteration over start-byte-unrestricted
  patterns. This is E4 (lazy ordered frontier) territory; note the
  reverted-experiment blacklist in `textmate-engine.md` explicitly rules out
  naive per-candidate memoization, so any next step must be the
  frontier/scanner design, not result caching.
- cpp small-corpus numbers are dominated by one-shot grammar/regex
  compilation (`parse_concat`, `intern_literal_trie`), not tokenization —
  compile-side work is the lever there, and PGO already recovers most of it.

## Decisions

- Adopt mimalloc for `mark`/`mark-bench` (this tree).
- Adopt `scripts/build-pgo` for release binaries; profiles must be retrained
  whenever the allocator or engine code changes materially.
- `alloc-trial` feature on `mark-syntax` retained for future allocator A/Bs
  of the profiling examples.
- E2/E3 as originally scoped are superseded: their gates (cpp, large-rust,
  libc++) pass via E1 + build-level work. Remaining engine work is
  E4-class (php/nix/TS-cache-off) and compile-cost reduction (cpp/libc++
  first-highlight latency).
