# Development

This repository favors small, verified changes. Read the relevant code first,
make the smallest safe diff, and document user-visible behavior changes.

## Prerequisites

- Git
- Rust toolchain from [`rust-toolchain.toml`](../rust-toolchain.toml)
- `curl`, `tar`, and `install` for installer smoke tests
- Nix with flakes enabled

The development flake provides the Rust toolchain, `hk`, `just`, Node.js 24,
and all formatter and linter dependencies. Do not install global tools just to
work in the repository.

## Setup

Enter the pinned development shell and set up the repository:

```sh
nix develop
just setup
```

From the Nix development shell, install global hk Git hooks:

```sh
just hooks
```

The Nix development shell provides `hk` and Git 2.54 or newer, which hk's
global hooks require. The hook command is a no-op in repositories without
`hk.pkl`.
This repository's pre-commit hook runs fast staged-file checks and safe fixers;
pre-push directly requires dependency audit, Clippy, and rust-analyzer checks.

## Common commands

```sh
just check
just ci-check
just ci-rust
just ci-generated
just ci-performance
hk check --all --plan
cargo fmt --all --check
cargo audit --deny warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo build -p mark-cli --locked
```

The `scripts/ci/` suites are the canonical commands used by GitHub Actions and
the matching `just ci-*` recipes. `just ci-check` runs the complete local CI
suite. Pull requests classify changed paths and run only affected suites, then
join them behind the single `CI gate` check. See [Continuous
integration](ci.md) for the workflow graph and required repository settings.

## Verification ladder

Use the cheapest check that proves the change first:

1. `cargo fmt --all --check`
2. Focused unit test, for example `cargo test -p mark-tui filter`
3. `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
4. Focused integration or smoke test
5. `just ci-rust`
6. The affected generated or performance suite
7. `just ci-check`

The scheduled Extended validation workflow owns rust-analyzer diagnostics,
shared-runner performance thresholds, and the four-platform test matrix. Pull
request CI retains deterministic performance smoke coverage without making
machine-sensitive latency thresholds a merge gate.

## Local smoke tests

Installer and update smoke test:

```sh
scripts/smoke-installer-update
```

Interactive error-pane smoke test:

```sh
scripts/test-diff-error-pane
```

The interactive smoke test must run in a terminal.

## Allocation profiling

`mark-bench` has an opt-in counting allocator that reports allocation calls,
reallocations, byte churn, retained-byte deltas, and peak live-byte growth for
loading, model construction, filtering, rendering, and scroll passes:

```sh
cargo run -p mark-bench --release --features allocation-profile -- \
  measure-patch path/to/change.diff --max-scroll-steps 100 --json
```

The JSON report adds `allocation_profile` to each run. The human report prints
the same totals and per-stage breakdown. Keep this feature out of latency runs:
its atomic counters intentionally perturb timings. Use a normal release
`mark-bench` build for before/after latency and RSS measurements.

## Profile-guided builds

`scripts/build-pgo` produces a profile-guided release `mark` binary:
it builds instrumented binaries, trains on the committed engine corpora and
bench fixtures, merges the profiles with `llvm-profdata` (needs
`rustup component add llvm-tools`), and rebuilds with `-Cprofile-use`.
Engine-bound corpora run 15–35% faster than a plain release build
(`docs/performance-reports/2026-07-12-engine-optimization.md`). Retrain
whenever the engine or the global allocator changes materially — a stale
profile silently forfeits most of the gain.

## Release flow

The main `mark` binary release uses GitHub Releases.

1. Update the workspace package version in [`Cargo.toml`](../Cargo.toml).
2. Merge the change and wait for the exact `main` commit to pass `CI gate`.
3. Push a `vX.Y.Z` tag, or run the Release workflow manually from `main`.

Release refuses a tag outside `main`, a stale manual-dispatch SHA, a version
mismatch, or a source commit without a successful CI push run. The qualified
source is then built once per target; the release workflow does not repeat the
complete test suite on every platform.

The Release workflow builds macOS and Linux assets named like:

```text
mark-vX.Y.Z-aarch64-apple-darwin.tar.gz
mark-vX.Y.Z-x86_64-apple-darwin.tar.gz
mark-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz
mark-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

Those names are part of the installer contract.

## Nightly flow

The Nightly workflow runs daily at 08:00 UTC (or manually from `main`) and
publishes the latest CI-qualified `main` commit to a mutable `vnightly` GitHub
prerelease. It does not run after every push. The installer treats it as an
explicit version channel:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | MARK_VERSION=nightly sh
```

Nightly replaces the active `mark` binary. Users switch back to the latest
stable semver release with:

```sh
mark update
```

Keep `vnightly` marked as a prerelease and not latest. The installer only
resolves semver tags like `v0.13.0` for the default `latest` channel, so stable
updates do not accidentally install nightly.

Nightly builds set `MARK_BUILD_CHANNEL=nightly`, so `mark --version` includes
the channel and source commit.
