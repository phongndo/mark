# Sticky hunk rendering optimization — 2026-08-23

- Commit: working tree on top of `e678e6a`.
- Host: arm64 macOS 26.5.2, 64 GiB RAM.
- Rust/Cargo: 1.98.0.
- Raw artifacts: `target/perf-current/` (ignored).
- Latency protocol: separate ordinary-release binaries, 160x40 viewport,
  alternating process order, 20 pairs for balanced/100k and 20 pairs for each
  one-million-line hunk case. Atomic allocation counters were not enabled.
- Allocation protocol: one release run per fixture with the opt-in profiling
  allocator; 200 balanced and 100 mega-100k positions per scroll pass.

## Finding

Pinned hunk headers introduced two forms of viewport-unbounded repeated work:

1. The overlay recomputed the focused hunk and separately materialized another
   complete viewport plan after the normal viewport renderer had already done
   both operations.
2. Rendering a hunk header counted additions/deletions, searched for a fallback
   display context, and materialized the complete context before fitting it to
   the terminal. Pinning repeated that work every frame. A synthetic
   million-line hunk therefore inspected one to two million model lines per
   scroll frame, and a long fallback line added work proportional to its full
   byte length, even though the rest of rendering remained viewport-bounded.

A normal-frame CPU sample remains dominated by Ratatui paragraph grapheme
segmentation and buffer diffing. The new sticky-header work was small on
ordinary hunks, but became the dominant Mark-controlled cost on one huge hunk.

## Retained changes

- Compute viewport hunk focus once and share it with the normal row renderer and
  sticky overlay. The overlay no longer allocates a second focus plan or a
  redundant plan solely to inspect its first slot; both wrapped and unwrapped
  builders emit a diff line before any following annotation block.
- While `DiffSearchIndex` is already walking line payloads, summarize deltas
  and the fallback-context line location for hunks with at least 16,384 lines.
  Store only those sparse summaries. Ordinary hunks keep the existing direct,
  allocation-free path, avoiding a per-hunk cache tax.
- Render both in-flow and pinned headers from the cached large-hunk summary.
  Meta lines remain excluded exactly as before. Cache tests cover large, small,
  meta-line, and all-blank hunk cases.
- Borrow the selected context and let the existing terminal-safe width fitter
  materialize only the visible prefix. This removes full-line copies and keeps
  header work bounded by terminal width even when the fallback line is huge.

## Latency results

Medians from the final alternating-order run:

| Fixture / pass | Before | After | Change |
| --- | ---: | ---: | ---: |
| Balanced cold scroll, 200 frames | 51.152 ms | 51.249 ms | +0.2% |
| Balanced warm scroll, 200 frames | 42.313 ms | 42.569 ms | +0.6% |
| Balanced random scroll, 200 frames | 51.017 ms | 51.161 ms | +0.3% |
| Mega-100k cold scroll, 100 frames | 21.792 ms | 21.806 ms | +0.1% |
| Mega-100k warm scroll, 100 frames | 20.625 ms | 20.477 ms | -0.7% |
| Mega-100k random scroll, 100 frames | 24.728 ms | 24.712 ms | -0.1% |
| One-million-line hunk initial render | 0.846 ms | 0.298 ms | **-64.8%** |
| One-million-line hunk cold scroll, 20 frames | 14.966 ms | 4.267 ms | **-71.5%** |
| One-million-line hunk warm scroll, 20 frames | 14.780 ms | 4.052 ms | **-72.6%** |
| One-million-line hunk random scroll, 20 frames | 15.099 ms | 4.842 ms | **-67.9%** |
| One-million-line hunk cold-frame max | 0.776 ms | 0.227 ms | **-70.7%** |
| Blank million-line hunk initial render | 3.289 ms | 0.305 ms | **-90.7%** |
| Blank million-line hunk warm scroll, 20 frames | 64.241 ms | 4.495 ms | **-93.0%** |
| Blank million-line hunk warm-frame max | 3.273 ms | 0.241 ms | **-92.7%** |
| Long-context million-line hunk initial render | 1.407 ms | 0.579 ms | **-58.9%** |
| Long-context million-line hunk warm scroll, 20 frames | 21.272 ms | 5.096 ms | **-76.0%** |
| Long-context million-line hunk random scroll, 20 frames | 21.051 ms | 5.443 ms | **-74.2%** |

Precomputing the ordinary million-line summary moves about 0.30 ms into model
open (13.448 -> 13.745 ms, +2.2%), while open plus first render improves
(14.294 -> 14.043 ms, -1.8%). On the blank hunk, a 2.58 ms open shift is also
recovered before the first frame (18.177 -> 17.769 ms, -2.2%). The long-context
case likewise improves open plus first render by 1.6% despite moving 0.56 ms
into open. Standard open and scroll medians remain within noise; paired standard
scroll-pass comparisons range from about -0.9% to +0.4%.

## Allocation results

`Churn` is cumulative requested allocation bytes, not retained memory.

| Fixture / stage | Before churn | After churn | Change |
| --- | ---: | ---: | ---: |
| Balanced complete run | 126.30 MB | 121.86 MB | -3.5% |
| Balanced warm scroll | 27.09 MB | 25.65 MB | -5.3% |
| Mega-100k complete run | 79.20 MB | 75.48 MB | -4.7% |
| Mega-100k warm scroll | 11.94 MB | 10.70 MB | **-10.4%** |
| Mega-100k random scroll | 15.83 MB | 14.58 MB | -7.8% |

Allocation/reallocation operation counts fall by 0.2–0.7% in scroll stages.
Peak live growth is unchanged; the optimization removes transient viewport-plan
storage rather than retained model data. A cached large-hunk summary is one
fixed-size record per qualifying hunk and is included in search-index memory
accounting; source text is not duplicated.

## Rejected trials

1. **Allocation-free contiguous focus search.** This removed another viewport
   vector, but repeated standard A/Bs showed a consistent 0.4–0.7% scroll
   slowdown. It was removed rather than trading latency for allocation counts.
2. **Cache every hunk summary.** Standard fixtures do not benefit enough to
   justify extra index storage and line-kind work. The retained 16K threshold
   bounds worst-case frame work while leaving ordinary hunks unchanged.
3. **Replace the exact-sized file read with `fs::read`.** Alternating cached-file
   runs ranged from -0.2% to +0.5%; no latency gain reproduced, so the ingress
   path remains unchanged.

## Correctness and decisions

- `cargo test --workspace --all-targets --all-features --locked`, workspace
  Clippy with warnings denied, `scripts/check-architecture`, and
  `scripts/ci/performance smoke` are green (`mark-tui`: 703 tests).
- Existing sticky, wrapped-sticky, focused-header, grep-header, and annotation
  tests remain green. New tests verify cached addition/deletion counts,
  fallback-context location, meta-line exclusion, all-blank context, and the
  uncached small-hunk path.
- Search indexes are rebuilt or restored together with their matching
  changeset, so cached `(file, hunk)` summaries do not survive diff replacement.
- Keep owned Ratatui frame output and full-buffer diffing. Standard frames are
  already roughly 0.2–0.3 ms, and the sampled tail is primarily dependency
  text/layout work rather than another unbounded application scan.
