# Direct diff rendering and inline comparison optimization — 2026-08-23

- Commit: working tree on top of `0488db7`.
- Host: arm64 macOS 26.5.2, 64 GiB RAM.
- Rust/Cargo: 1.98.0.
- Raw artifacts: `target/perf-next/` (ignored).
- Latency protocol: ordinary release binaries, alternating process order, a
  persistent 160x40 Ratatui test terminal, and 24–160 pairs per fixture. Atomic
  allocation counters were disabled.
- Allocation protocol: one opt-in profiling-allocator process per fixture, 200
  balanced and 100 mega-100k positions per scroll pass.

## Profile findings

The post-sticky-header profile exposed three independent costs:

1. Ratatui's unwrapped `Paragraph` path collected every visible line into a
   temporary `Vec<StyledGrapheme>`, even though Mark had already fitted and
   wrapped the viewport. Unicode grapheme segmentation and this reflow staging
   dominated normal frames.
2. Lazy inline emphasis cloned both complete side-index vectors before
   comparing each visible changed-line pair. Its LCS then repeatedly compared
   token strings even when fingerprints or an identical suffix could reject or
   resolve the comparison.
3. Search-index construction converted every printable ASCII patch span through
   the lossy UTF-8 path, then scanned the resulting string again to discover
   that every byte occupied one terminal cell.

After the retained changes, normal frame time is primarily direct cell writes,
Ratatui cell equality/buffer diffing, and first-visit inline LCS work. Unicode
frames retain grapheme segmentation by design. All remaining work is bounded by
viewport cells or configured inline-diff limits.

## Retained changes

- Render already-fitted diff lines directly into contiguous Ratatui buffer-row
  slices. Printable ASCII spans use a byte iterator; Unicode spans retain
  grapheme boundaries, wide-cell behavior, control filtering, alignment, and
  truncation semantics. This removes Paragraph reflow storage and repeated
  coordinate indexing without changing the owned line model.
- Borrow pre-indexed changed blocks while resolving inline pairs instead of
  cloning their deletion/addition vectors.
- Trim the common LCS suffix that the existing reverse backtracker necessarily
  selects, and compare compact token fingerprints before exact text. Fingerprint
  collisions still fall back to byte equality. `u32` offsets keep each token at
  the previous 16-byte size under the existing 4 KiB line cap.
- Compute printable-ASCII search widths directly from patch bytes. Non-ASCII,
  control, and invalid-UTF-8 lines continue through the exact existing lossy and
  terminal-width rules, without a second ASCII probe.

## End-to-end latency

Medians from final alternating-order runs:

| Fixture / metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Many-small initial render | 0.342 ms | 0.222 ms | **-35.1%** |
| Many-small warm scroll, 200 frames | 44.132 ms | 24.784 ms | **-43.8%** |
| Balanced open model | 6.502 ms | 6.113 ms | **-6.0%** |
| Balanced initial render | 0.375 ms | 0.238 ms | **-36.5%** |
| Balanced cold scroll, 200 frames | 52.852 ms | 29.055 ms | **-45.0%** |
| Balanced warm scroll, 200 frames | 43.632 ms | 23.433 ms | **-46.3%** |
| Balanced random scroll, 200 frames | 52.889 ms | 29.383 ms | **-44.4%** |
| Mega-100k open model | 10.656 ms | 8.657 ms | **-18.8%** |
| Mega-100k initial render | 0.301 ms | 0.196 ms | **-34.8%** |
| Mega-100k warm scroll, 100 frames | 20.540 ms | 10.442 ms | **-49.2%** |
| Mega-100k random scroll, 100 frames | 24.613 ms | 13.764 ms | **-44.1%** |
| Mega-1m initial render | 0.322 ms | 0.212 ms | **-34.2%** |
| Mega-1m warm scroll, 20 frames | 4.331 ms | 2.318 ms | **-46.5%** |
| Large-single warm scroll, 100 frames | 19.933 ms | 9.884 ms | **-50.4%** |
| Minified one-line initial render | 0.964 ms | 0.950 ms | -1.5% |
| Syntax-Rust warm scroll, 50 frames | 11.900 ms | 7.495 ms | **-37.0%** |

