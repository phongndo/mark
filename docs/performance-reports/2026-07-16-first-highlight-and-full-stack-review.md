# First-highlight and full-stack optimization round 10 — 2026-07-16

Focus: a regression-intolerant review of Mark startup, diff loading/modeling,
viewport rendering, syntax scheduling, grammar setup, and the remaining native
TextMate engine tail.

- Commit: working tree on top of `eb7e759`.
- Host: arm64 macOS, 16 logical CPUs, 64 GiB RAM; rustc/cargo 1.88.0.
- Raw artifacts: `target/perf-round10/` (ignored).
- Syntax protocol: release `mark-bench syntax-compare`, separate process per
  sample, identical committed catalog corpus per A/B, 15 alternating-order
  pairs. Matcher profiles used release `profile-cold --mode line-cold` and the
  macOS sampling profiler.
- Whole-program protocol: release `mark-bench measure`, three samples, 160x40
  viewport, at most 100 measured scroll positions.

## Retained optimization

Bundled grammar dependency discovery now parses each selected grammar once.
The previous first-highlight path inflated a grammar blob, parsed it into a
full `serde_json::Value` solely to discover external includes, retained that
intermediate tree, and then parsed the same JSON again into `CompiledGrammar`.
The new path walks the include graph already represented by `CompiledGrammar`,
uses a scope index rather than repeated linear blob scans, and drops decoded
JSON bytes after each grammar is compiled.

The dependency rules remain deliberately identical to vscode-textmate's
loader. In particular, capture-only includes and repositories local to an
include-only rule are not expanded while computing the bundled closure. A
reference parity test protects the embedded-heavy Markdown/MDX/AsciiDoc/PHP/
YAML closures and Wikitext's local-repository boundary. A one-off exhaustive
check also compared all 264 public roots with the previous JSON traversal.

### First-highlight latency

Alternating-order process-cold medians, 15 samples per side:

| Corpus | Before | After | Delta |
| --- | ---: | ---: | ---: |
| AsciiDoc stress | 47.151 ms | 40.771 ms | **-13.5%** |
| MDX stress | 49.232 ms | 42.970 ms | **-12.7%** |
| PHP stress | 25.300 ms | 24.249 ms | **-4.2%** |
| C++ stress | 18.475 ms | 17.390 ms | **-5.9%** |
| Rust stress | 1.619 ms | 1.598 ms | -1.3% |

The gain scales with the number and size of grammar blobs in the selected
external-include closure. Rust correctly stays nearly flat because its closure
is small.

### Process peak RSS

Seven separate-process samples were stable to one 16 KiB page. Median maximum
RSS:

| Corpus | Before | After | Delta |
| --- | ---: | ---: | ---: |
| AsciiDoc stress | 63.95 MB | 55.21 MB | **-13.7%** |
| MDX stress | 68.78 MB | 57.25 MB | **-16.8%** |

The optimization removes the temporary generic JSON tree and no longer keeps
all inflated closure bytes until compilation starts.

## Full-stack profile

Current release measurements show that the non-engine paths are already below
the threshold for a worthwhile risky rewrite:

- Adjusted `mark --version` medians: **1.323–1.853 ms** across verification
  runs, within the 2 ms gate.
- Standard patches from 0.34–2.33 MB: load **0.10–0.40 ms**, open model
  **4.4–6.4 ms**, ordinary random-scroll max **0.02–0.15 ms**.
- 7.31 MB / 100k-row synthetic patch: load **1.1 ms**, open **9.1–9.6 ms**,
  random-scroll max **0.05–0.06 ms**.
- 1.60 MB pathological minified line: initial render **0.72–1.23 ms**;
  syntax correctly takes the configured large-line fallback rather than
  exposing the regex engine to unbounded work.
- Syntax-many-small-Rust: initial render **0.14 ms**, random-scroll max
  **0.15 ms**, and only 5.4 KiB of deduplicated scope-table storage.

