# TextMate engine benchmarks

The benchmark modes deliberately separate grammar/parser setup from full-file
highlighting and never serialize tokens inside the timed interval.

## Native Mark

```sh
cargo build --release -p mark-syntax --example profile-cold
target/release/examples/profile-cold \
  --mode process-cold \
  --assets assets/tm-grammars/languages \
  --scope source.rust \
  target/syntax-fixtures/syntax-large-rust/repo/src/bench1.rs 1
```

Run the command in a fresh process for each process-cold sample. The driver
loads only the requested grammar's transitive external-include closure.

## Pinned standalone vscode-textmate

```sh
npm install --prefix tools/golden-oracle
node tools/textmate-bench.mjs \
  --mode process-cold \
  --assets assets/tm-grammars/languages \
  --scope source.rust \
  --file target/syntax-fixtures/syntax-large-rust/repo/src/bench1.rs \
  --iterations 1 --json
```

Use `--mode same-driver --iterations 3` for repeated passes after one setup.

## Historical Tree-sitter implementation

This runs the actual Mark implementation immediately before commit `692e78d`,
using its pinned `tree-sitter-highlight` and language-pack dependencies:

```sh
tools/benchmark-legacy-tree-sitter.sh \
  rust "$PWD/target/syntax-fixtures/syntax-large-rust/repo/src/bench1.rs" 1
```

The first invocation creates an ignored detached worktree and may download the
pinned parser artifact. Parser installation is excluded from reported setup
and highlighting time. Results are highlighting measurements, not parse-only
Tree-sitter numbers.

## Quality oracle

```sh
node tools/golden-dump.mjs \
  --assets assets/tm-grammars/languages \
  --scope text.html.markdown \
  --file benchmarks/textmate/corpora/markdown-embedded-private.md \
  --out /tmp/oracle.jsonl

target/release/examples/tokenize \
  --assets assets/tm-grammars/languages \
  --scope text.html.markdown \
  benchmarks/textmate/corpora/markdown-embedded-private.md >/tmp/native.jsonl

python3 tools/compare-textmate-scopes.py /tmp/oracle.jsonl /tmp/native.jsonl
```

Tree-sitter and TextMate token counts are not directly comparable; report
throughput together with each engine's emitted segment/token count.
