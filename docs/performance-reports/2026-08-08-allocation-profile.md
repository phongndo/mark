# Allocation and model-reuse report — 2026-08-08

- Commit: working tree on top of `75337e8`
- Host: arm64 macOS 26.5.2, 64 GiB RAM
- Rust/Cargo: 1.97.0
- Raw artifacts: `target/allocation-profile/` (ignored)
- Allocation protocol: release `mark-bench` with the opt-in
  `allocation-profile` feature, one process per fixture command. The profiling
  allocator wraps the shipped mimalloc allocator and records process-wide
  allocation/reallocation counts and bytes around each benchmark stage.
- Latency protocol: ordinary release allocator, 5–9 in-process samples after a
  separate release rebuild. Atomic allocation counters are never used for the
  latency comparison.

## Retained changes

1. **Stage-level allocation profiling.** `mark-bench` can now report allocation
   calls, reallocations, byte churn, retained-byte deltas, and peak live-byte
   growth for parse/load, model open, filters, initial render, and scroll
   passes. The feature is opt-in so normal benchmark and production allocator
   paths remain unchanged.
2. **Single-owner render assembly.** Split/unified cells append gutters and
   content directly into a span vector sized from syntax and inline-range
   counts, avoiding geometric growth for heavily styled lines. Generated gutter
   strings move into their span and reserve a sign byte only when needed;
   ordinary ASCII padding allocates its final capacity once; and statusline
   spans move owned text or borrow static labels when no truncation is needed.
   This removes temporary per-cell vectors and strings without changing rendered
   ownership.
3. **Reusable inline emphasis.** Valid sorted ranges borrow cached storage during
   validation, including allocation-free grapheme checks for Unicode. Nonempty
   range arrays are shared across frames, while empty ranges remain allocation-
   free. Invalid external ranges still take the normalizing owned fallback.
4. **Flat compositor storage and lazy overlay planning.** The per-frame
   compositor stores small rectangle components inline instead of allocating a
   trait object for each layer. Choice/item vectors are built only for an open
   overlay; closed menus no longer contribute allocations to every diff frame.
5. **Allocation-free split-run modeling.** Eager and sparse split models now
   summarize each deletion/addition run in one scan. Contiguous runs derive row
   indexes arithmetically; interleaved fallback runs use iterators. The previous
   design allocated two temporary vectors per changed run, including tens of
   thousands of vectors in generated mega diffs.
6. **Filter model reuse.** Applying a filter whose visible file set is unchanged
   reuses the current hunk-view model. Full-file mode still rebuilds because its
   context preparation can change rows. Cursor-mode and hunk-focus side effects
   are preserved on the reuse path.
7. **Right-sized sparse split models.** Before building a sparse split model, a
   no-allocation shape pass counts its row segments. The segment vector then
   allocates once near its final size instead of repeatedly doubling from a
   file/hunk-only estimate. Pathological one-run hunks still reserve one segment
   rather than an upper bound proportional to their row count.

## Allocation results

`ops` is allocation calls plus successful reallocations. `Churn` is the total
requested bytes over those operations, not RSS. Peak is live bytes above the
stage baseline as observed by the counting allocator.

| Fixture / pass | Before ops | After ops | Change | Before churn | After churn | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Balanced, complete run (1.36 MB patch, 200 scrolls/pass) | 941,435 | 526,873 | **-44.0%** | 150.4 MB | 109.7 MB | **-27.0%** |
| Mega 100k, complete run (7.31 MB patch, 100 scrolls/pass) | 492,785 | 171,343 | **-65.2%** | 83.0 MB | 62.5 MB | **-24.7%** |
| Balanced, three scroll passes | 929,286 | 519,962 | **-44.0%** | 140.6 MB | 102.1 MB | **-27.4%** |
| Mega 100k, three scroll passes | 328,223 | 167,383 | **-49.0%** | 43.7 MB | 32.2 MB | **-26.3%** |
| Mega 100k, open model | 42,525 | 2,510 | **-94.1%** | 8.10 MB | 7.46 MB | -7.9% |
| Mega 100k, unchanged file-filter apply + reset | 80,098 | 39 | **-99.95%** | 7.70 MB | 9 KB | **-99.9%** |
| Mega 1M, sparse open model | 4,275 | 4,266 | -0.2% | 90.5 MB | 56.4 MB | **-37.7%** |
| Mega 1M, clear grep / rebuild sparse model | 46 | 37 | -19.6% | 50.2 MB | 16.1 MB | **-67.9%** |

