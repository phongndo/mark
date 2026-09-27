# Syntaxmate 0.2.0 migration measurements

Measured **2026-09-27**, comparing Mark at
`738599df777cb2c5f8359da2599e2a0f962f010a` with syntaxmate **0.1.3** against
that revision plus this migration to **0.2.0**. The
[recorded samples](syntaxmate-0.2.0.json) contain build and corpus hashes,
per-process timings, memory measurements, and benchmark counters.

First-pass highlighting improved by 12–61% on the six selected files. Large
Rust diff syntax readiness improved by 30%, and measured peak RSS decreased.
Repeated Rust highlighting regressed by 10%; Nix's smaller 4% difference had
overlapping ranges. Warm scrolling changed little on these fixtures. These
results describe the selected workloads, not every bundled grammar.

## Setup and scope

- Host: `z`, NixOS 26.05, x86-64, Ryzen 9 9950X (16 cores / 32 threads).
- Toolchain: the repository's `nix develop` shell, rustc 1.98.1, LLVM 22.1.8.
- Ordinary release `mark-bench` binaries: mimalloc, thin LTO, one codegen unit,
  `panic=abort`; no PGO or allocation profiler. Both builds include diagnostics;
  raw syntax runs use `--skip-counters` and leave engine counters disabled.
- Twelve fresh processes per version per case, alternating before/after and
  after/before. Runs were sequential, with no concurrent builds or tests. CPU
  affinity and governor were unchanged; filesystem caches were not flushed.
- An isolated `XDG_CONFIG_HOME` prevents personal settings from affecting inputs.
  The TUI harness intentionally bypasses user theme settings. The recorded TUI
  runs explicitly set `MARK_TEXTMATE_BENCH_THEME=github-dark`; an earlier
  unthemed pilot is excluded. The configured scope override is also bypassed
  by this harness, so its correctness was checked separately with tests and
  `mark syntax inspect`.
- TUI options: split view, 160 columns, 40 rows, 20 scroll positions, step 20.
  Each scenario runs plain mode followed by syntax mode in one fresh process.
  These are TestBackend frames, not terminal I/O or CLI-to-first-frame timings.

Tables show **median (minimum–maximum)** across twelve processes. Changes use
unrounded medians; negative latency or memory changes are improvements.
The ranges describe observed variability, not confidence intervals.

## Synchronous highlighting

