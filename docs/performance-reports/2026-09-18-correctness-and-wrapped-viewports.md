# Correctness and wrapped-viewport performance — 2026-09-18

## Identities and scope

These identities describe the measurement snapshot, before committing or
integrating newer upstream commits.

- Branch: `main`; initial tree clean. No pre-existing changes, branch switches,
  resets, dependency changes, local dependency overrides, or commits made.
- Baseline: `56c2735558ebdf2a6158ff9d5c68d5fde3672b49`, isolated in a detached
  worktree under the artifact directory. Pristine workspace tests passed.
- Measured baseline: that commit **plus the same benchmark-only instrumentation
  used by the candidate**. `measurement.patch` adds the workload options and
  corrects syntax readiness; it contains no viewport or queue fixes.
  SHA-256: `e5278de6ba043d0a68079d4e3542857ff4943dc2ed37e01fde5fa047d4a620a2`.
- Candidate: uncommitted changes on that baseline. `candidate-source.patch`
  includes all changed/new Rust sources and tests, excluding documentation.
  SHA-256: `6ab510ac9ae7b791a44c4d3d47be4aa8d50c36b45191ba01b876c76d43642a6e`.
- Host: x86-64 NixOS/Linux 6.18.48, AMD Ryzen 9 9950X, 16 cores/32 threads.
  Rust 1.98.0 (`88d9e12ae`), Cargo 1.98.0, LLVM 22.1.8. Git 2.55.0 in
  the Nix checks; Git 2.54.0 in the host measurement environment.
- Both builds: ordinary release, mimalloc, `codegen-units=1`, thin LTO,
  `panic=abort`, no PGO or additional Rust flags. Allocation-profile binaries
  were built and measured separately. Syntaxmate remains registry version 0.1.2.
- Raw artifacts, scripts, binaries, fixture manifests, checks, and identities:
  `target/textmate-performance/reports/correctness-pass/` (called `$R` below).
- Reviewed CONTRIBUTING, development/CI/architecture docs, current CI scripts,
  recent rendering/allocation/syntax reports, and recent history including
  `0488db7`, `02675b5`, and `56c2735`. GitHub listed no open issues or PRs;
  the empty listings are saved in `$R/open-{issues,prs}.json`.

`mark-diff` owns parsing, span-backed payloads and changesets; `mark-git` owns
Git operations and `mark-core` shared errors/path utilities. `mark-tui` owns
layout, interaction, context/syntax scheduling and rendering. `mark-syntax`
adapts the published Syntaxmate API; its tokenizer was not duplicated or
modified. `mark-runtime` supplies the shared CPU pool/allocation counters;
`mark-session` and `mark-command` own protocol/transport and command/config
boundaries. Measurements extend `mark-bench`, not a new benchmark framework.

## Confirmed defects

### 1. Failed syntax admission silently discarded pending work

Reproducer: queue byte budget 10; enqueue visible work of 8 bytes and prefetch
work of 2 bytes; attempt visible work of 3 bytes. Admission fails, but previously
removed the prefetch job. Its runtime `pending` key was never removed because
an error cannot report evictions. Subsequent requests treated the missing job
as already pending, and the runtime could remain non-idle for that generation.

`syntax/queue.rs` now checks whether admission is possible after removing
prefetch work **before** modifying either queue. The bounded scan runs only
under queue pressure; successful admission retains the existing eviction order
and reporting. An individually oversized job likewise cannot destroy queued
work on rejection.

Regression commands, both observed failing before the fix:

```sh
cargo test -p mark-tui visible_job --locked
```

Tests: `rejected_visible_job_preserves_pending_prefetch_jobs` (runtime call
site) and `oversized_visible_job_does_not_evict_prefetch`. A further 495-case
admission test checks rejection atomicity, byte/entry bounds, exact eviction
reporting, generation changes, stale submission and close. Existing worker
shutdown, priority, and generation tests remain green. Red/green logs are in
`queue-red.log`, `queue-green.log`, and `benchmark-green.log`.

