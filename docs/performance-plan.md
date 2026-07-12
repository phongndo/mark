# Mark performance, efficiency, and quality plan

The working plan for performance research, profiling, debugging, evaluation,
and report capture across **the whole `mark` binary** — diff ingest, parse,
TUI model, search, rendering, the syntax runtime, and the native TextMate
engine. Target workload: a mixed-language unified diff with up to
**10,000,000 diff rows**, opened, scrolled, filtered, searched, and
highlighted without hangs, OOMs, or unbounded syntax work — while holding the
engine's correctness bar (oracle-exact highlighting, zero degradation on
committed corpora).

This document supersedes and combines the earlier plan docs
(`performance-10m-plan.md`, `textmate-performance-plan.md`, and
`textmate-grammar-expansion-plan.md`, all removed): the grammar-expansion
plan's catalog/quality phases landed in `54380a2`, its Q1–Q4 quality gates
are restated in §8, and its per-grammar landing checklist moved to
`textmate-engine.md` ("Adding or updating a grammar").

## 0. Current implementation status (2026-07-11 working tree)

The large-diff implementation pass has landed in the working tree. The release
gate run for `mega-diff-10m` now opens an 8,007,813-row UI model from a
750.3 MB patch with RSS delta 1.42× patch bytes, load 201 ms, open 126 ms,
interactive grep 232 ms, and random-scroll max 93 µs. See
`docs/performance-reports/2026-07-11-mega-diff-memory.md` for commands and
raw-artifact paths.

Implemented plan items include: byte/span-backed diff lines; lazy/no-copy grep
search over the changeset; sparse large UI row segments; explicit environment
diff limits; non-UTF8 per-line decode boundaries; static-mode huge syntax caps
and raw static fallback; viewport-safe syntax queue/cache byte budgets and
worker-side size checks; mega-diff fixtures/variants; random/end scroll and
RSS/component-memory reporting; repeated-sample JSON stats; CI smoke coverage;
and the current TextMate hot-path work already represented in the engine
baseline below.

The follow-up implementation pass also added default-on shared-pool rayon
diff-section parsing (`MARK_CPU_THREADS`, max 8), multi-worker syntax highlighting via
`[limits] worker_threads`, per-language/per-source-kind syntax latency buckets
and first-visible latency reporting, stage timing rows in `mark-bench`, sparse
inline hunk-emphasis storage with local block caps, a non-O(n) large-model
wrapping fallback table, exact sparse-model wrapping cache identity, unbounded
syntax queue eviction bookkeeping, profile/bundle assertion flags for CI, and
grammar/pattern ids on TextMate hotspot diagnostics.

## 1. Measured state (round 6 — 2026-07-11, `54380a2`, macOS/Apple Silicon, rustc 1.88, release)

Health: `cargo test -p mark-syntax --release` green (258 + 9 + 5 tests);
`divergences.toml` empty; `degraded_lines = 0` and `fallback_budget_kills = 0`
on all profiled corpora (rust, cpp, nix, php, typescript); stripped `mark`
binary **5.6 MB** with the full 254-language Shiki catalog (≤ 9 MB gate passes
with room).

Build used for all numbers:

```sh
cargo build --release -p mark-cli -p mark-bench
cargo build --release -p mark-syntax \
  --example profile-cold --example profile-counters --example tokenize
```

### 1.1 Engine throughput (process-cold, `profile-cold`)

Reference corpora:

| Corpus | Scope | Bytes | Result |
| --- | --- | ---: | ---: |
| large-rust `bench1.rs` | `source.rust` | 1,897,788 | **10.05–10.5 MB/s** (target 12) |
| libc++ `<string>` | `source.cpp` | 184,514 | **2.32–2.35 MB/s** (floor 2.5) |
| representative-markdown | `text.html.markdown` | 31,961 | 3.39 MB/s |
| `pi-mark/extensions/pi-mark.ts` | `source.ts` | 13,199 | ~1 MB/s (small/noisy — one-shot compile dominates) |
| repeated TypeScript stress | `source.ts` | 83,460 | 16.3 MB/s with production line cache; **2.09 MB/s with line cache disabled** |