These measurements support retaining the current span-backed diff parser,
sparse large-row model, viewport-bounded rendering, asynchronous syntax queue,
and full-file/line caps. The 10M scale results and memory ratios remain covered
by the round-7/mega-diff reports.

## Remaining engine attribution

Release sampling confirms that the slow language tail is no longer dominated
by catalog lookup, worker setup, or output construction:

- C++: about 60% of samples remain in ordered pattern-set selection; about 45%
  are in fallback matching and 29% in the ordered bytecode VM. The surviving
  VM executions are mostly winner/heavy-pattern work, agreeing with round 8.
- MDX: about 69% is ordered pattern-set selection, 53% fallback matching, and
  32% bytecode VM work.
- AsciiDoc: about 64% is ordered pattern-set selection, 39% fallback matching,
  and 25% bytecode VM work.

This makes a compact/specialized VM the only plausible large engine lever, but
it is also the highest correctness-risk area: ordered alternatives, captures,
lookaround, recursion, atomic groups, and Oniguruma offsets are observable.
It should not be changed without exact scope streams, differential regex tests,
the 264-language floor, and before/after process RSS.

## Rejected trials

The following experiments were removed because they failed the acceptance
rules even when they looked locally attractive:

1. **Precomputed scanner entry epsilon closures.** It removed repeated split
   traversal but still had to enqueue the same consuming states. Fennel was
   0.35% slower, Emacs Lisp flat, and Rust only 0.67% faster. No meaningful
   aggregate gain.
2. **Lazy local bytecode scratch construction.** This improved the focused
   AsciiDoc fallback corpus by 8.3% and MDX by 3.7%, but a four-sweep catalog
   A/B was slightly worse in aggregate (20.592 -> 20.490 MB/s median) and
   produced material regressions in scanner-heavy languages, including Fennel
   (-5.9%). Rejected rather than hiding the losses behind aggregate noise.
3. **Warm canonical-language fast path.** A deliberately favorable 10,000-pass
   tiny-file microbenchmark improved only 1.9%; real syntax jobs do much more
   work, so the expected end-to-end gain is not meaningful.
4. **Distribution PGO integration.** Existing local PGO remains valuable
   (documented 5–35% engine gains), but enabling it in the four-platform release
   workflow without per-target training freshness and build-time safeguards
   would make releases more failure-prone. Keep `scripts/build-pgo` opt-in until
   the release pipeline can prove those properties.

The reverted-experiment list in `docs/textmate-engine.md` still applies; this
round did not retry per-pattern next-match memoization, naive linear bytecode,
position-only recursive subroutines, oversized budgets, or frontier-forced
candidate scans.

## Correctness and acceptance gates

- `cargo test --workspace --all-targets --all-features --locked` and workspace
  clippy with warnings denied are green.
- `cargo test -p mark-syntax --release --locked`: 342 unit tests, 5 capture
  quality tests, 21 TextMate golden tests, and 2 theme golden tests green.
- Exact manifest scope streams remain green with zero budget degradation.
- The retained closure sets match the previous JSON dependency contract for
  every public root in the exhaustive research check; representative contracts
  remain as a permanent test.
- No public API, syntax limits, queue policy, fallback budget, grammar asset,
  or rendered-token representation changed.
- Focused gains are double-digit on the intended embedded-heavy first-highlight
  path and process RSS falls rather than trading memory for latency.

## Safe follow-ups

1. Add byte accounting for compiled grammar/matcher caches before attempting
   another memory optimization. Entry counts alone cannot police a large
   dynamic end-pattern or line-cache payload.
2. If the bytecode VM is revisited, start with instruction-width and dispatch
   measurements, not semantic shortcuts. Require alternating-order corpus A/Bs
   and exact oracle output for every retained change.
3. Make release PGO reproducible per target (training corpus hash, allocator
   identity, profile freshness, timeout budget, and a plain-release fallback)
   before considering it for shipped assets.
4. Keep the current large-source and long-line caps. Removing them can improve
   a synthetic throughput number while making the interactive program less
   predictable and more error-prone.