### 2. Promoted jobs retained incorrect priority metadata

A prefetch job moved into the visible queue still carried `Prefetch` in the job
sent to the worker. This misclassified results and first-visible latency.
Promotion now changes the job's priority, preserving its source key and original
queue timestamp. `promoted_job_reports_visible_priority` failed before the
one-line metadata correction (`promotion-red.log`) and passes now.

### 3. The syntax benchmark could finish before highlighting

The old benchmark settled only after cold scrolling. Random scrolling could
queue new work and return a report before it completed; shutdown could discard
queued work. An exploratory run reported 254 queued jobs but only about 200
completed. A focused reproduction with two files failed with **2 completed / 4
queued** (`benchmark-red.log`).

The benchmark now records additional first-frame syntax readiness separately,
then includes draining and repainting in syntax-enabled scroll samples. Repaint
can enqueue full-file-to-hunk fallback, so readiness is checked again. A
30-second timeout fails instead of silently reporting success. This waits for
queued prefetch work too and includes 1 ms polling granularity; these are
highlight-ready samples, not pure rendering measurements.

`syntax_benchmark_finishes_jobs_from_the_last_random_viewport` protects the
reproduction. All final syntax A/B samples completed every queued job with zero
failures, skips or evictions. The measurement correction is applied to **both**
binaries; the old incomplete timings are not used to claim a speedup.

## Retained optimization

`render/diff/viewport.rs` used to render a complete logical wrapped row and only
then apply `skip(row_offset).take(remaining)`. A pair of 256 KiB lines therefore
built thousands of invisible styled rows for each 40-row frame.

- `render/diff/wrapped.rs` owns wrapped-row dispatch, moved out of `diff.rs` to
  keep the architectural seam within its existing size budget.
- `unified.rs`, `split.rs`, and `context.rs` accept the requested continuation
  range and allocate/materialize only that range. Absolute continuation indexes
  still drive gutters, empty-pane decoration, grep offsets and visual mapping.
- `viewport.rs` passes the visible range before materialization. Annotation
  insertion, sticky overlays, cursor styles and selection snapshots stay in
  their existing paths.
- `run.rs`, `theme/benchmark.rs`, and `mark-bench/src/main.rs` expose
  `measure-patch --wrap-lines --annotations N --annotation-words N`. Annotation
  setup is charged to model open; wrapped scroll positions use visual rather
  than model rows. Defaults and syntax timing semantics are documented in
  `docs/development.md`.

No cache, unsafe code, new concurrency, or retained render working set was
introduced. **This is not constant-time layout:** wrap-start vectors still
scan/materialize all start columns, and fitting visible continuations can scan
their source prefixes. Expanded context can still copy a complete source line.
The change removes invisible styled-row construction, not every source-length
cost.

New differential coverage compares complete styled lines against their requested
windows over 48 generated Unicode/control/tab inputs, both layouts, nine widths,
empty/past-end windows, search highlighting and focused hunks. Additional tests
cover Syntaxmate-highlighted context and saved/draft annotation placement versus
viewport hit-testing plans across resizes. Existing selection, sticky-header,
wrapping/layout toggles, filtering, context/full-file and reload tests passed.

## Protocol and latency

Final runs use an isolated **empty** `XDG_CONFIG_HOME`, identical fixtures,
160×40 persistent TestBackend terminals, separate processes and alternating
A/B then B/A order. Default CPU/worker limits and syntax caps are unchanged.
These exercise the complete draw/buffer-diff path, not terminal I/O throughput.
Latency binaries have no allocation counters enabled. First-frame values time
the draw itself, not CLI-start-to-frame: load, model open and filter stages are
recorded separately. Syntax scroll positions include additional readiness paints.

Fixtures come from `mark-bench fixtures`: many-small-files,
syntax-many-small-rust, balanced-changeset and mega-diff-1m (74,041,831 patch
bytes / one million diff body rows). Additional inputs:

- Long wrapped: one 262,144-byte deletion and addition, ASCII, 524,367-byte patch.
  Syntax disabled; these lines exceed the unchanged 8 KiB syntax-line cap.
- Annotations: balanced fixture, up to 50 saved notes of 1,000 words each.
- Mixed syntax: complete fmt 11.2.0 `format.h` content named `format.cpp` to select
  C++, baseline `parser.rs` and `docs/usage.md`, and MDN's finished accessibility assessment
  HTML at the saved commit. Source total 203,967 bytes / 5,520 lines. Both the
  patch/TUI path and synchronous `syntax-compare` were measured. The latter
  produced exactly 99,270 tokens over two passes in every sample.

Medians; `n` is separate processes **per side**, not frames:

| Workload / metric | Before | After | Change | n |
| --- | ---: | ---: | ---: | ---: |
| Long wrapped first useful frame | 545.506 ms | 17.162 ms | **-96.9%** | 10 |
| Long wrapped cold scroll, 10 frames | 5,478.850 ms | 105.860 ms | **-98.1%** | 10 |
| Long wrapped warm scroll, 10 frames | 5,411.683 ms | 105.034 ms | **-98.1%** | 10 |
| Long wrapped random scroll, 10 frames | 5,404.399 ms | 138.258 ms | **-97.4%** | 10 |
| Long wrapped whole benchmark process | 16.821 s | 0.377 s | **-97.8%** | 10 |
| Many-small warm scroll, 200 frames | 24.476 ms | 24.319 ms | -0.6% | 20 |
| Balanced warm scroll, 200 frames | 22.324 ms | 22.328 ms | +0.02% | 20 |
| Mega-1m load | 43.631 ms | 43.755 ms | +0.3% | 12 |
| Mega-1m model open | 22.254 ms | 21.835 ms | -1.9% | 12 |
| Mega-1m warm scroll, 20 frames | 2.054 ms | 2.038 ms | -0.8% | 12 |
| Annotation-heavy warm scroll, 50 frames | 7.373 ms | 7.412 ms | +0.5% | 16 |
| Same long lines, unwrapped first frame | 0.278 ms | 0.272 ms | -2.0% | 12 |
| Syntax Rust initial additional readiness | 1.260 ms | 1.258 ms | -0.1% | 16 |
| Syntax Rust warm scroll, 20 positions | 3.779 ms | 3.851 ms | +1.9% | 16 |
| Mixed syntax initial additional readiness | 79.178 ms | 79.615 ms | +0.6% | 12 |
| Mixed syntax warm scroll, 20 positions | 3.525 ms | 3.522 ms | -0.1% | 12 |
| Synchronous mixed syntax, two complete passes | 99.635 ms | 99.572 ms | -0.06% | 8 |

Variability and tradeoffs:

- Long wrapped warm-pass IQR: **5,367.769–5,602.957 ms →
  103.952–107.640 ms**. Median worst warm frame: 557.251 → 10.724 ms;
  median worst random frame: 561.338 → 17.417 ms. These are medians of per-run
  maxima, not invented frame p95s. Individual frame distributions are not saved.
- Ordinary warm-control changes are small/mixed, not additional optimization
  claims. Syntax Rust warm IQR overlaps: 3.737–3.965 → 3.746–4.086 ms.
  Annotation warm IQR: 7.329–7.490 → 7.351–7.515 ms.
- Opening is not universally faster: the primary many-small batch measured
  1.197 → 1.339 ms (+11.9%). A separate **40-pair** recheck measured
  1.327 → 1.312 ms, so that opening slowdown did not reproduce. The primary
  many-small median worst cold frame increased 0.234 → 0.306 ms; the recheck
  was 0.307 → 0.325 ms with overlapping IQRs. No normal-tail improvement is claimed.
