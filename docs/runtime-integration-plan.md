# Tokio + rayon runtime integration plan

Elaborates Phase C of `docs/performance-plan.md` (§9) into a concrete runtime
architecture: one tokio runtime for I/O and coordination, one shared capped
rayon pool for CPU-bound data parallelism, dedicated threads only where a
priority queue needs them (syntax workers). Goal: turn the already-landed
parallel machinery on by default, remove per-call pool construction and ad-hoc
thread spawning, and put every thread in the process under a single documented
budget — without regressing startup (2 ms), the non-TTY streaming path, memory
(G-MEM), or scroll latency (G-SCROLL).

## 1. Current state (audited 2026-07-12, branch `feat/rust-textmate-engine`, working tree)

### 1.1 Tokio (mark-tui only)

- `crates/mark-tui/src/runtime.rs` owns a multi-thread runtime: **2 workers,
  4 max blocking threads**, `enable_time()` only. Entry:
  `run.rs::run_diff_with_options` → `runtime::block_on`.
- `block_on` has a nested-runtime escape hatch: when called from inside a
  runtime it spawns an OS thread that **builds a second throwaway runtime**.
- `spawn_detached_blocking` spawns a **raw OS thread per call**; six
  production call sites (`app/diff_load.rs`, `app/diff_load/prefetch.rs`,
  `app/diff_load/review.rs`, `app/editor_reload.rs`, `app/filters.rs`,
  `live_diff.rs` via `run_detached_blocking`). Rationale (documented by the
  `detached_blocking_does_not_hold_current_runtime_open` test): tokio runtime
  drop waits for `spawn_blocking` tasks, and a stuck git subprocess must not
  hang exit.
- `send_with_timeout` backpressures sync workers with a 1 ms sleep-poll loop
  (bounded at 10 ms).
- The event loop (`app/runner.rs::run_loop`) is a frame loop that awaits
  `TerminalEventReader::read_timeout`; everything else it does per frame is
  synchronous draining.

### 1.2 Rayon (mark-diff only)

- `parser.rs::parse_patch_bytes_parallel` splits at `diff --git` boundaries
  and `par_iter`s sections — but it **builds a fresh
  `rayon::ThreadPoolBuilder` pool on every parse call**, reads
  `MARK_DIFF_PARSE_THREADS` from the environment on every call, and the
  **default is 1, i.e. parallel parse is off** for every real user.
- Phase M landed (span-based model; parse is no longer allocation-bound), so
  the §9 precondition for C1 ("only worth it after M1") is now satisfied.
  The measured serial 10 M-line load is ~200–310 ms; §10's gate is < 100 ms.

### 1.3 Dedicated threads

- Syntax workers: `syntax/runtime.rs` spawns `worker_threads`
  (default `cores/2` clamped to 1–4) plain `std::thread`s over the shared
  priority `SyntaxWorkerQueue`; results return through a tokio mpsc into the
  frame loop. Workers do **git subprocess I/O and CPU tokenization on the
  same thread** (full-file source fetch is deliberately worker-side so
  queueing never blocks on I/O).
- `mark-git` is entirely synchronous `std::process::Command`, always invoked
  from blocking/detached threads. This is fine and stays.

### 1.4 The problem, stated

1. The one measured multi-hundred-ms win (parallel parse) is **disabled by
   default** and pays pool construction per call when enabled.
2. Thread creation is uncoordinated: 2 tokio workers + ≤4 blocking + N
   detached threads (one per in-flight load/reload/prefetch/filter) + ≤4
   syntax workers + ≤8 per-call rayon threads. A live-reload storm on an
   8-core machine can transiently run ~18+ threads with no shared budget.
3. Runtime plumbing has smells that will bite future work: throwaway nested
   runtimes, per-call env reads, sleep-poll backpressure, unnamed detached
   threads invisible to profiling.

Time is still not the whole-binary bottleneck (memory was, and Phase M fixed
it), so this plan is deliberately narrow: consolidation + default-on for
proven wins + evidence-gated candidates. No speculative parallelism; the §9
non-candidates (per-line TextMate parallelism, viewport rendering, global-pool
work from tokio runtime threads) and the reverted-experiment blacklist in
`textmate-engine.md` remain in force.

## 2. Target architecture

