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

Report throughput together with each engine's emitted segment/token count;
token counts differ between engines and are not directly comparable.