The minified fixture has one renderable scroll position and remains dominated by
bounded long-line fitting. A 50,000-line all-Unicode addition fixture and a
second fixture with Unicode late in otherwise ASCII source kept load, open, and
scroll medians within about 1% after the search fast path stopped repeating the
ASCII probe. The Unicode rendering path itself remains unchanged except for
removing Paragraph's temporary collection.

## Allocation results

`Churn` is cumulative requested allocation bytes, not retained memory.

| Fixture / stage | Before ops | After ops | Before churn | After churn | Churn change |
| --- | ---: | ---: | ---: | ---: | ---: |
| Balanced complete run | 584,388 | 566,743 | 121.86 MB | 99.71 MB | **-18.2%** |
| Balanced cold scroll | 216,513 | 207,837 | 45.08 MB | 35.13 MB | **-22.1%** |
| Balanced warm scroll | 154,053 | 152,653 | 25.65 MB | 22.40 MB | **-12.7%** |
| Balanced random scroll | 207,354 | 199,860 | 42.17 MB | 33.30 MB | **-21.0%** |
| Mega-100k complete run | 203,237 | 201,071 | 75.48 MB | 70.18 MB | **-7.0%** |
| Mega-100k warm scroll | 57,915 | 57,215 | 10.70 MB | 9.07 MB | **-15.2%** |
| Mega-1m warm scroll, 20 frames | 11,715 | 11,575 | 2.71 MB | 2.39 MB | **-12.0%** |

Peak live growth is unchanged. Direct rendering removes transient Paragraph
capacity growth; borrowed block indexes and smaller LCS matrices remove
first-visit churn. No rendered-line cache or additional retained model is added.

## Correctness evidence

- A deterministic differential test compares the direct widget with the old
  Paragraph reference across 256 generated cases containing multiple spans,
  inherited styles, all alignments, narrow clipping, pre-existing buffer
  symbols, controls, combining marks, CJK, emoji ZWJ sequences, and halfwidth
  sound marks.
- A 4,096-case differential test compares optimized inline changed-token masks
  with the original complete LCS matrix. Exact byte comparison remains the
  collision fallback.
- Search-width regression coverage includes printable ASCII, tabs, controls,
  Unicode, and invalid UTF-8.
- Existing rendering, wrapping, inline emphasis, non-UTF-8, syntax, annotation,
  sticky-header, and snapshot tests exercise the affected paths.
- `cargo test --workspace --all-targets --all-features --locked` (including 706
  `mark-tui` tests), workspace Clippy with warnings denied,
  `scripts/check-architecture`, and `scripts/ci/performance smoke` are green.

## Rejected trials

1. A specialized integer hasher reduced selected passes by only 0–1.5% with
   mixed random-scroll tails; the extra hashing machinery was removed.
2. Skipping blank-symbol writes regressed scroll passes by 4–7%; `set_char`
   regressed them by 7–13%.
3. Folding the viewport base-style pass into per-cell styles regressed frames by
   15–24% because style patching moved into the inner character loop.
4. A stack LCS matrix, retained LCS scratch, exact token pre-counting, and a
   heuristic token reserve all reduced allocation events but regressed ordinary
   latency by roughly 0.5–2.4%; none were retained.
5. Per-line UTF-8 validity fields increased the 100k-line model by 800 KiB.
   Packed/lazy variants avoided the memory cost but regressed all-Unicode open
   or random-scroll cases. The retained search-only byte path delivers the ASCII
   win without changing line storage.

## Decision

Keep full Ratatui buffer diffing and owned viewport lines. At roughly
0.10–0.15 ms per warm standard frame, the remaining normal-frame tail is the
required per-cell write/compare path; terminal scroll-region rendering would
reintroduce the overlay, wrapping, resize, and recovery risks rejected in the
previous review. Further inline work should begin with first-visit allocation
attribution and must preserve the exact differential LCS masks.
