# Runtime integration performance report — 2026-07-12

- Commit: `bbc8e3f` plus the runtime-integration working tree
- Host: arm64 macOS 26.5.2, 16 physical cores (12 performance + 4 efficiency), 64 GiB RAM
- Rust: 1.88.0
- Raw artifacts: `target/tmp-mark-bench-fixtures/runtime-{baseline,integrated}-*.json`

## R0 baseline

The pre-integration release binary was sampled as a separate process. Peak
threads were collected externally with `ps -M`; all other fields are from
`mark-bench measure-patch`.

| Corpus | Load | First grep | Peak threads | RSS delta |
| --- | ---: | ---: | ---: | ---: |
| mega-diff-1m | 24.0 ms | 22.6 ms | 2 | 109.2 MB |
| mega-diff-10m | 281.2 ms | 228.9 ms | 2 | 1.064 GB |

The 10M first-grep result exceeded the 100 ms evidence threshold, so R3 was
activated. Existing syntax latency data did not show fetch-bound
visible-priority starvation, so the conditional R5 fetch/tokenize split was
not activated.

## Integrated result

Release build, default `MARK_CPU_THREADS` (shared pool capped at 8), one run per
tier. The benchmark's new peak-thread sampler excludes its own census thread.

| Corpus | Load | Open | First grep | Random-scroll max | Peak threads | RSS delta |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| mega-diff-100k | 2.9 ms | 17.1 ms | 3.0 ms | 81 µs | 1 | 10.8 MB |
| mega-diff-1m | 14.8 ms | 18.2 ms | 3.7 ms | 66 µs | 9 | 101.1 MB |
| mega-diff-10m | 89.3 ms paired p50 | 121.7 ms | 30.5 ms | 79 µs | 9 | 983.6 MB |

- R3 passes: 10M first grep is 30.5 ms, below 100 ms, and parallel search adds
  no work when grep is unused.
- R2 passes: the paired 10M load median is 89.3 ms, below 100 ms. The
  task-local span backing removed cross-worker Arc-counter contention while
  also reducing line-model memory.
- G-MEM passes: the 10M RSS delta decreased by approximately 7.5% and remains
  well below the 3x patch-size gate.
- G-OPEN/G-SCROLL remain within their existing gates.
- The 100K parser guard remains serial and does not start the CPU pool.

## Fixed-thread parser/search sweep

Separate-process 10M runs, in alternating order:

| CPU threads | Load observations | First-grep observations | Peak threads |
| ---: | --- | --- | ---: |
| 1 | 173.4 ms | 211.1 ms | 2 |
| 2 | 119.0 ms | 108.2 ms | 3 |
| 4 | 86.7 ms | 59.9 ms | 5 |
| 8/cap | 89.3 ms paired p50 | 30.5 ms | 9 |

Parallel parse is enabled by default, uses the shared named pool, preserves
output order, and stays within the RSS gate. Parser division is capped at four
byte-balanced tasks after the sweep showed that more parse tasks saturate this
host; the eight-thread shared pool remains available to grep and other batch
work. `MARK_CPU_THREADS=1` remains the serial A/B and operational fallback.

## Startup and shutdown

- `scripts/check-startup target/release/mark 2.0`: adjusted median 1.24 ms
  (2.57 ms raw process time minus 1.33 ms process-runner baseline).
- The runtime regression test bounds shutdown of a runtime with a blocked
  blocking worker to 300 ms; the process-global runtime intentionally lives
  until process exit, so blocked Git work cannot make runtime drop hang.