Per-language sweep over the regenerated core-repeated members (~83 KB each,
39 languages, process-cold), slowest first:

| Language | MB/s | | Language | MB/s |
| --- | --- | --- | --- | --- |
| nix | **2.2–2.4** | | ocaml | 21.9 |
| php | **2.7–2.8** | | ruby | 22.2 |
| markdown | 9.8 | | javascript | 25.0 |
| cpp | 10.6 | | css / c / llvm | 33–34 |
| asm | 13.9 | | swift / jsx / riscv / scss | 39–44 |
| html | 14.6 | | csharp/mips/python/go/bash/sql/mojo | 48–76 |
| tsx | 16.0 | | java / odin / powershell / lua / mlir | 74–96 |
| typescript | 18.0 | | rust / zig / terraform → json | 100–295 |

The engine pass in `54380a2` was transformative vs round 5: nix 0.07 → 2.2
(32×), sql 0.85 → 75, cpp stress 0.78 → 10.6, markdown 1.34 → 9.8; cpp
degraded lines 1,229 → 0. Round-5 hotspot tables are obsolete.

**Line-cache caveat:** the repeated corpus rewards the production line cache
(TS: 16.3 vs 2.09 MB/s without it). Cold-scanner improvements must also be
validated on non-repeated corpora (large-rust, libc++, representative-*) so
cache wins do not hide scanner regressions.

### 1.2 Engine counter diagnosis of the remaining tail

| Corpus | Key counter signal |
| --- | --- |
| nix repeated | 22,780 state-cache misses / 3,418 lines (6.7 per line); pattern hotspot time is microseconds — cost is pure state identity/maintenance (frame-stack case, §5 E1). 787 k candidates considered, 239 k fallback attempts. |
| php repeated | 1.20 M candidates considered, prefilter hit rate 3.3% (1.17 M checks → 38 K hits); one DFA pattern (`((?:(?:final\|abstract\|…)\s+)*)(function)`, 11.9 K attempts) is ~11% of run time alone. |
| cpp repeated | 14.7 M fallback steps (was 54.6 M), 1.07 M candidates, prefilter hit rate 5.8%. |
| libc++ string | 1.55 M candidates, 1.01 M fallback attempts, 20.2 M fallback steps; scope-resolution and declaration regexes dominate. |
| TypeScript repeated | 1.12 M candidates, 739 k fallback attempts, 30.5 M fallback steps; declaration/function/regex-literal lookaround patterns dominate. |
| large-rust | 35.9 M candidates considered, 9.47 M fallback attempts, 81.2 M fallback steps; top DFA hotspot is the angle-bracket spacing rule. |

### 1.3 Whole-binary evaluation

**Mega-diff tiers** — synthetic unified diffs generated from the
core-repeated corpus (real code, 39 languages; ~60% context / 20% deletions /
20% additions, whole-file hunks), driven through `mark-bench measure-patch`
(real parse + real `DiffApp` + synthetic TUI):

| Tier | Patch | Files | load (parse) | open (DiffApp) | grep filter | max RSS |
| --- | --- | --- | --- | --- | --- | --- |
| 100 K lines | 2.3 MB | 23 | 5.3 ms | 16.8 ms | — | ~54 MB |
| 1 M lines | 22.3 MB | 227 | 29–41 ms | ~70 ms | 5.7 ms | **325 MB** |
| 10 M lines | 223 MB | 2,265 | 286–310 ms | ~650 ms | 56 ms | **3.21 GB** |

**Existing `mark-bench` stress suite** (for context):

