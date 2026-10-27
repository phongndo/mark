# Development

## Setup and checks

Enter the pinned [Nix development shell](../flake.nix), then build:

```sh
nix develop
just setup
```

The shell supplies the toolchain and check dependencies. Its `mark` command
uses this checkout's debug binary, rebuilding when source changes. `just hooks`
installs this repository's hk Git hooks, which call the shell's `hk`, so commit
and push from inside `nix develop`; review [hk.pkl](../hk.pkl) for what runs.

Use `just --list` to discover tasks. [justfile](../justfile) and
[scripts/ci/](../scripts/ci/) own the commands used locally and in Actions.
Start with a focused test for the changed behavior; `just ci-check` runs the
complete local CI suite. [CI administration](ci.md) covers repository settings
that cannot be inferred from a local run.

Special verification paths:

- Installer/update changes: `scripts/smoke-installer-update`.
- Nix package changes, or files embedded from outside `crates/`:
  `nix build .#mark`. The [flake](../flake.nix) builds from an explicit file set.
- Error-pane and terminal recovery changes: `scripts/test-diff-error-pane`,
  run in a terminal.
- Bundled agent workflow changes: [live agent evaluations](agent-review-evals.md).
  `cargo test -p mark-cli skill` checks packaging and CLI examples, not agent
  judgment.
- Cross-subsystem refactors: `scripts/check-architecture`; see the
  [boundary rationale](architecture.md).
- Harness packages: `just ci-integrations` exercises
  adapters and self-contained packaging. `scripts/test-pi-review` checks the
  real terminal handoff when Pi is installed; `scripts/test-browser-review`
  drives the local viewer in temporary Chromium. Real host and model behavior
  remains covered by [agent evaluations](agent-review-evals.md).

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
to model-open cost. Wrapped scrolling uses visual rows. Runs use the split
layout; add `--unified` to measure the unified one.

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

Recorded comparisons: [Syntaxmate 0.2.0 migration](performance/syntaxmate-0.2.0.md)
(2026-09-27; includes raw samples and reproduction commands).

For profile-guided builds, use [scripts/build-pgo](../scripts/build-pgo). It
requires `llvm-profdata` from the Rust `llvm-tools` component. Retrain after
material engine or allocator changes rather than assuming old profiles help.

## Releases

Bump the workspace version in [Cargo.toml](../Cargo.toml), refresh
`Cargo.lock`, and merge. The [Release workflow](../.github/workflows/release.yml)
runs on every push to `main` and acts when the version has no release yet: it
waits for a successful CI **push** run on that exact commit, builds, tags, and
publishes. If it fails, fix `main` and the next push retries, or dispatch it
from the current `main`. A retry of the original release commit also retries
integration publication when the core release already exists. A later commit
with that same version does not replace the released source. The
[Installs workflow](../.github/workflows/installs.yml) then installs the release
with Homebrew, the install script, mise, and Nix on each platform and checks the
version; it runs again daily.

Source qualification and packaging rules live in the Release workflow and
[build-dist.yml](../.github/workflows/build-dist.yml). Asset names are an
installer contract: [install.sh](../scripts/install.sh) and mise's GitHub
backend select archives by their target triple. Change the installer and
packaging checks together if that contract changes.

The [integration publication workflow](../.github/workflows/publish-integrations.yml)
runs after the core release. It generates all five standalone ZIPs, Claude and
Codex marketplace catalogs, and Pi/OpenCode npm tarballs from the same qualified
source and workspace version. `release-index.json` records artifact hashes and
npm integrity. The generated `integrations` branch is the public marketplace;
the source templates under `assets/integrations` are not installable packages.
GitHub downloads and marketplace publication can finish even when npm needs
authentication. The workflow reports npm failures and can resume without
republishing conflicting immutable versions.

npm trusted publishers for `mark-pi` and `mark-opencode` must name GitHub owner
`phongndo`, repository `mark`, calling workflow `release.yml`, and environment
`release`. Allow both direct publication and `npm dist-tag`. The workflow pins
Node 24 and npm 11.21.0 for OIDC publication and tag promotion. An optional
`NPM_TOKEN` secret in the `release` environment is the explicit fallback.
Configure publishers in each package's npm settings after its initial
authenticated publication; see [npm's trusted publisher documentation](https://docs.npmjs.com/trusted-publishers/).
No npm login token belongs in the repository.

For local artifact inspection, run:

```sh
scripts/ci/prepare-integrations --source-sha "$(git rev-parse HEAD)" --output /tmp/mark-integration-release
```

The output directory must be empty. `node --test tools/test-integrations-release.mjs`
checks deterministic packaging, registry verification, and retry behavior with
an isolated registry fixture. Actual publication additionally verifies the
downloaded npm tarball bytes before promoting either package to `latest`.

The generated `mark-cli.rb` release asset is consumed by
[phongndo/homebrew-tap](https://github.com/phongndo/homebrew-tap). For immediate
tap updates, put `HOMEBREW_TAP_TOKEN` in the `release` GitHub Environment: a
fine-grained token scoped to that repository with Contents write permission.
Without it, the tap's hourly update is the fallback.

The [Nightly workflow](../.github/workflows/nightly.yml) publishes the qualified
`main` build as `vnightly`. Keep it a prerelease, not latest. To try it with the
portable installer:

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | MARK_VERSION=nightly sh
```

Nightly replaces the active binary. Run `mark update` to return to stable.
