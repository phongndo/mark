# Syntax highlighting architecture

Mark consumes [`syntaxmate = "0.1"`](https://crates.io/crates/syntaxmate) as an
ordinary registry dependency. Syntaxmate owns the Rust-native TextMate engine,
bundled grammar catalog, exact-scope data model, oracle corpus, fuzz targets,
performance evidence, and asset provenance.

`crates/mark-syntax` is a product adapter. It owns only Mark concerns:

- syntax settings and language enablement;
- custom path mappings;
- source-size, queue, and cache policy used by the TUI;
- conversion of exact scope stacks to optional coarse `SyntaxClass` fallbacks;
- Mark's built-in TUI themes and user color/selector overrides.

The adapter uses Syntaxmate's documented `Tokenizer` and `Catalog` APIs. It has
no access to Syntaxmate internals, no copied grammar bundle, and no filesystem
or network fallback for grammar loading.

## Correctness boundary

Syntaxmate validates tokenization against pinned `vscode-textmate` and
`vscode-oniguruma` oracles. Mark validates the downstream boundary through its
workspace suite, including exact-scope theme resolution, rendering, full-file
and hunk highlighting, queue behavior, resource limits, and packaged themes.

Run Mark's complete local regression suite with:

```sh
scripts/ci/rust
scripts/ci/performance smoke
scripts/ci/generated
```

Syntaxmate releases are upgraded deliberately through `Cargo.lock`; engine or
catalog changes must pass both projects' CI before Mark ships them.
