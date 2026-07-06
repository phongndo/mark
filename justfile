setup:
    mise install
    cargo fetch --locked
    cargo build -p mark-cli --locked

check:
    HK_PROFILE=full mise x -- hk check --all --check

ci-check:
    HK_PROFILE=full,pi,ci mise x -- hk check --all --check --no-fail-fast

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
    pi -e ./pi-mark/extensions/pi-mark.ts