| Scenario | Patch/rows | Result |
| --- | --- | --- |
| Standard generated suite | up to 30 k rows | opens in ~6–13 ms; warm scroll 12–31 µs/step without syntax |
| `syntax-large-rust` | 16 k rows, 1.9 MB source | syntax settle ~161 ms, syntax RSS delta ~43 MB |
| `huge-mixed-stress` | 23.5 MB patch, 211 k rows, 1,500 files | load ~15 ms, open ~41 ms, RSS delta ~84 MB |
| `huge-mixed-stress` + TS syntax | same patch | queue stayed viewport-bounded: 64 jobs, ~570 KB queued source, ~5.3 MB syntax cache, settle ~113 ms |

Interpretation: **time scales linearly and is acceptable** (~1 s to
interactive at 10 M lines; scroll, hunk navigation, and filters stay in the
tens of ms; the interactive syntax queue is genuinely viewport-bounded).
**Memory is the blocker: peak RSS ≈ 14× patch bytes.** The diff input, parsed
line strings, UI row model, and search index all materialize whole-diff
structures; that is not a 10M-row architecture.

Other whole-binary numbers:

- Startup: `mark --version` / `mark patch /dev/null` = **2.0 ms**; the
  254-language catalog is lazily decoded (first highlight of a language
  ≈ 2–3 ms including zlib inflate + grammar compile).
- Non-TTY path (`mark patch big.diff | …`): pure streaming, 0.02 s / 8 MB RSS
  for the 223 MB patch — already ideal; do not regress it.
