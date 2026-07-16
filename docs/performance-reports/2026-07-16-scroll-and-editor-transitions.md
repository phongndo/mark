# Scrolling and editor-transition review — 2026-07-16

Focus: regression-intolerant profiling of full-screen scrolling, annotation-heavy
scrolling, editor suspension/resume, and post-editor reload work.

- Commit: working tree on top of `5c03b4d`.
- Host: arm64 macOS, 16 logical CPUs, 64 GiB RAM; rustc/cargo 1.88.0.
- Raw artifacts: `target/perf-scroll-editor/` (ignored).
- Standard-scroll protocol: release `mark-bench measure`, persistent ratatui
  `TestBackend`, 160x40 diff viewport plus header, 200 sequential/warm/random
  positions, 40–100 alternating-order process pairs.
- Annotation protocol: the balanced fixture with 1, 10, or 50 evenly distributed
  annotations; 20 scroll positions and 20 alternating-order process pairs.
  Long-note runs used 1,000 words per annotation. The 100k-row case used the
  committed `mega-diff-100k` fixture.
- Editor protocol: release `mark diff --no-watch --no-syntax` under a PTY,
  `$EDITOR=/usr/bin/true`, zero pexpect send delay, 30 alternating-order pairs.
  The measured interval starts immediately before Ctrl-G and ends when the
  repainted `editor closed` notice is observed.

## Benchmark correction

The old scroll benchmark called `render_row` for each visible model row. It did
not execute viewport planning, focused-hunk selection, annotation composition,
full screen layout, ratatui buffer diffing, or the normal draw path. The retained
benchmark now keeps one test terminal and invokes the same full `draw` function
used by the application. Initial rendering also owns the first viewport-size
application rather than moving that work outside the timer.

This correction was important to the acceptance decision: an annotation-free
fast path improved isolated diff-content rendering by 2–9%, but only about
0–1% of complete frame time. It is retained only as a guard that keeps the
optional annotation indexes free for the default path, not claimed as a
standalone product win.

## Retained scrolling optimizations

1. **Cache annotation-to-model-row resolution.** Previously every call to
   `max_scroll` scanned the complete eager model once per saved annotation.
   Scrolling was therefore O(annotations × model rows) per input event. Rows
   are now cached, seeded directly when a draft is committed, and invalidated
   whenever the view model is replaced. Multiple missing rows after a layout,
   filter, context, or diff-model rebuild are recovered in one ordered model
   pass, preserving the original first-match behavior. The lookup now uses
   `iter_rows`, which also works for sparse mega-diff models where `rows` is
   intentionally empty.
2. **Cache saved-annotation block heights.** Height depends on immutable text
   storage and viewport width. The cache validates both text allocation
   identity/length and width, and draft heights remain uncached because drafts
   change interactively. Save/remove paths explicitly invalidate their entry.
3. **Count wrapped annotation lines without constructing them.** Rendering and
   height calculation now share one visitor, so bounds checks no longer allocate
   a `Vec<String>` and one `String` per wrapped line. A parity test covers empty,
   multiline, tab/control, narrow, and wide-Unicode text.
4. **Avoid transient annotation copies.** Annotation key discovery returns its
   single possible key directly instead of allocating a one-element vector.
   Rendering borrows saved note text after mutable row rendering instead of
   cloning the complete annotation store every frame.
5. **Keep the no-annotation path free.** Viewport planning directly creates
   ordinary diff slots, rendering skips annotation-key construction, and
   `max_scroll` returns the ordinary viewport bound before touching either
   annotation cache.

The caches are advisory only. Their keys remain the existing path/side/line
identity, negative row results are invalidated with the model, text replacement
and width changes are validated before height reuse, and no rendered or exported
annotation representation changed.

## Results

### Full-screen annotation scrolling

Medians for 20 full draws on the balanced fixture:

| Annotations | Note size | Warm scroll before | Warm scroll after | Random max before | Random max after |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 2 words | 5.88 ms | 5.85 ms | 0.408 ms | 0.405 ms |
| 10 | 2 words | 51.86 ms | 5.98 ms (**-88.5%**) | 2.762 ms | 0.412 ms (**-85.1%**) |
| 50 | 2 words | 256.98 ms | 6.60 ms (**-97.4%**) | 13.269 ms | 0.445 ms (**-96.7%**) |
| 10 | 1,000 words | 59.53 ms | 6.35 ms (**-89.3%**) | 3.137 ms | 0.428 ms (**-86.4%**) |
| 50 | 1,000 words | 294.48 ms | 8.29 ms (**-97.2%**) | 15.282 ms | 0.522 ms (**-96.6%**) |