The 100k peak-live measurement remained flat because retained changeset/model
storage dominates that run; balanced peak growth fell from 6.61 MB to 6.13 MB.
On the 1M tier,
model reuse and right-sized sparse storage lowered measured peak growth from
204.6 MB to 171.5 MB (-16.2%). The final instrumented 1M run completed 46,163
allocation/reallocation operations with 236.3 MB of cumulative churn across
load, filtering, and all render passes.

## Ordinary release latency and RSS

| Fixture | Metric | Before | After | Change |
| --- | --- | ---: | ---: | ---: |
| Balanced | open p50 | 6.16 ms | 5.48 ms | -11.0% |
| Balanced | unchanged file-filter apply | 76 µs | 5 µs | **-93.4%** |
| Balanced | warm scroll total, 200 frames | 44.23 ms | 43.49 ms | -1.7% |
| Balanced | random scroll total, 200 frames | 53.46 ms | 52.52 ms | -1.8% |
| Mega 100k | open p50 | 9.55 ms | 9.15 ms | -4.2% |
| Mega 100k | unchanged file-filter apply | 564 µs | 4 µs | **-99.3%** |
| Mega 100k | warm scroll total, 100 frames | 20.22 ms | 19.88 ms | -1.7% |
| Mega 100k | random scroll total, 100 frames | 24.63 ms | 23.97 ms | -2.7% |

The scroll latency changes are small but consistently non-regressing. Shared
inline arrays remain bounded by the existing hunk LRU rather than adding an
unbounded render cache. The retained user-visible latency win is model reuse
when a filter maps to the existing file set. Follow-up alternating-binary A/B
runs kept the
shared inline storage and exact span sizing only after they showed no consistent
balanced-diff regression and a 2–4% improvement across the 100k scroll passes.
An unconditional gutter over-reservation candidate was discarded because it
reduced calls while increasing requested bytes; the retained version reserves
its extra byte only for rows that append a blank sign.

The final ordinary-release 1M gate measured a 6.42 ms load p50, 11.89 ms open
p50, 274 µs random-scroll max p50, and 59.0 MB maximum RSS delta (0.80x the
74.0 MB patch), passing the existing 3x RSS gate. The sparse capacity pass adds
about 1.7 ms to 1M open time while reducing the post-load RSS delta by 42.8%
(103.2 MB to 59.0 MB).

The 10M release tier measured an 89.4 ms load p50, 85.4 ms open p50, 32.2 ms
grep p50, 361 µs random-scroll max p50, and 564.9 MB maximum post-load RSS
delta for a 750.3 MB patch. This remains comfortably inside the existing open,
search, scroll, and 3x RSS gates.

## Correctness and interpretation

- Split model parity remains covered for eager/sparse layouts; a new test covers
  deliberately interleaved deletion/addition runs and the explicit sparse
  fallback.
- A new filter test verifies that an unchanged visible-file set preserves model
  identity.
- The complete `mark-tui` library suite passed after the rendering and model
  changes.
- Allocation stage counters are process-wide. Syntax workers that overlap a
  stage are intentionally included, so syntax-stage counts can vary with worker
  scheduling. Use these counters to locate ownership churn, not as call-stack or
  latency data.

## Decisions and follow-ups

- Keep owned ratatui frame output. Borrowing source text through the complete
  mutable app/render path would add lifetime coupling for a path already below
  a millisecond per frame.
- Keep the sparse/eager model thresholds and span-backed patch representation.
- Do not cache every filtered mega-diff model: avoiding a rebuild by retaining
  another 16+ MB model would trade predictable memory for a less common clear-
  filter transition. Reuse only when the current model is already valid.
- Future allocation work should target syntaxmate/worker setup separately;
  concurrent syntax allocation attribution is materially different from the
  deterministic plain-render path.
