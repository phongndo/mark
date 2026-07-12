# Performance reports

Committed summaries for performance runs that affect product decisions live in
this directory. Raw JSON/counter/flamegraph artifacts should stay under ignored
`target/textmate-performance/reports/<date>-<commit>/`.

Use `docs/performance-plan.md` for the active benchmark protocol,
acceptance gates, and optimization backlog.

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

