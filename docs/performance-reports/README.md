# Performance reports

Committed summaries for performance runs that affect product decisions live in
this directory. Raw JSON/counter/flamegraph artifacts should stay under ignored
`target/textmate-performance/reports/<date>-<commit>/`.

Each report records the benchmark protocol and gates used for that run.
Deterministic performance checks live under `scripts/ci/`, with persisted
benchmark policies under `benchmarks/`.

## Report template

```md
# <area> performance report — YYYY-MM-DD

- Commit:
- Tree status:
- Host:
- Rust/Cargo:
- Node/oracle versions:
- Raw artifacts: target/textmate-performance/reports/<run>/

| Tool | Corpus | Command | p50 / p95 / max | Peak RSS | Gate | Notes |
| --- | --- | --- | --- | --- | --- | --- |

## Findings

## Regressions

## Decisions

## Follow-ups
```