- Long-line model open measured 1.010 → 1.140 ms (+0.131 ms), included rather
  than hidden in setup. No wrap cache/precomputation was moved into model open;
  the first-frame gain alone is over 528 ms.
- Full metric medians, IQRs, ranges and paired changes are in `summary.json`;
  `recheck-many-small/` retains the additional control samples.

## Memory and allocation

RSS uses ordinary release binaries and **five** alternating GNU Time 1.10 pairs.
Small-process peaks from Python's `wait4` can inherit the launcher's high-water
floor; those latency `.meta` peaks are not used for the RSS conclusions below.

| Peak process RSS | Before | After | Change |
| --- | ---: | ---: | ---: |
| Long wrapped | 9.031 MiB | 7.254 MiB | **-19.7%** |
| Mega-1m | 195.059 MiB | 194.547 MiB | -0.3% |
| Annotation-heavy | 11.773 MiB | 12.039 MiB | +2.3% |
| Mixed syntax | 51.543 MiB | 51.313 MiB | -0.4% |

Separate allocation-profile builds, three alternating process pairs per fixture:

| Long wrapped complete run | Before | After | Change |
| --- | ---: | ---: | ---: |
| Allocation calls | 594,515 | 21,519 | **-96.4%** |
| Reallocation calls | 91,990 | 3,124 | **-96.6%** |
| Cumulative requested bytes (decimal MB) | 138.719 | 10.876 | **-92.2%** |
| Peak live-byte increase (decimal MB) | 3.768 | 1.586 | **-57.9%** |
| End-of-run live-byte delta | 1,600 B | 1,600 B | unchanged |

These are process-wide counters, including the existing 1 ms thread-census
observer and any overlapping workers. Faster runs also perform fewer census
allocations; the churn reduction is **not** pure renderer-stack attribution.
Warm-stage live-byte delta is zero on both sides. No extra retained cache buys
the gain. Many-small complete-run allocation calls and churn are identical;
mega, annotation and mixed-syntax churn changes are under 0.01%, with unchanged
peak live growth. Per-stage counters remain in `allocation/` and `summary.json`.

## Commands and artifacts

All Rust commands ran through `nix develop --command`; no global tools installed.
The source/fixture preparation commands are recorded in `prepare-fixtures.sh`.
Representative commands (run separately against `baseline-bench` and
`candidate-bench`, not an allocation-profile binary):

```sh
R=target/textmate-performance/reports/correctness-pass
cargo build -p mark-bench -p mark-cli --release --locked
cargo build -p mark-bench --release --features allocation-profile --locked

XDG_CONFIG_HOME="$PWD/$R/empty-config" "$R/bin/baseline-bench" \
  measure-patch "$R/fixtures/long-wrapped.diff" --wrap-lines \
  --max-scroll-steps 10 --json
XDG_CONFIG_HOME="$PWD/$R/empty-config" "$R/bin/candidate-bench" \
  measure-patch "$R/fixtures/balanced-changeset/patch.diff" \
  --annotations 50 --annotation-words 1000 --max-scroll-steps 50 --json

python3 "$R/run_ab.py"
python3 "$R/run_alloc.py"
python3 "$R/run_rss.py"
AB_OUTPUT=recheck-many-small AB_SAMPLES=40 python3 "$R/run_ab.py" many-small
python3 "$R/summarize.py"
```

`build_comparisons.sh` records both worktree builds and binary capture before
switching features. Every measurement `.meta` records its full command and
configuration directory. `sha256sums.txt`, `fixture-sha256sums.txt`,
`identities.json`, `toolchain.txt`, and the source patches identify the inputs.

## Correctness coverage and verification

Risk-ranked invariants and evidence:

1. **Queue ownership / readiness:** no pending key without queued/in-flight work;
   atomic rejection, reported eviction, bounded admission, generation rejection,
   close and worker shutdown. New reproductions/property coverage above.
