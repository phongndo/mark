# Development

This repository favors small, verified changes. Read the relevant code first,
make the smallest safe diff, and document user-visible behavior changes.

## Prerequisites

- Git
- Rust toolchain from [`rust-toolchain.toml`](../rust-toolchain.toml)
- `curl`, `tar`, and `install` for installer smoke tests
- `just` for repository recipes
- `mise` for hk hook/tool provisioning
- Node.js 24 and pnpm 11 for `pi-mark`
- Nix, optional but preferred for a complete local shell

Do not install global tools just to work in the repo when `nix develop` or the
checked-in package manager setup is available.

## Setup

Preferred:

```sh
nix develop
just setup
```

Without Nix:

```sh
cargo fetch --locked
cargo build -p mark-cli --locked
```

Install global hk Git hooks:

```sh
just hooks
```

hk's global hooks require Git 2.54 or newer. The Nix development shell provides
a new enough Git. The hook command is a no-op in repositories without `hk.pkl`.
This repository's pre-commit hook runs fast staged-file checks and safe fixers;
pre-push enables the slower `full` and `pi` profiles for affected files.

## Common commands

```sh
just check
just ci-check
just ci-rust
just ci-generated
just ci-performance
mise x -- hk check --all --plan
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo build -p mark-cli --locked
```

The `scripts/ci/` suites are the canonical commands used by GitHub Actions and
the matching `just ci-*` recipes. `just ci-check` runs the complete local CI
suite. Pull requests classify changed paths and run only affected suites, then
join them behind the single `CI gate` check. See [Continuous
integration](ci.md) for the workflow graph and required repository settings.

For the Pi package:

```sh
cd pi-mark
pnpm install
pnpm run check
```

Run the local extension from the repository root with:

```sh
pi -e ./pi-mark/extensions/pi-mark.ts
```

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
resolves semver tags like `v0.10.1` for the default `latest` channel, so stable
updates do not accidentally install nightly.

Nightly builds set `MARK_BUILD_CHANNEL=nightly`, so `mark --version` includes
the channel and source commit.

## pi-mark release flow

`pi-mark` is published separately to npm.

1. Update `pi-mark/package.json` version.
2. Merge the change and wait for the exact `main` commit to pass `CI gate`.
3. Run the `Publish pi-mark` workflow from `main`.

The workflow requires the current CI-qualified `main` tip, validates the
package, publishes with npm provenance, and can create a `pi-mark-vX.Y.Z`
GitHub release.
