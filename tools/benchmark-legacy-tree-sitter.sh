#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
worktree=${MARK_TREE_SITTER_WORKTREE:-"$root/target/tree-sitter-baseline-worktree"}
commit=${MARK_TREE_SITTER_COMMIT:-"692e78d^"}

if [[ ! -e "$worktree/.git" ]]; then
  rm -rf "$worktree"
  git -C "$root" worktree add --detach "$worktree" "$commit" >/dev/null
fi
mkdir -p "$worktree/crates/mark-syntax/examples"
cp "$root/benchmarks/tree-sitter-baseline.rs" \
  "$worktree/crates/mark-syntax/examples/tree-sitter-baseline.rs"

exec cargo run \
  --manifest-path "$worktree/Cargo.toml" \
  --release --locked -p mark-syntax \
  --example tree-sitter-baseline -- "$@"