The one-annotation case is neutral, so the optimization does not trade the
common small review for the stress case. With the row cache already present,
height caching alone reduced 50-long-note warm full-frame time by 77.9%.
Borrowing rather than cloning the store removed another 2.9% in that case.

Batch recovery after model invalidation reduced measured file-filter application
from 2.50 ms to 0.33 ms (-86.8%) with 50 annotations on the balanced fixture,
and from 18.05 ms to 2.54 ms (-85.9%) on the 80,079-row mega-diff fixture.

### Annotation-free scrolling

Complete-frame standard runs remained statistically flat while trending slightly
faster: warm totals moved by -0.4% to -0.7% on the many-small, balanced, and
large-single-file fixtures, and random totals stayed within -1.1% to +0.1%.
Repeated random-scroll maxima were flat within noise. The pathological minified
line also remained flat. No default-path regression was reproduced.

### Editor entry and exit

The safe editor round trip with an instant editor measured **27.8 ms median**
(30 alternating samples). About 20 ms is the deliberate input-settle window
used after disabling mouse reporting; the remainder is event-reader shutdown,
terminal mode changes, process launch/wait, reader restart, and repaint.

An experimental zero-settle build measured **6.79 ms median (-75.6%)**, but was
rejected. The settle window prevents terminal mouse escape reports already in
flight from becoming input to the editor or shell, especially over multiplexers
and remote links. Removing or shortening it would make editor transitions less
reliable to improve a path dominated by launching a real editor.

No change was retained for editor exit either. The existing behavior already:

- pauses the event reader before surrendering the TTY;
- flushes pending input on resume and filters transient quit keys;
- fingerprints the target and skips reload when no edit occurred;
- resumes the UI immediately and queues a path-scoped reload asynchronously.

The existing reload benchmark measured path-scoped loading at 17.96–18.21 ms
versus 34.43–37.73 ms for full reloads (**1.89–2.10x faster**). Replacing this
with a more aggressive lifecycle would add synchronization and input-ownership
risk without a demonstrated user-visible gain.

## Rejected trials

1. **Remove the 20 ms terminal settle.** Large synthetic gain, but permits mouse
   report leakage into the editor/shell. Rejected on correctness and reliability.
2. **Share inline-emphasis vectors with `Arc`.** Warm rendering improved about
   3%, but balanced cold/random passes regressed about 4–5% because atomic/shared
   storage overhead moved work into cache misses. Reverted.
3. **Single-probe inline LRU insertion.** Warm passes improved 3–7%, but several
   cold/random cases and maxima regressed 1–4%. Reverted rather than hiding the
   loss behind warm-only numbers.
4. **Skip hidden-menu list construction.** Full-screen A/Bs were flat (roughly
   ±0.5%); the work is too small beside normal frame construction. Reverted.
5. **Annotation-store borrowing before height caching.** It removed allocation
   churn but full-frame gains were below 1% while height calculation still
   dominated. It was accepted only after height caching made the remaining
   2–4% long-note gain measurable.
6. **Terminal scroll-region/incremental-line rendering.** Not implemented. It
   would couple correctness to terminal capabilities, overlays, wrapped rows,
   annotations, resizing, and alternate-screen recovery. The current bounded
   full redraw is sub-millisecond in standard release runs and is safer.

## Correctness and acceptance gates

- Existing annotation rendering, navigation, wrapping, resize, layout, filter,
  context, reload, export, old/new-side, and sparse-model contracts remain green.
- New tests cover model-cache invalidation, batched row recovery, text/width
  height invalidation, and exact count-only/rendered wrapping parity.
- No terminal protocol, input-settle duration, editor command construction,
  reload policy, annotation identity, rendered text, or public API changed.
- `cargo test --workspace --all-targets --all-features --locked` is green,
  including 515 `mark-tui` tests; workspace clippy with warnings denied is green.
