setup:
    mise install
    cargo fetch --locked
    cargo build -p mark-cli --locked

check:
    HK_PROFILE=full hk check --all --check

ci-check: ci-rust ci-generated ci-performance ci-workflows

ci-rust:
    scripts/ci/rust

ci-generated:
    scripts/ci/generated

ci-performance:
    scripts/ci/performance smoke

ci-workflows:
    mise x -- actionlint -color

fix:
    hk fix --all

check-architecture:
    scripts/check-architecture

fmt:
    cargo fmt --all

test:
    cargo test --workspace --all-targets --all-features --locked

build:
    cargo build -p mark-cli --locked

hooks:
    git config --unset core.hooksPath || true
    hk install --global
