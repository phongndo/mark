# Development

## Setup and checks

Enter the pinned [Nix development shell](../flake.nix), then build:

```sh
nix develop
just setup
```

The shell supplies the toolchain and check dependencies. Its `mark` command
uses this checkout's debug binary, rebuilding when source changes. `just hooks`
optionally installs **global** hk Git hooks; review [hk.pkl](../hk.pkl) for what
runs on commit and push.

Use `just --list` to discover tasks. [justfile](../justfile) and
[scripts/ci/](../scripts/ci/) own the commands used locally and in Actions.
Start with a focused test for the changed behavior; `just ci-check` runs the
complete local CI suite. [CI administration](ci.md) covers repository settings
that cannot be inferred from a local run.

Special verification paths:

- Installer/update changes: `scripts/smoke-installer-update`.
- Error-pane and terminal recovery changes: `scripts/test-diff-error-pane`,
  run in a terminal.
- Bundled agent workflow changes: [live agent evaluations](agent-review-evals.md).
  `cargo test -p mark-cli skill` checks packaging and CLI examples, not agent
  judgment.
- Cross-subsystem refactors: `scripts/check-architecture`; see the
  [boundary rationale](architecture.md).

## Performance work

Use the existing `mark-bench` harness. Discover scenarios and options with:

```sh
cargo run -p mark-bench --release --locked -- --help
cargo run -p mark-bench --release --locked -- measure-patch --help
```

For wrapped views and annotation-heavy reviews:

```sh
cargo run -p mark-bench --release --locked -- measure-patch change.diff \
  --wrap-lines --max-scroll-steps 100 --json
cargo run -p mark-bench --release --locked -- measure-patch change.diff \
  --annotations 50 --annotation-words 1000 --max-scroll-steps 100 --json
```

These measure complete TestBackend frames, including layout and buffer diffing,
not terminal I/O or CLI-start-to-first-frame latency. Annotation setup belongs
to model-open cost. Wrapped scrolling uses visual rows.

For syntax-enabled runs, `initial_render_micros` is the first useful frame;
`initial_syntax_ready_micros` is the additional wait and paint for highlighting.
Scroll samples include syntax readiness, queued prefetch work, and repainting,
including full-file-to-hunk fallback. Polling adds timing granularity; readiness
timeouts fail the run. Use `syntax-compare` for synchronous whole-document
highlighting instead. The measurement implementation lives in
[run.rs](../crates/mark-tui/src/run.rs).

Allocation profiling is a **separate build**, not a latency benchmark:

```sh
cargo run -p mark-bench --release --locked --features allocation-profile -- \
  measure-patch change.diff --max-scroll-steps 100 --json
```

Counters are process-wide, so overlapping workers and observers contribute;
allocation churn is not RSS or call-stack attribution. Use ordinary release
binaries for latency and RSS comparisons.

For an A/B comparison, use identical fixtures, an isolated empty
`XDG_CONFIG_HOME`, equivalent instrumentation, and alternating process order.
Record revision/build identities, host, commands, sample counts, variability,
and excluded or untested cases with the change. Include model-open cost and
retained memory when evaluating a cache; fewer allocations alone are not a win.
The executable smoke and extended gates live in
[scripts/ci/performance](../scripts/ci/performance).

For profile-guided builds, use [scripts/build-pgo](../scripts/build-pgo). It
requires `llvm-profdata` from the Rust `llvm-tools` component. Retrain after
material engine or allocator changes rather than assuming old profiles help.

## Releases

1. Bump the workspace version in [Cargo.toml](../Cargo.toml) and refresh
   `Cargo.lock`.
2. Merge and wait for a successful CI **push** run on that exact `main` commit.
3. Push the matching `vX.Y.Z` tag, or dispatch the
   [Release workflow](../.github/workflows/release.yml) from the current `main`.

Source qualification and packaging rules live in that workflow and
[build-dist.yml](../.github/workflows/build-dist.yml). Asset names are an
installer contract; change [install.sh](../scripts/install.sh) and packaging
checks together if that contract changes.

The generated `mark-cli.rb` release asset is consumed by
[phongndo/homebrew-tap](https://github.com/phongndo/homebrew-tap). For immediate
tap updates, put `HOMEBREW_TAP_TOKEN` in the `release` GitHub Environment: a
fine-grained token scoped to that repository with Contents write permission.
Without it, the tap's scheduled update is the fallback.

The [Nightly workflow](../.github/workflows/nightly.yml) publishes the qualified
`main` build as `vnightly`. Keep it a prerelease, not latest. To try it with the
portable installer:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | MARK_VERSION=nightly sh
```

Nightly replaces the active binary. Run `mark update` to return to stable.
