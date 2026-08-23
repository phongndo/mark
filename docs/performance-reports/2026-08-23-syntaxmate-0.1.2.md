# Syntaxmate 0.1.2 and dependency refresh — 2026-08-23

- Baseline: Mark `02675b5` with crates.io `syntaxmate 0.1.1`.
- Candidate: the same Mark source with only crates.io `syntaxmate 0.1.2` advanced.
- Syntaxmate release commit: `d969efc80923f8977cfff3c936197f0ff9873924`.
- Host: arm64 macOS 26.5.2, 16 logical CPUs, 64 GiB RAM.
- Rust/Cargo: 1.98.0.
- Raw artifacts: `target/syntaxmate-0.1.2-eval/` (ignored).

## Protocol

Latency used independent ordinary-release `mark-bench` binaries linked against
0.1.1 and 0.1.2. The binaries were captured before refreshing unrelated
packages so the A/B attributes only Syntaxmate. Each corpus had 15
alternating-order, separate-process pairs.
The measured command was `mark-bench syntax-compare --file <fixture>
--iterations 3 --skip-counters --json`; source collection is outside the timed
highlight region and the normal mimalloc allocator was used.

Allocation efficiency used independent release binaries with Mark's opt-in
profiling allocator wrapping mimalloc. Seven separate-process pairs alternated
order on the deterministic 240-file `syntax-many-small-rust` fixture (455,004
patch bytes), with 200 measured scroll positions. Every measured syntax run
queued and completed 480 jobs with zero failures. Allocation instrumentation
was not used for the latency results.

## Highlight latency

Medians over 15 samples; token counts cover the three highlight passes.

| Corpus | Bytes | 0.1.1 median | 0.1.2 median | Change | Tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| Markdown stress | 5,088 | 17.566 ms | 17.769 ms | +1.2% | 2,493 |
| C++ stress | 11,814 | 15.855 ms | 15.785 ms | -0.4% | 8,091 |
| Rust stress | 5,951 | 1.677 ms | 1.688 ms | +0.7% | 6,375 |
| HTML stress | 9,431 | 13.291 ms | 13.205 ms | -0.6% | 8,178 |

The mixed changes, all within 1.2%, show no material latency shift in Mark's
owned whole-document API. Setup remained 10 microseconds at the median for both
versions on every corpus. The release is therefore latency-neutral in this
integration rather than a throughput optimization claim.

## Allocation and efficiency impact

Medians over seven syntax-enabled product runs. `Churn` is cumulative requested
allocation bytes, not retained memory.

| Metric | 0.1.1 | 0.1.2 | Change |
| --- | ---: | ---: | ---: |
| Complete run allocation calls | 1,886,071 | 1,775,399 | **-5.9%** |
| Complete run alloc + realloc operations | 1,941,502 | 1,830,830 | **-5.7%** |
| Complete run churn | 190.74 MiB | 181.82 MiB | **-4.7%** |
| Cold-scroll alloc + realloc operations | 850,453 | 759,472 | **-10.7%** |
| Cold-scroll churn | 91.26 MiB | 83.91 MiB | **-8.1%** |
| Peak live-byte increase | 22.20 MiB | 22.18 MiB | -0.1% |
| End-of-run live-byte delta | 3.54 MiB | 3.54 MiB | 0.0% |

Reallocation calls were flat (55,431 versus 55,432 at the median), and model-open
churn was byte-identical at 4,623,205 bytes. The gain is concentrated in the
cold scroll stage where syntax work is first requested. This agrees with
Syntaxmate 0.1.2's deferred group-zero synthesis, direct compact-capture output,
bounded buffer reuse, and compact candidate indexes: it removes transient work
without retaining more memory.

The bundled grammar asset is byte-identical across the releases (2,274,428
bytes, SHA-256 `1d509a0683b450d09ac44d66cfdd2078b9ebcbaf3636df0a88d6b3276f166827`).
The ordinary release benchmark binary grew by 16,608 bytes (+0.22%); this is a
small code-size tradeoff rather than bundle growth.

## Correctness and dependency state

- All four corpus token counts are identical between 0.1.1 and 0.1.2.
- Mark resolves one registry-sourced Syntaxmate package with no path, Git, or
  patch override, and distribution notices now come from the published 0.1.2
  crate.
- The Rust lockfile also advances `cc` 1.4.3 -> 1.4.4, `log` 0.4.33 -> 0.4.34,
  and `uuid` 1.24.1 -> 1.25.0. `cargo update --dry-run` has no remaining
  resolvable update; `generic-array` 0.14.7 is held by `crypto-common`'s exact
  transitive requirement.
- Development dependencies are refreshed: `@shikijs/themes` 3.23.0 -> 4.4.3
  (with regenerated, rehashed vendored themes), the Shiki comparator to 4.4.3,
  hk 1.49.0 -> 1.56.1, Go 1.25.11 -> 1.27.0 for workflow linting,
  current Nix inputs, and current pinned GitHub Action releases.
  `github-vscode-themes` 6.3.4, actionlint 1.7.12, and cargo-audit 0.22.2
  were already current. `npm outdated` reports no remaining package update.
- `cargo test --workspace --all-targets --all-features --locked`, workspace
  Clippy with warnings denied, the Rust 1.88 MSRV check, cargo-audit with
  warnings denied, `scripts/check-architecture`, and
  `scripts/ci/performance smoke` pass.
- Vendored-theme regeneration/checks, workflow actionlint, Nix flake
  evaluation, hk configuration validation, Shiki 4.4.3 comparison smoke, and
  distribution notice packaging pass.

## Decision

Keep Syntaxmate 0.1.2. It preserves Mark's measured token output and latency
while reducing allocation operations by 5.7% and byte churn by 4.7% over a
complete syntax-enabled run, with double-digit operation reduction in the
cold syntax stage and no retained-memory regression.