Inputs are Mark's baseline `crates/mark-syntax/src/types.rs` (40,075 bytes) and
five [Syntaxmate fixtures pinned to `ebc81f77`](https://github.com/phongndo/syntaxmate/tree/ebc81f77db38471e088b935ba48334c997c7d00c/tests/fixtures/textmate):
`cpp/libcxx_vector.cpp` (1,185 bytes), `typescript/stress.ts` (5,903),
`tsx/stress.tsx` (6,499), `markdown/stress.md` (5,088), and `nix/stress.nix`
(5,854). Both binaries read identical copies; hashes are in the sample file.

The first pass includes lazy tokenizer construction, tokenization, and Mark's
scope adaptation. Catalog detection and source-file I/O happen outside this
timer. It does not measure complete cold process startup or theme resolution.

| First pass, milliseconds | 0.1.3 | 0.2.0 | Change |
| --- | ---: | ---: | ---: |
| Rust | 3.376 (3.257–3.562) | 2.973 (2.847–3.125) | -12.0% |
| C++ | 6.556 (6.277–7.711) | 3.930 (3.768–4.693) | -40.0% |
| TypeScript | 7.319 (6.980–7.933) | 4.865 (4.746–5.260) | -33.5% |
| TSX | 7.484 (7.122–8.399) | 4.952 (4.763–6.116) | -33.8% |
| Markdown | 16.033 (15.652–17.557) | 6.269 (6.113–7.353) | -60.9% |
| Nix | 1.529 (1.488–1.883) | 1.329 (1.206–1.556) | -13.0% |

A separate process repeats the same file thirty times using one highlighter
and its default line cache. These are **amortized times per pass**, including
the first-use cost; they are not uncached engine throughput or isolated warm
passes.

| Thirty-pass average, milliseconds/pass | 0.1.3 | 0.2.0 | Change |
| --- | ---: | ---: | ---: |
| Rust | 0.382 (0.360–0.391) | 0.422 (0.405–0.480) | +10.3% |
| C++ | 0.243 (0.226–0.269) | 0.156 (0.151–0.206) | -36.0% |
| TypeScript | 0.362 (0.336–0.484) | 0.291 (0.281–0.372) | -19.4% |
| TSX | 0.397 (0.384–0.495) | 0.348 (0.335–0.389) | -12.3% |
| Markdown | 0.599 (0.575–0.712) | 0.280 (0.271–0.290) | -53.3% |
| Nix | 0.092 (0.088–0.123) | 0.096 (0.090–0.099) | +4.1% |

The C++ fixture emits 267 segments before and 287 after; the other five
segment counts are unchanged. Syntaxmate 0.2.0 includes an upstream grammar
refresh and correctness changes, including a rewritten C++ grammar. These are
version comparisons, not experiments holding engine output constant. See the
[release changes and migration guide](https://github.com/phongndo/syntaxmate/blob/v0.2.0/CHANGELOG.md).

## Diff rendering

`syntax-many-small-rust` has 240 files and a 455,004-byte patch.
`syntax-large-rust` has one 32,000-line source file and a 1,944,551-byte patch.
The additional syntax-ready time follows the first useful, initially unstyled
frame. Small-fixture readiness is affected by the harness's polling granularity.
Scroll rows below divide each pass's total time by its twenty positions.

| Many small Rust files, milliseconds | 0.1.3 | 0.2.0 | Change |
| --- | ---: | ---: | ---: |
| Model open | 1.919 (1.842–2.255) | 1.502 (1.277–1.776) | -21.7% |
| First useful frame | 0.253 (0.226–0.305) | 0.274 (0.233–0.332) | +8.5% |
| Additional syntax-ready time | 2.334 (1.279–2.552) | 1.296 (1.286–2.353) | -44.5% |
| Cold scroll, per position | 0.399 (0.381–0.487) | 0.410 (0.390–0.457) | +2.7% |
| Warm scroll, per position | 0.211 (0.203–0.224) | 0.212 (0.207–0.218) | +0.3% |
| Random scroll, per position | 1.307 (1.264–1.325) | 1.278 (1.045–1.334) | -2.3% |

| Large Rust diff, milliseconds | 0.1.3 | 0.2.0 | Change |
| --- | ---: | ---: | ---: |
| Model open | 2.585 (2.310–3.103) | 1.881 (1.646–2.196) | -27.3% |
| First useful frame | 0.788 (0.701–0.836) | 0.914 (0.837–0.956) | +16.1% |
| Additional syntax-ready time | 88.801 (87.677–90.897) | 62.457 (60.290–64.525) | -29.7% |
| Cold scroll, per position | 0.220 (0.214–0.246) | 0.226 (0.217–0.249) | +2.6% |
| Warm scroll, per position | 0.192 (0.186–0.213) | 0.198 (0.191–0.238) | +3.3% |
| Random scroll, per position | 0.260 (0.255–0.285) | 0.268 (0.261–0.282) | +3.0% |

Both versions completed syntax jobs without failures. The large fixture's
warm-plus-random phase recorded 86,214 theme-cache hits and no misses in both
versions. The small fixture recorded 76,126 hits / zero misses before and
74,656 hits / 1,470 misses after. The new adapter caches per highlighting
result; it cannot share caches using Syntaxmate's now-private storage identity.

The 1,598,013-byte `syntax-minified-rust` patch was also measured as a resource
limit control. Neither version completed syntax jobs for it, so its zero
syntax-ready time is not a highlighting speedup. Plain-mode controls and this
case's full measurements are retained in the JSON.

## Memory and executable size

Peak RSS comes from Linux `wait4().ru_maxrss`, covering the complete fresh
process, including source buffers, allocator retention, and mapped code/assets.
For TUI cases it covers both the plain and syntax passes. This is not a measure
of live highlighting allocations alone.

| Peak RSS, MiB | 0.1.3 | 0.2.0 | Change |
| --- | ---: | ---: | ---: |
| Rust, first pass | 27.373 (27.086–27.375) | 25.773 (25.727–25.777) | -5.8% |
| C++, first pass | 39.393 (39.090–39.406) | 27.777 (27.727–27.781) | -29.5% |
| TypeScript, first pass | 35.391 (35.102–35.391) | 27.777 (27.520–27.785) | -21.5% |
| TSX, first pass | 33.387 (33.031–33.387) | 27.785 (27.781–27.785) | -16.8% |
| Markdown, first pass | 49.156 (49.098–49.406) | 33.785 (33.434–33.785) | -31.3% |
| Nix, first pass | 27.375 (27.102–27.375) | 23.777 (23.691–23.781) | -13.1% |
| Many small Rust diffs | 64.346 (62.574–66.320) | 60.234 (58.984–61.238) | -6.4% |
| Large Rust diff | 137.676 (136.262–142.766) | 122.420 (121.070–129.434) | -11.1% |
| Minified guardrail | 32.293 (31.898–32.430) | 25.033 (24.797–25.234) | -22.5% |

Scope storage is opaque in 0.2.0, so the harness's scope/cache byte estimates
changed meaning. The small fixture's estimated retained syntax cache rose from
3,114,344 to 3,747,516 bytes; the large fixture changed from 22,282,468 to
22,284,804. The adapter adds per-result representatives and bounded style
slots, while estimates can no longer deduplicate engine tables across results
or inspect private capacities. Use RSS for the cross-version memory comparison.

The unstripped release **benchmark executable** grew from 8,535,648 to
9,213,728 bytes (+7.9%). This is not a shipped CLI archive-size measurement.
Syntaxmate's release notes describe its tradeoff of larger uncompressed bundle
metadata for cheaper construction and lower retained heap.

## Reproduction

Build the baseline revision and migrated tree in their Nix shells with
`cargo build -p mark-bench --release --locked`. Save the resulting executables
as `target/syntaxmate-0.2.0/{before,after}/mark-bench` before rebuilding.
The compiler, binary, lockfile, and implementation hashes are in the JSON.
The after build also selected `-p mark-cli` for CLI validation; its benchmark
uses the same release profile and diagnostic features.

In `nix develop`, generate the fixtures once with the baseline binary:

```sh
bench_dir="$PWD/target/syntaxmate-0.2.0"
"$bench_dir/before/mark-bench" fixtures --out "$bench_dir/fixtures" \
  --scenario syntax-many-small-rust --scenario syntax-large-rust \
  --scenario syntax-minified-rust
mkdir -p "$bench_dir/config" "$bench_dir/corpus"
export XDG_CONFIG_HOME="$bench_dir/config"
export XDG_CACHE_HOME="$bench_dir/cache"
```

Copy the six pinned inputs listed above into `corpus`, with the filenames in
the JSON's `corpora` entries. Get the Rust file with
`git show 738599df:crates/mark-syntax/src/types.rs`. For each binary and file:

```sh
"$bench_dir/before/mark-bench" syntax-compare \
  --file "$bench_dir/corpus/mark-types.rs" --iterations 1 --skip-counters --json
# Repeat with --iterations 30, each other file, and after/mark-bench.
```

Measure each diff scenario separately:

```sh
MARK_TEXTMATE_BENCH_THEME=github-dark "$bench_dir/before/mark-bench" measure \
  --fixtures "$bench_dir/fixtures" --scenario syntax-large-rust \
  --syntax-language rust --max-scroll-steps 20 --json
# Repeat for the other two scenarios and after/mark-bench.
```

Repeat each pair twelve times, reversing binary order on even rounds. Clear
other `MARK_*` environment overrides first. Collect stdout as JSON, and on
Linux use `os.wait4(child.pid, 0)` to capture that process's `ru_maxrss` in KiB;
multiply by 1,024 for bytes. The tables use medians across processes, dividing
raw thirty-pass totals by thirty and scroll totals by twenty. Initial TUI pilot
runs and compilation times are excluded.

## Verification

- Full workspace tests: 984 passed; the affected syntax and TUI suites also
  passed after the final cache allocation adjustment (74 and 718 tests).
- Workspace Clippy with all targets/features and `-D warnings`, formatting,
  architecture checks, installer/update smoke, and performance smoke passed.
- Regression coverage preserves owned scopes after later documents and
  tokenizer destruction, plus cached property-match flags across theme and
  override switches. Existing exact-color, modifier, theme, and rendering tests
  pass. CLI inspection confirmed a keyword override changes the final color
  to `#ff00ff` and applies bold.
- Release CLI build and archive packaging passed. All eight grammar notice
  files match the published 0.2.0 crate and appear in the archive.
- The existing CLI startup gate passed: adjusted `mark --version` median
  0.513 ms against its 2 ms budget. No before/after CLI startup claim is made.

Not measured: other hosts, uncached filesystem startup, terminal output,
allocation profiles, PGO, the full language catalog, or interactive theme
switch latency. Recheck these dated results before using them for later releases.