- CPU attribution during the 10M run (`sample`, top-of-stack): malloc/free
  ≈ 22% of samples (allocation churn is the #1 CPU cost), then `DiffApp`
  init, `terminal_text`, `TextMatcher::matches`, `GrepMatcher::find_at`.
  `parse_patch` itself is ~1%.

### 1.4 Where the 14× memory goes (10M-line diff)

Estimates from struct layout × measured counts (8.33 M UI rows, 10.0 M diff
body lines, 223 MB patch):

| Component | Approx cost | Mechanism |
| --- | --- | --- |
| Parsed `Changeset` | ~0.9–1.1 GB | `DiffLine` enum ≈ 48 B/line + one heap `String` per line (~32 B avg after allocator rounding); 10 M lines, 10 M allocations |
| `UiModel.rows` | ~0.5 GB | `UiRow` enum ≈ 56–64 B × 8.33 M rows (largest variant `Collapsed` has 6 words) |
| `DiffSearchIndex` | ~0.45 GB | `grep_text: Vec<u8>` is a **full second copy of all diff text** + `SearchLineRef` per line + `filter_texts` |
| Raw patch buffer | 223 MB | git output `Vec<u8>` (kept through parse; `from_utf8_lossy` borrows when valid UTF-8, copies the whole patch when not) |
| `DiffViewModel` (static-pager path) | ~0.27 GB when built | `DiffRowRef` 32 B × rows |

Second-order risks: the syntax LRU is **entry-bounded (512), not
byte-bounded** — `memory_bytes()` exists but is only reported, never
enforced; full-file highlight sources are fetched via `git show` without a
size check before reading all bytes.

### 1.5 Tooling breakage found

- `crates/mark-syntax/examples/bundle-measure.rs` fails with
  `UnknownLanguage("bash")` since the catalog rework — fix or delete.
- Round-5 sweep members were 108 KB; the regenerated members are ~83 KB with
  partially different content, so cross-round MB/s comparisons are
  approximate.

### 1.6 External engine lessons to keep applying

- vscode-textmate tokenizes line-by-line with a carried rule stack;
  incremental work can stop once the end-state stabilizes again.
- Compact/binary token streams and immutable/shared scope stacks matter more
  than rich per-token objects on hot paths.
- Syntect-style wins remain relevant: lazy regex compilation, scope
  interning, regex/match caching, compact scope metadata, pre-linked grammar
  includes.
- Mark's production path stays pure Rust and offline; external engines are
  baselines/oracles, not runtime dependencies.

## 2. Goals and acceptance gates

### 2.1 Correctness gates (release blockers, every optimization)

- `cargo test -p mark-syntax --test textmate_golden --locked` green.
- `node tools/generate-goldens.mjs --check` clean; no committed fixture
  divergences; `stoppedEarly = false` in oracle goldens.
- `degraded_lines == 0` and `fallback_budget_kills == 0` on committed corpora.
- If an optimization changes scope output, first add/refresh oracle fixtures —
  never accept visual-only evidence.

### 2.2 Engine performance gates

| Gate | Target | Today |
| --- | --- | ---: |
| large Rust process-cold | ≥ 12 MB/s | 10.05 |
| libc++ C++ process-cold | ≥ 2.5 MB/s, zero degradation | 2.32 |
| nix / php repeated process-cold | ≥ 4 MB/s | 2.2 / 2.7 |
| TS/TSX/JS representative corpora | ≥ 4 MB/s **without** relying on repeated-line cache wins | 2.09 (cache-off TS) |
| core repeated mixed-language sweep | ≥ 6 MB/s aggregate; minimum ≥ 8 MB/s after E4 | — |

### 2.3 Whole-binary 10M-diff gates (mega-diff harness, release build)

| Gate | Target | Today |
| --- | --- | --- |
| G-MEM: peak RSS opening the TUI | **≤ 3× patch bytes** (~0.7 GB @ 223 MB) | 14.4× (3.21 GB) |
| G-OPEN: load + open to interactive | ≤ 1 s; no unnecessary full-output strings | ~0.95 s ✅ (keep) |
| G-SCROLL: scroll/hunk-nav | warm p95 ≤ 16 ms, max ≤ 50 ms | sub-ms ✅ (keep) |
| G-SEARCH: grep filter over 10 M lines | ≤ 250 ms | 56 ms ✅ (keep) |
| G-STREAM: non-TTY path | O(buffer) memory, streams or raw-passes | ✅ (keep) |
| G-LIMITS: oversized inputs | bounded by explicit `max_*` policies with visible truncation/skip reasons, never an OOM | no policies today |
| G-SYNTAX: interactive syntax | visible-window bounded; queue/cache respect byte budgets; no whole-diff static syntax by default | entry-bounded only |

## 3. Phase M — memory model (the 10M blocker; do first)

### M1. Span-based diff model (keystone)

Replace per-line `String` ownership with spans into the shared patch buffer:
`Changeset` keeps `raw: Arc<[u8]>` (the git output, exactly one copy);
`DiffLine` becomes a compact record `{ kind: u8, old_line: u32, new_line: u32,
text: (u32 offset, u32 len) }` ≈ 17–20 B packed vs ~80 B + allocation today.

- Kills ~10 M allocations (the measured #1 CPU cost) and ~0.7–0.8 GB.
- Parse becomes a scan that never copies line bodies; expect load well under
  200 ms at 10 M lines.
- API: `DiffLine::text(&self, raw: &Patch) -> &str` (lossy decode at the
  boundary per line, cached nowhere) or a resolver handle owned by
  `Changeset`; callers in mark-tui/mark-cli/mark-bench migrate mechanically.
- Line numbers as `u32` (4-billion-line files out of scope; assert/saturate).
- Watch out: `text_mut` and any caller holding `&str` across changeset
  mutation; difftool/patch sources already produce owned buffers, so
  ownership stays uniform. Do not keep both `raw_patch` and the span buffer
  as separate copies.

Acceptance: 10M-tier parsed-model cost ≤ 250 MB over the raw patch; load
≤ 200 ms; no behavior change in `mark-diff` tests.

### M2. Search index without text duplication

`DiffSearchIndex.grep_text` re-concatenates every diff line. After M1, index
over the raw patch buffer: store per-line `(offset, len)` (already the model
representation) and run `GrepMatcher`/`memchr` directly on `raw`;
`SearchLineRef` fields shrink to `u32`. `filter_texts` (per-file path
strings) stays — it is small. Additionally make index construction **lazy and
cancelable** (built on first search, not at open), and keep file-name
filtering separate from full-text grep.

Acceptance: search-index memory ≤ 100 MB on the 10M tier; grep ≤ 56 ms
(one contiguous buffer should be faster, not slower); zero index cost when
search is never used.

### M3. Compact UI rows

Two options, in order of preference:

1. **Implicit row index**: rows are fully derivable from (file, hunk) prefix
   sums the way `DiffViewModel` already binary-searches; store per-hunk
   cumulative row starts (O(hunks) = 2,265 entries on the 10M tier) and
   materialize a `UiRow` on demand. Collapsed/expanded context state lives in
   a small side map keyed by hunk. Memory: ~0.5 GB → ~1 MB.
2. If (1) fights too much UI code: pack `UiRow` to 16 B (`u32` indices, `u8`
   tag; move the rare `Collapsed` payload to a side table).

Apply the same treatment to `DiffViewModel`/`DiffRowRef` on the static-pager
path.

Acceptance: UiModel memory ≤ 150 MB on the 10M tier; scroll/viewport-plan
timings unchanged (binary search per visible row ≈ 30 lookups/frame).

### M4. Explicit input limits and degradation policy

Add `max_patch_bytes`, `max_diff_rows`, `max_files`, `max_hunks`, and
`max_line_bytes` policies with **visible truncation/skip reasons** (never a
silent hang or OOM): oversized inputs enter an explicit truncated mode or
fall back to stats-only/raw-pass. Disable or shrink diff cache/prefetch above
size thresholds; avoid cloning the base changeset in non-live/static modes.

### M5. Keep the lossy-decode boundary honest

With M1 the model no longer needs `String::from_utf8_lossy` over the whole
patch (today it borrows when valid, copies 223 MB when any byte is invalid).
Decode per line at render/search time so a single invalid byte no longer
doubles peak memory. Non-UTF-8 bodies become a benchmark fixture variant
(§7 B1).

### M6. Streaming static pager

Stream the static pager parse→render where possible; otherwise raw-pass or
stats-only huge inputs instead of building a full app + model. The
non-interactive path must never materialize whole-diff structures the
terminal will only see once.

Sequencing: M1 → M2/M3 share the span plumbing; M4–M6 are independent and
can land any time.

## 4. Phase U — UI, search, and render scalability

- **U1: sparse/windowed line wrapping** — compute wrapping for the viewport
  (plus margin), not for every row up front.
- **U2: cap inline hunk emphasis by visible window** — never allocate vectors
  sized by a 10M-line hunk (`InlineHunkEmphasisCache` inputs).
- **U3: random-access benchmarks** — add random-jump/end-of-file paths to the
  bench suite, not only sequential scrolling, so lazy structures are honest.
- **U4: render hot-path audit** — `terminal_text` and `TextMatcher::matches`
  showed up in whole-binary samples; re-profile after M1–M3 and shave what
  remains (they may simply be the survivors once allocation noise is gone).

## 5. Phase E — TextMate engine hot path (carried over, re-based)

Methodology unchanged: alternating-order separate-process A/B, paired
medians, byte-exact scope streams vs the pinned oracle before/after; validate
on **both** repeated and non-repeated corpora (line-cache caveat, §1.1). The
reverted-experiment blacklist in `textmate-engine.md` still applies (no
per-candidate memoization, no linear-only bytecode, no position-only
subroutines, no 10× budgets).

- **E1: persistent hash-consed frame stack.** nix pays 6.7 state-cache
  misses/line on pure state identity; keeps O(1) state equality and bounded
  stack cloning for every deep-stack grammar. Acceptance: nix ≥ 8 MB/s (from
  2.2), no sweep regression > 1%, streams byte-identical.
- **E2: aggregated candidate prefilter.** php 1.2 M candidates at 3.3% hit
  rate; cpp 1.07 M at 5.8%; html/TS-family similar. Per-candidate-set union
  first-byte bitmap + required-literal cursors so whole groups are skipped
  before per-pattern checks. Acceptance: php ≥ 6 MB/s, cpp ≥ 15 MB/s, no
  regression.
- **E3: bytecode alternation/repetition + C-family comment-or-space idiom.**
  cpp still burns 14.7 M fallback steps; libc++ at 2.32 vs the 2.5 floor;
  large-rust at 10.05 vs 12 (81 M fallback steps, angle-bracket spacing rule
  on top). Includes iterative fallback execution, smaller capture layouts,
  cheaper lookbehind. Acceptance: libc++ ≥ 2.5 MB/s, large-rust ≥ 12 MB/s,
  cpp fallback steps cut ≥ 3×.
- **E4: lazy ordered frontier.** Single-pass mixed regular/advanced candidate
  traversal in grammar order; lifts the TS/markdown/html mid-tier
  (9.8–18 MB/s). After E1–E3. Acceptance: sweep minimum ≥ 8 MB/s (nix/php
  per E1/E2), markdown ≥ 15 MB/s, TS cache-off ≥ 4 MB/s.
- **E5: single-pattern DFA outliers + word-set matching.** php's
  `((?:modifiers\s+)*)(function)` style bounded-repetition-of-alternation
  prefixes; extend the multi-literal/keyword word-set matcher to any
  remaining huge case-insensitive alternations (SQL-style, assembly-style).
  Fold into E2 if trivial.
- **E6: diagnosability.** Grammar-specific hotspot reports mapping pattern
  ids back to grammar file, rule path, and corpus line samples; better budget
  diagnostics. Cheap, do alongside E1.

## 6. Phase S — syntax runtime at diff scale

- **S1: byte-budgeted syntax LRU** in addition to entry count, using the
  existing `HighlightedSide::memory_bytes()` as weight (default budget e.g.
  64 MB, configurable next to `cache_entries`).
- **S2: byte-capped worker queue** — cap queued full-file job source bytes so
  a burst of 1 MB files cannot hold ~0.5 GB of pending sources.
- **S3: size-check before fetch** — check blob size (`git cat-file -s` /
  `symlink_metadata`) before reading full-file sources; skip with
  `TooLarge` instead of reading then discarding.
- **S4: queue/skip observability** — per-language and per-source-kind queue
  latency and skip-reason metrics in the benchmark report.
- **S5: static-mode assertion** — interactive syntax stays strictly
  viewport/prefetch bounded; assert that static mode never calls
  `prepare_syntax_for_viewport(model.len())` for huge diffs.

## 7. Phase B — benchmarks, CI, and report capture

### B1. Fixtures

Port the mega-diff generator (session scratchpad script) into `mark-bench
fixtures --scenario mega-diff-{100k,1m,10m}`: deterministic output from the
committed core-repeated corpus builder, manifest with line/file counts.
Add variants: one huge hunk, many hunks, many files, binary-heavy, generated
files, minified long lines, non-UTF-8 bodies. Record per-tier floors (§2
gates) next to `benchmarks/textmate/corpora.toml`.

### B2. Measurement features

- Repeated-sample p50/p95/max and peak-RSS reporting in `mark-bench`.
- Stage timing counters: git/read, normalize, decode, parse, model, search
  index, syntax queue, render, static write.
- Component memory estimates for `Changeset`, raw patch, UI model, search
  index, inline cache, syntax cache, diff cache.

### B3. CI jobs

- Perf smoke with loose (2×) regression bounds: large-rust, libc++,
  nix/php, sweep aggregate, and the mega-diff generator at a CI-safe scale
  (1M tier).
- RSS gate: `mark-bench measure-patch --assert-max-rss-ratio 3.0` on the 1M
  tier.
- Counters assertion: `degraded_lines == 0 && fallback_budget_kills == 0`
  across the sweep corpus; `divergences.toml` emptiness.
- First-highlight latency check (< 5 ms per language) to catch
  compression/compile regressions; fix or remove `bundle-measure` (§1.5).

### B4. Report recording

Two layers:

1. **Raw artifacts** under ignored `target/`:

   ```text
   target/textmate-performance/reports/<date>-<commit>/
     host.json
     profile-cold/*.txt
     counters/*.json
     mark-bench/*.json
     oracle/*.json
     sample-stacks/*.txt
     heap/*.txt
   ```

2. **Committed summaries** in `docs/performance-reports/YYYY-MM-DD-*.md`
   when a run changes a decision, with the ledger row shape:

   | Date | Commit | Host | Tool | Corpus | Command | p50 / p95 / max | Peak RSS | Gate | Notes |
   | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

Required metadata for every raw JSON report: commit SHA + dirty flag; host
CPU/RAM/OS; Rust/Cargo/Node/oracle versions; exact command line and env
overrides; corpus path, bytes, lines/rows, language mix, SHA-256; wall-clock
timings (p50/p95/p99/max for repeated samples); peak RSS and component
estimates; syntax counters (degraded lines, fallback kills/steps, cache hits,
candidates, capture replays, top hotspots); correctness status (divergence
count, `stoppedEarly`, degraded/skipped lines, fixture versions).

Benchmark rules: build release before measuring; separate processes for
process-cold runs; alternate A/B order and compare paired medians; never
serialize full tokens inside timed intervals; counters in a separate
diagnostic pass (they perturb timing); raw files stay ignored, only summaries
and decisions get committed. Keep a decision log for reverted experiments
with measured deltas so neutral ideas are not retried.

### B5. Reproducible commands

```sh
# corpora + oracle tools
npm install --prefix tools/golden-oracle
python3 tools/build-textmate-corpora.py

# engine timing
target/release/examples/profile-cold --mode process-cold \
  --assets assets/tm-grammars/languages --scope source.rust \
  target/syntax-fixtures/syntax-large-rust/repo/src/bench1.rs 1

# engine counters (separate diagnostic pass)
target/release/examples/profile-counters \
  --assets assets/tm-grammars/languages --scope source.cpp \
  /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/c++/v1/string \
  > target/textmate-performance/reports/<run>/counters/cpp-libcxx-string.json

# pinned vscode-textmate oracle timing
node tools/textmate-bench.mjs --mode process-cold \
  --assets assets/tm-grammars/languages --scope source.rust \
  --file target/syntax-fixtures/syntax-large-rust/repo/src/bench1.rs \
  --iterations 1 --json

# whole-binary fixture + measurement
target/release/mark-bench fixtures --out target/syntax-fixtures \
  --scenario huge-mixed-stress --force
target/release/mark-bench measure --fixtures target/syntax-fixtures \
  --scenario huge-mixed-stress --syntax-language typescript --json

# patch-file path (mega-diff tiers)
target/release/mark-bench measure-patch PATCH.diff \
  --syntax-language rust --syntax-language typescript --json
```

## 8. Phase Q — quality guardrails (hard gates on everything above)

Unchanged and non-negotiable:

1. **Q1 zero degradation:** budget kills remain a safety valve for
   adversarial input, but any budget kill or degraded line on a committed
   fixture/benchmark corpus is a release blocker (CI-asserted, §7 B3).
2. **Q2 oracle parity:** byte-exact scope-stack parity with the pinned
   `vscode-textmate@9.2.0` + `vscode-oniguruma@1.7.0` oracle on every
   committed fixture; `divergences.toml` stays empty; streams tracked by
   sha256.
3. **Q3 inventory-driven conformance:** every regex construct a grammar uses
   that is not in `regex-conformance.mjs`'s proving set gets a conformance
   case before the grammar (or optimization touching it) lands.
4. **Q4 fixture policy:** every public language ships basic + stress
   fixtures, oracle goldens with `stoppedEarly: false`, exact + coarse
   parity, non-ASCII content where plausible.
5. Every optimization run includes the golden harness **and** a counter pass;
   perf wins that change output are rejected until fixtures prove the new
   output correct.

## 9. Phase C — concurrency (last; time is not the bottleneck today)

Mechanism: add **`rayon`** as a workspace dependency
(`[workspace.dependencies]`), used by `mark-diff` (C1/C2) behind its
data-parallel iterators — compute-only scoped joins with no async
interaction, so it composes with the existing tokio runtime. Cap the pool
(e.g. `min(cores, 8)`, never the global default pool from TUI runtime
threads); the non-TTY streaming path must not touch the pool at all.

Evaluation protocol (adopt only on evidence):

- Candidate wins: C1/C2 below; batch/static syntax rendering;
  `mark-bench syntax-compare`; per-file search-index construction after M2's
  lazy indexing exists.
- Non-candidates: parallelizing lines within one TextMate file; viewport
  rendering; global-pool work from tokio runtime threads.
- Controls: fixed worker counts (1, 2, 4, physical-core cap); record queue
  latency, p95 scroll, peak RSS, syntax cache memory, visible-job starvation.
- Gate: keep single-worker behavior as baseline; adopt only if settle time
  improves materially without worse scroll latency, memory, or
  cancellation/backpressure behavior.

Items:

- **C1: parallel parse.** Split the patch at `diff --git` boundaries (memchr
  scan), `par_iter` file sections into per-section `Vec<DiffFile>`,
  concatenate in order. Only worth it after M1 (parse is allocation-bound
  today). Expected: 310 ms → < 100 ms at 10 M lines.
- **C2: parallel search-index build / first grep.** Same file-section split,
  same rayon scope.
- **C3: N highlight workers.** Keep the existing dedicated-thread,
  priority-ordered queue model (a work-stealing pool adds nothing there);
  spawn `min(4, cores/2)` workers, one `SyntaxHighlighter` + tokenizer caches
  per worker, explicit queue limits, visible-priority preserved. Improves
  cold-scroll settle in wide multi-language diffs.

## 10. Sequencing summary

| Step | Gate |
| --- | --- |
| M1 span model | 10M model ≤ 250 MB over patch; load ≤ 200 ms |
| M2 search index (lazy, span-based) | index ≤ 100 MB; grep ≤ 56 ms |
| M3 UI rows | UiModel ≤ 150 MB; scroll unchanged |
| → G-MEM check | 10M peak RSS ≤ 3× patch bytes |
| M4 limits / M5 decode / M6 static streaming | explicit truncation; no lossy full-copy; pager streams |
| S1–S5 syntax budgets | LRU/queue byte budgets enforced; static assertion |
| B1–B4 bench + CI | tiers, RSS gate, counters gate, report ledger |
| U1–U4 UI scalability | wrap/emphasis windowed; random-jump benches green |
| E1 frame stack (+E6 diagnosability) | nix ≥ 8 MB/s |
| E2 aggregated prefilter (+E5 outliers) | php ≥ 6, cpp ≥ 15 MB/s |
| E3 bytecode remainder | libc++ ≥ 2.5, large-rust ≥ 12 MB/s |
| E4 frontier | sweep min ≥ 8 MB/s, markdown ≥ 15, TS cache-off ≥ 4 |
| C1–C3 rayon parallelism | 10M load < 100 ms; settle improved per protocol |

Deliberately out of scope: streaming/incremental parse of the interactive TUI
model beyond M6 (M1–M3 already put a 1-billion-line diff within a few GB),
semantic-token overlays, theme work, hand-written lexer shortcuts (coverage
stays an asset problem per the engine charter).
