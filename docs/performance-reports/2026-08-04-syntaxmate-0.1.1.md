# Syntaxmate 0.1.1 integration report — 2026-08-04

- Baseline: Mark `f70ed87` with crates.io `syntaxmate 0.1.0`
- Candidate: Mark working tree with crates.io `syntaxmate 0.1.1`
- Syntaxmate release commit: `653aee9`
- Host: local macOS development machine
- Protocol: seven alternating-order, separate-process samples per corpus
- Build: independent optimized `mark-bench` release binaries
- Measured command: `mark-bench syntax-compare --file <fixture> --iterations 3 --skip-counters --json`
- Raw artifact: `target/syntaxmate-0.1.1-mark-comparison.json`

| Corpus | Bytes | 0.1.0 median | 0.1.1 median | Change | Tokens over 3 passes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Markdown stress | 5,088 | 37.281 ms | 15.691 ms | **-57.9%** | 2,493 |
| C++ stress | 11,814 | 21.477 ms | 14.341 ms | **-33.2%** | 8,091 |
| Rust stress | 5,951 | 1.695 ms | 1.467 ms | **-13.5%** | 6,375 |
| HTML stress | 9,431 | 14.413 ms | 11.815 ms | **-18.0%** | 8,178 |

## Findings

- Mark resolves exactly one registry-sourced `syntaxmate 0.1.1`; no path, Git,
  or patch override is present.
- Highlight token counts are unchanged between 0.1.0 and 0.1.1 on every measured
  corpus. Syntaxmate's release gates additionally preserve complete TextMate
  golden scope streams and UTF-8 byte ranges.
- The measured Mark path uses structured whole-document highlighting, so it
  directly benefits from compiled grammar IR, dense repository contexts,
  shared regex analysis, and compact bytecode. Reusable line sinks, direct
  HTML/ANSI rendering, and incremental theme caching remain available for
  future call sites but are not claimed in this measurement.
- The 0.1.1 performance guardrails are development-only and add no production
  instrumentation to Mark.

## Correctness and acceptance gates

- `cargo test -p mark-syntax --all-targets --all-features --locked`: 67 passed.
- `scripts/check-architecture`: passed and confirmed the crates.io dependency
  boundary.
- Mark's copied grammar license notices are byte-identical to the notices in the
  published `syntaxmate 0.1.1` crate.
- Workspace formatting, Clippy with warnings denied, and all-target/all-feature
  tests passed; `mark-syntax` passed 67 tests and `mark-tui` passed 605 tests.
- `scripts/ci/performance smoke` passed, including 240 completed Rust syntax
  jobs with zero failures and the mega-diff memory/rendering scenario.
- Distribution packaging includes the renamed `syntaxmate-0.1.1` notices.

## Decision

Keep `syntaxmate 0.1.1`. All representative Mark highlighting medians improve,
token counts remain stable, and the dependency continues to satisfy Mark's
registry-only architecture boundary.
