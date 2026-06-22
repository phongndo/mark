setup:
    cargo fetch --locked
    cargo build -p dx-cli --locked

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

fmt:
    cargo fmt --all

test:
    cargo test --workspace --all-targets --all-features --locked

build:
    cargo build -p dx-cli --locked

hooks:
    git config core.hooksPath .githooks

pi-check:
    cd pi-dx && pnpm run check

pi-dev:
    pi -e ./pi-dx/extensions/pi-dx.ts