```
                    ┌──────────────────────────────────────────┐
                    │ tokio runtime (process-global, 2 workers) │
                    │  event loop · timers · channels · watch   │
                    └──────┬──────────────┬────────────────────┘
        spawn_blocking /   │              │  cpu() bridge (spawn_fifo + oneshot)
        managed blocking   │              │
                    ┌──────▼──────┐  ┌────▼─────────────────────┐
                    │ blocking     │  │ shared rayon pool         │
                    │ tier (git,   │  │ min(physical cores, 8),   │
                    │ fs, editor)  │  │ lazy, named mark-cpu-N    │
                    └─────────────┘  │  parse · search index ·   │
                                     │  future batch work        │
                                     └───────────────────────────┘
                    ┌──────────────────────────────────────────┐
                    │ syntax workers (dedicated threads, ≤4,    │
                    │ priority queue — unchanged model)         │
                    └──────────────────────────────────────────┘
```

Role rules (enforced by review, asserted where cheap):

- **tokio** owns wall-clock waiting: terminal events, debounce timers,
  channel coordination, subprocess *waiting* (via the blocking tier).
- **rayon (shared pool)** owns CPU-bound divisible work: parse sections,
  search-index sections, future `mark-bench` batch jobs. Never entered from a
  tokio worker thread by `install` — always bridged (spawn from a blocking
  context, or `spawn_fifo` + oneshot).
- **Dedicated threads** only for the syntax priority queue (work stealing
  adds nothing to a priority-ordered queue, per §9 C3) and the terminal event
  reader.
- The **non-TTY streaming path and `mark --version` must never construct
  either pool.** Lazy initialization makes this structural, and R6 adds an
  assertion.

## 3. Work items

### R1. `mark-runtime` shared-pool foundation

Add a small crate (or module in `mark-core` if a crate feels heavy — decide by
dependency direction: `mark-diff` must reach it without depending on
`mark-tui`) exposing:

- `cpu_pool() -> &'static rayon::ThreadPool` — `OnceLock`, built on first
  use with `num_threads = min(physical cores, 8)`, threads named
  `mark-cpu-{i}`. Env override `MARK_CPU_THREADS` (0/1 = serial) replaces
  `MARK_DIFF_PARSE_THREADS`, read **once**.
- `run_cpu(f) -> impl Future` — bridges async → pool via `spawn_fifo` + tokio
  oneshot, so tokio workers never block on CPU work.
- `is_cpu_pool_started() -> bool` — for the R6 assertions.

Rules: nothing may call `rayon::join`/`par_iter` on rayon's implicit global
pool — repo grep check in `scripts/check-architecture` (`rayon::` usage
allowed only under `pool.install`/`spawn_fifo` through this module).

Acceptance: `mark --version` still ≤ 2 ms; non-TTY 223 MB pipe run shows pool
never started; thread names visible in `sample`.

### R2. Default-on parallel parse (C1 — the headline win)

- `parse_patch_bytes_parallel` takes the shared pool instead of building one;
  threads default to pool size; the `PARALLEL_PARSE_MIN_BYTES = 8 MiB` and
  `sections >= 2` guards stay (small/single-file patches never touch the
  pool).
- Callers already run parse on blocking threads (`spawn_blocking` /
  detached), so `pool.install` there is safe.
- Keep the serial path as the fallback and as the A/B baseline.

Acceptance (per §9 protocol: fixed thread counts 1/2/4/cap, alternating-order
separate-process runs, paired medians):

- 10 M-tier load < 100 ms (from ~200–310 ms serial).
- Byte-identical `Vec<DiffFile>` vs serial parse on the full fixture suite
  (order-preserving concat already guarantees this — add the test).
- Peak RSS delta vs serial ≤ 5% (per-section `Vec<DiffFile>` staging is
  bounded by section sizes).
- No regression on the 100 K tier (guard rails keep it serial anyway).

### R3. Parallel first-grep / search-index build (C2 — evidence-gated)

Interactive grep is already 56 ms against a 250 ms gate, so this is *not*
urgent; do it only if R2's section-splitting infrastructure makes it nearly
free or if the 10 M first-grep p95 exceeds ~100 ms in R0 measurement. Same
file-section split, same shared pool, built from the existing lazy-index
trigger (still constructed on first search, never at open), cancelable
between sections via the existing generation check.

Acceptance: first grep on 10 M tier ≤ 100 ms, zero cost when search unused,
no RSS regression.

### R4. Tokio runtime consolidation

1. **One runtime.** `block_on` uses `global_runtime()` instead of building a
   throwaway runtime per top-level call; the nested-runtime-on-a-thread path
   stays only as a guarded fallback for tests that call `block_on` from
   inside a runtime.
