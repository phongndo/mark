# Continuous integration

## Workflow model

`CI` is the only pull-request workflow that validates source changes. It first
classifies the base-to-head diff, fans affected work out in parallel, and joins
all selected jobs behind `CI gate`:

```text
Classify changes
  ├─ Workflow lint
  ├─ Rust correctness
  ├─ MSRV
  ├─ Generated and oracle contracts
  ├─ TextMate golden shards
  ├─ Deterministic performance smoke
  └─ pi-mark package quality
          ↓
       CI gate
```

An unrelated documentation-only change can therefore complete after
classification and the gate. Cargo and toolchain changes select every
Rust-related lane; workflow and CI-script changes deliberately select all
lanes.

The lane mapping lives in [`scripts/ci/changes.py`](../scripts/ci/changes.py).
Keep it conservative and add a case to `tools/test_ci_changes.py` whenever a
new generated artifact or cross-component dependency is introduced. Do not use
workflow-level `paths` filters for required checks: GitHub can leave a skipped
required workflow pending indefinitely.

## Validation tiers

- **CI** runs merge-blocking deterministic validation on pull requests and
  pushes to `main`.
- **Extended validation** runs rust-analyzer, machine-sensitive performance
  thresholds, and the native four-platform test matrix each day.
- **Nightly** builds the latest exact `main` SHA only after that SHA has a
  successful CI push run.
- **Release** accepts only a version-matching commit reachable from `main` with
  a successful CI push run, then builds distribution assets without repeating
  the test matrix.
- **Publish pi-mark** accepts only the current CI-qualified `main` tip before
  contacting npm.

Nightly and Release call the same reusable distribution workflow and package
assets through `scripts/ci/package-dist`. Publish jobs alone receive
`contents: write`; build and validation jobs remain read-only.

## Required repository settings

Configure a branch ruleset for `main` with:

- pull requests required;
- `CI / CI gate` required;
- `PR Template / Required PR fields` required if PR metadata is policy;
- merge queue enabled only with the existing `merge_group` CI trigger.

Configure `release` and `nightly` GitHub Environments with the desired branch
and reviewer policy. Workflow checks still enforce exact-SHA qualification,
even when an environment has no manual reviewer.

## Local reproduction

Run the same suites used by Actions:

```sh
just ci-rust
just ci-generated
just ci-performance
just pi-check
just ci-workflows
```

`just ci-check` runs all of them. The scheduled performance thresholds can be
reproduced with:

```sh
scripts/ci/performance extended
```
