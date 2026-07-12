# Mega-diff memory and scroll report — 2026-07-11

- Commit: working tree after span-diff/sparse-row memory pass
- Tree status: dirty (this implementation changeset)
- Host: macOS/Apple Silicon developer workstation
- Rust/Cargo: rustc 1.88 / cargo 1.88
- Raw artifacts: `target/tmp-mark-bench-fixtures/measure-1m-final.json`, `target/tmp-mark-bench-fixtures/measure-10m-final.json`, `target/tmp-mark-bench-fixtures/measure-1m-final-after-remaining.json`, `target/tmp-mark-bench-fixtures/measure-10m-final-after-remaining.json`

| Tool | Corpus | Command | p50 / p95 / max | Peak RSS | Gate | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| mark-bench | `mega-diff-1m` (1,000,000 diff rows, 74.0 MB patch, 800,783 UI rows) | `target/release/mark-bench measure-patch target/tmp-mark-bench-fixtures/mega-diff-1m/patch.diff --max-scroll-steps 20 --assert-max-rss-ratio 3.0 --json` | load 20.6 ms; open 20.8 ms; grep 68.0 ms; random-scroll max 87 µs | RSS delta 112 MB (1.51× patch bytes); UI model estimate 14.4 MB; search index estimate 27 KB | G-MEM/G-SCROLL pass at 1M smoke scale | Span-backed diff lines; sparse large-row model; no duplicated grep text; no eager large-line-width scan; random/end scroll path measured. |
| mark-bench | `mega-diff-10m` (10,000,000 diff rows, 750.3 MB patch, 8,007,813 UI rows) | `target/release/mark-bench measure-patch target/tmp-mark-bench-fixtures/mega-diff-10m/patch.diff --max-scroll-steps 20 --assert-max-rss-ratio 3.0 --json` | load 201 ms; open 126 ms; grep 232 ms; random-scroll max 93 µs | RSS delta 1.07 GB (1.42× patch bytes); UI model estimate 144 MB; search index estimate 272 KB | G-MEM/G-OPEN/G-SEARCH/G-SCROLL pass at 10M release scale | Raw hunk prefilter avoids per-line terminal conversion on no-match searches; RSS gate passed. |
| mark-bench | `mega-diff-10m` after remaining-plan pass | same, `--max-scroll-steps 20 --assert-max-rss-ratio 3.0 --json` | load 224 ms; open 133 ms; grep 235 ms; random-scroll max 82 µs | RSS delta 1.06 GB (1.42× patch bytes) | G-MEM/G-OPEN/G-SEARCH/G-SCROLL pass | Adds stage timing rows, syntax latency buckets, sparse inline cache accounting, and opt-in parallel parser plumbing without changing the default RSS gate. |
| mark-bench | `mega-diff-100k` (100,000 diff rows, 7.3 MB patch, 80,079 UI rows) | same, `--scenario mega-diff-100k` | load ~2 ms; open ~12 ms; grep ~7 ms; random-scroll max < 0.1 ms | RSS delta ~17 MB (~2.35× patch bytes) | G-MEM/G-SCROLL pass at smoke scale | Used for fast iteration. |

## Findings

- Span-backed diff parsing removes per-line `String` copies and keeps a single raw patch backing store for hunk payloads.
- Search index construction no longer duplicates full diff text; file-filter metadata is the only eager search allocation, and large diffs skip the eager max-line-width scan.
- Large UI models switch to sparse row segments above the eager threshold; scroll/render work remains viewport-bounded in the synthetic benchmark; random jumps and end-of-file jumps are now measured separately from sequential scroll.
- Syntax queueing no longer runs git/blob-size subprocesses on the viewport path; full-file size checks happen in the worker before reading blob contents.

## Decisions

- Keep the non-TTY streaming path unchanged.
- Use `mega-diff-1m` as the CI-safe RSS smoke gate and reserve 10M for local/release-gate runs.
- The 10M run is under the 3× patch-byte RSS gate and the 250 ms interactive grep gate on this host.

## Follow-ups

- Keep static-mode syntax disabled for oversized models; static pager streaming can further reduce one-shot output memory later.