2. **Fix the shutdown root cause instead of routing around it.** The detached
   raw threads exist because runtime drop waits for blocking tasks. Switch
   exit to `runtime.shutdown_timeout(Duration::from_millis(250))` (or
   `shutdown_background()`), then migrate the six `spawn_detached_blocking`
   call sites to `spawn_blocking` with `max_blocking_threads` raised to 8.
   This makes load/reload/prefetch/filter workers named, budgeted, and
   visible to tokio metrics. Keep `spawn_detached_blocking` only if a call
   site genuinely must outlive the runtime (editor launch may qualify —
   audit each site; document survivors).
3. **Backpressure.** Replace `send_with_timeout`'s sleep-poll with
   `blocking_send` on a `Handle`-aware path or leave as-is if the 10 ms bound
   never fires in practice — measure first (add a counter), don't churn.

Acceptance: clean exit ≤ 300 ms even with a hung `git` child (regression test
mirroring the existing detached-thread test); no behavior change in
load/reload tests; thread census during a reload storm shows only named,
budgeted threads.

### R5. Syntax runtime — keep the model, split I/O only if metrics demand

The dedicated-thread priority-queue model stays (§9 C3 verdict). The one
integration candidate: full-file source fetch (`git show`, size checks) runs
on the same thread that tokenizes, so a slow fetch can stall a CPU worker
while visible-priority hunk jobs wait. If S4 queue-latency metrics show
fetch-bound starvation (fetch latency a significant share of
visible-priority p95), split the worker pipeline: fetch stage on the tokio
blocking tier → tokenize stage on the syntax worker. Otherwise: no change.

### R6. Thread budget, observability, and CI

- Document the budget in `docs/architecture.md`: tokio 2 workers + ≤8
  blocking, rayon ≤ min(cores, 8) shared by all CPU work, syntax ≤4, event
  reader 1. Worst-case concurrent CPU-hungry threads ≤ cores + small
  constant; pools are shared, not stacked.
- `mark-bench measure`/`measure-patch` gain a peak-thread-count column
  (macOS `proc_pidinfo` / Linux `/proc/self/status` Threads) next to peak
  RSS.
- CI additions to the §7 B3 jobs: parallel-vs-serial parse equivalence test;
  startup-time check; non-TTY pool-never-started assertion
  (`is_cpu_pool_started()` + debug assert on the streaming path); exit-with-
  hung-child test.

## 4. Sequencing and gates

| Step | Gate |
| --- | --- |
| R0 baseline census | thread counts + first-grep p95 recorded on 1 M/10 M tiers (one `mark-bench` run, feeds R3/R5 decisions) |
| R1 shared pool | startup ≤ 2 ms; non-TTY pool untouched; named threads |
| R2 parallel parse default-on | 10 M load < 100 ms; output byte-identical; RSS Δ ≤ 5% |
| → re-run mega-diff release gate | G-MEM/G-OPEN/G-SCROLL/G-SEARCH all still green |
| R4 runtime consolidation | exit ≤ 300 ms with hung child; budgeted named threads |
| R6 budget + CI | census in bench reports; CI equivalence + startup + streaming assertions |
| R3 parallel first-grep | only if R0 shows first-grep p95 > ~100 ms; then ≤ 100 ms |
| R5 syntax fetch/tokenize split | only if S4 metrics show fetch-bound visible-priority starvation |

R1+R2 are the payload and land together. R4 and R6 are independent cleanups.
R3 and R5 are decided by R0/S4 evidence, not by default.

## 5. Risks and mitigations

- **Oversubscription during reload storms** — shared pool + blocking-tier cap
  + R6 census gate; syntax workers already capped at 4.
- **Deadlock via `install` from a tokio worker** — forbidden by role rules;
  the only async entry is the `run_cpu` bridge; grep check in
  `check-architecture`.
- **Startup/streaming regression from pool construction** — lazy `OnceLock`
  init plus explicit CI assertions (R6).
- **Parallel parse output divergence** — order-preserving concat + CI
  equivalence test on the full fixture suite; serial path retained.
- **Shutdown_timeout masking real leaks** — the hung-child regression test
  distinguishes "git is stuck" (acceptable, timed out) from "our worker
  deadlocked" (test fails under the serial baseline too).
- **Cross-crate dependency tangle** — the pool lives below `mark-diff` and
  `mark-tui` in the crate graph; neither may reach into the other's runtime.

## 6. Explicit non-goals

Unchanged from §9: no per-line parallelism inside one TextMate file, no
parallel viewport rendering, no async rewrite of `mark-git`, no work-stealing
replacement for the syntax priority queue, and the non-TTY path stays
pool-free and O(buffer). Engine throughput work continues under Phase E on
its own track — this plan does not touch the tokenizer hot path.
