setup:
    mise install
    cargo fetch --locked
    cargo build -p mark-cli --locked

check:
    HK_PROFILE=full mise x -- hk check --all --check

ci-check: ci-rust ci-generated ci-performance pi-check ci-workflows

ci-rust:
    scripts/ci/rust

ci-generated:
    scripts/ci/generated

ci-performance:
    scripts/ci/performance smoke

ci-workflows:
    mise x -- actionlint -color

fix:
    mise x -- hk fix --all

check-architecture:
    scripts/check-architecture

fmt:
    cargo fmt --all

test:
    cargo test --workspace --all-targets --all-features --locked

build:
    cargo build -p mark-cli --locked

hooks:
    @set -eu; profile_bin="$HOME/.nix-profile/bin"; if [ -d "$profile_bin" ]; then PATH="$profile_bin:$PATH"; fi; export PATH; mise trust --yes mise.toml; git config --unset core.hooksPath || true; mise_path="$(command -v mise)"; "$mise_path" x hk -- hk install --global --mise

pi-check:
    cd pi-mark && pnpm run check

pi-dev:
    cargo build -p mark-cli --locked
    PI_MARK_BIN="$PWD/target/debug/mark" pi -e ./pi-mark/extensions/pi-mark.ts
