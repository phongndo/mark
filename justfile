setup:
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
    actionlint -color

ci-integrations:
    cargo build -p mark-cli --locked
    MARK_TEST_BIN="$PWD/target/debug/mark" node --test assets/integrations/tests/*.test.mjs assets/integrations/fx/tests/*.test.mjs

test-browser-review:
    scripts/test-browser-review

test-pi-review:
    scripts/test-pi-review

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
    hk install