2. **Coordinates / rendering:** graphemes, UTF-8, terminal columns, partial tabs,
   continuation gutters, split asymmetry, focus/search styles and annotation
   placement. New window/plan tests plus existing resize, selection, layout,
   filter, full-file/context and reload transition tests.
3. **Parsing / source identity:** a new Git differential fixture compares string
   and byte parsers with `git diff --numstat -z -M` and original old/new line
   bytes. It includes additions/deletions, empty files, renames, quoted/tab/Unicode
   paths, binary entries, CRLF and missing final newlines. Existing malformed
   input, byte preservation and resource-limit tests also passed. No parser
   optimization was warranted by these controls.
4. **Session / human state:** existing isolation, stale-generation, atomic batch,
   bounded pagination/invalid cursor, multibyte continuation and human-comment
   preservation tests passed. No session behavior or serialization optimization
   was made without a measured bottleneck.

Passed, with logs/exit codes in `$R/checks/`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
scripts/check-architecture
just ci-performance
just ci-check
scripts/ci/performance extended
```

The candidate has 714 `mark-tui` tests and 66 `mark-diff` unit tests passing.
The canonical CI aggregate also ran installer/update, generated-theme and
workflow checks. No pre-existing failing canonical check was observed.
Both archived release `mark --version` binaries passed `scripts/check-startup
<binary> 2.0` (25 measured launches each); this smoke is not a startup speedup
claim.

`scripts/test-diff-error-pane` passed under a controlled PTY: invalid configuration,
Escape without quit, failed editor launch, resize, failed reload, clean exit.
`terminal_smoke.py` and transcripts are saved. Two initial driver attempts needed
cursor-position-query handling and whitespace-normalized assertions for
cursor-addressed output; these were harness limitations, not product fixes.
Clipboard integration and a real editor/multiplexer matrix were not exercised.

Only x86-64 Linux was tested. ARM Linux, both macOS architectures, Rust 1.88 MSRV,
rust-analyzer, 10M diffs, real terminal I/O throughput and reload latency A/Bs
were not run. At measurement time `docs/agent-review-evals.md` was absent; the
available `docs/live-agent-review-demo.md` and protocol tests were consulted.
The evaluation document was added upstream and read during integration below.
**No real-agent behavioral evaluation is claimed.**

### Upstream integration before push

The first push found two upstream commits not in the local baseline:
`916eb20` (agent-review workflow/evaluations) and `7c4609c` (release 0.14.1).
The unpublished change was rebased onto them without conflicts. The TUI, parser,
and benchmark crate files are byte-identical to the measured candidate; upstream
changed the bundled skill, its CLI tests, documentation, and workspace versions.
The measurements above remain the archived 0.14.0 comparison, **not a fresh
benchmark of the rebased 0.14.1 tree**. All seven canonical verification commands
above passed again after integration; logs are separate under
`$R/post-integration-checks/`.

## Decisions and remaining opportunities

Keep the large, attributable wrapped-rendering improvement and the three
correctness fixes. Ordinary views and syntax throughput are controls, not
additional speedup claims. No speculative production optimization was added and
then reverted. Exploratory batches with incomplete syntax timing or inherited
local configuration are retained under `exploratory/`, excluded from the tables.

Remaining opportunities, in priority order:

1. Stream/window wrap-start discovery and avoid rescanning the same prefix for
   each visible continuation. Preserve tab fragments, grapheme boundaries,
   split-side exhaustion and selection mapping; do not promise constant-time
   random access without an index and its retention/invalidation cost.
2. Profile long-note formatting and draft copies before bounding annotation
   materialization. The 1,000-word-note control is already about 0.15 ms/frame;
   a cache is not justified just by the existence of repeated work.
3. Attribute pressure-driven syntax source rebuilding and obsolete in-flight
   work. Keep tokenizer preparation/cancellation/engine reuse inside Syntaxmate
   APIs; do not duplicate engine internals in Mark.
4. Measure full-file context copies and large live-session requests independently
   before changing their lazy loading, generation identity or pagination rules.
