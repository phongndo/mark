# VS Code TextMate Theme and Grammar Parity Plan

**Status:** Implemented (local validation; CI restructuring excluded by request)  
**Repository:** `phongndo/mark`  
**Primary target:** VS Code + GitHub Dark High Contrast with semantic highlighting disabled  
**Scope:** Syntax tokenization, TextMate theme resolution, rendering, validation, and catalog-wide grammar auditing

## 1. Executive summary

Mark already performs differential validation of its TextMate tokenizer against a pinned `vscode-textmate` oracle. The LaTeX screenshot mismatch is not primarily caused by the LaTeX grammar. It is caused by the rendering pipeline collapsing a complete TextMate scope stack into one of a small number of `SyntaxClass` values before applying a hand-written palette.

That conversion loses distinctions required by real VS Code themes, including:

- `support.function` versus `entity.name.function`;
- `constant.character` versus generic `constant`;
- nested and parent scope selectors;
- selector specificity and rule order;
- `fontStyle` modifiers such as bold, italic, underline, and strikethrough;
- language-specific exceptions in the theme;
- token background colors.

The fix is not a LaTeX-specific remapping. Mark must retain exact scope-stack information through the highlighting pipeline and resolve the official VS Code theme rules against that stack. In parallel, Mark should strengthen its grammar parity contract so that every bundled language is checked against an explicit, pinned reference grammar and `vscode-textmate` behavior.

## 2. Goals

### 2.1 Required goals

1. Match VS Code TextMate token styling for GitHub Dark High Contrast when semantic highlighting is disabled.
2. Preserve exact TextMate scope stacks through tokenization and rendering.
3. Implement VS Code-compatible TextMate theme selector matching, specificity, precedence, foreground/background colors, and font modifiers.
4. Keep release builds offline, deterministic, and Node-free.
5. Validate resolved styles across all bundled public language IDs, not only LaTeX.
6. Audit grammar provenance and behavior separately from theme behavior.
7. Preserve Mark's existing diff backgrounds and inline-diff emphasis through a documented style-composition policy.
8. Add diagnostics that make future grammar-versus-theme investigations straightforward.

### 2.2 Non-goals for this project

1. Language-server semantic-token parity. That is a separate feature and must not be mixed into the TextMate parity contract.
2. Pixel-identical screenshots across different fonts, terminal emulators, antialiasing settings, and display profiles.
3. Modifying grammars merely to compensate for a theme-resolution defect.
4. Requiring Node, VS Code, or network access in the shipped binary.
5. Guaranteeing that every terminal visibly supports every style modifier. Mark must emit the correct style flags even when a terminal ignores some of them.

## 3. Current problem

The current pipeline is effectively:

```text
source text
  -> TextMate tokenizer
  -> exact scope stack
  -> collapse to SyntaxClass
  -> hand-written SyntaxPalette
  -> terminal Style
```

The required pipeline is:

```text
source text
  -> TextMate tokenizer
  -> exact interned scope stack
  -> compiled TextMate theme selector resolver
  -> resolved syntax style
  -> merge with diff/UI overlays
  -> terminal Style
```

The existing `SyntaxClass` abstraction remains useful as a fallback for ANSI/Base16 themes and custom overrides, but it cannot be the source of truth for VS Code theme parity.

## 4. Correctness contracts

Mark should maintain four explicit and separate contracts.

### 4.1 Grammar asset parity

For each language, record the exact grammar source, version/revision, root scope, canonicalized hash, dependencies, and license. For languages built into VS Code, compare Mark's grammar against the pinned VS Code grammar. For extension-provided languages, compare against the pinned upstream extension or Shiki source used by Mark.

### 4.2 Tokenizer parity

Given identical grammar assets and source text, Mark must produce the same token boundaries, scope stacks, and line state as the pinned `vscode-textmate` + `vscode-oniguruma` oracle.

### 4.3 Theme parity

Given identical scope stacks and a pinned VS Code color theme, Mark must resolve the same:

- foreground color;
- background color;
- bold flag;
- italic flag;
- underline flag;
- strikethrough flag.

### 4.4 Render-composition parity

Mark must document how syntax styles combine with diff presentation. The recommended policy is:

- syntax foreground: retained;
- syntax font modifiers: retained;
- token background: used when no diff background is active;
- diff line background: takes precedence in diff rows;
- inline diff background and bold emphasis: applied after syntax style;
- explicit user color overrides: applied last.

This policy deliberately differs from a normal VS Code editor only where Mark's diff UI requires it.

## 5. Target data model

### 5.1 Preserve exact scope stacks

Do not copy strings into every segment. Store a compact index into a shared immutable scope table.

Suggested shape:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeStackRef(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSegment {
    pub byte_start: usize,
    pub byte_end: usize,
    pub scope_stack: ScopeStackRef,
    // Compatibility/fallback only. Not used by exact TextMate themes.
    pub coarse_class: Option<SyntaxClass>,
}

#[derive(Debug)]
pub struct HighlightScopeTable {
    // Each entry is a complete ordered TextMate scope stack.
    stacks: Vec<Arc<[ScopeAtomId]>>,
    atoms: Vec<Arc<str>>,
}

#[derive(Debug, Clone)]
pub struct HighlightedLine {
    pub fingerprint: LineTextFingerprint,
    pub segments: Vec<SyntaxSegment>,
    pub scope_table: Arc<HighlightScopeTable>,
}
```

The final exact shape can differ, but it must meet these requirements:

- scope-stack identity is stable for the lifetime of a highlighted result;
- lines cached independently can safely share the table;
- worker threads can produce results without sharing mutable state with the renderer;
- theme changes do not require retokenization;
- repeated scope stacks are interned once;
- the coarse class can still be derived for fallback themes and compatibility.

### 5.2 Resolved style model

```rust
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SyntaxModifiers: u8 {
        const BOLD          = 0b0001;
        const ITALIC        = 0b0010;
        const UNDERLINED    = 0b0100;
        const CROSSED_OUT   = 0b1000;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedSyntaxStyle {
    pub foreground: Option<RgbColor>,
    pub background: Option<RgbColor>,
    pub modifiers: SyntaxModifiers,
}
```

An explicit `fontStyle: ""` rule must be capable of clearing inherited/default modifiers.

## 6. Theme engine requirements

### 6.1 Theme assets

Vendor the exact generated JSON used by the selected VS Code theme rather than reconstructing syntax colors from a small palette.

Add a source manifest, for example:

```text
assets/tm-themes/
  SOURCE.toml
  github-dark-high-contrast.json
  licenses.json
```

The manifest must record:

- repository;
- extension/package version;
- source commit;
- source path;
- SHA-256 of the committed JSON;
- license;
- generation command, if transformed;
- whether the file is a fully resolved theme or contains includes.

The release binary must embed a deterministic compiled representation. Theme generation may use Node during development, but release builds must not.

### 6.2 Supported TextMate theme features

The Rust resolver must support at least:

1. `tokenColors` entries in declaration order.
2. `scope` as a string or array of strings.
3. Comma-separated selectors.
4. Scope-prefix matching on dot boundaries.
5. Parent/ancestor selectors expressed by whitespace-separated scopes.
6. Selector specificity compatible with VS Code/TextMate behavior.
7. Later-rule precedence when selector scores are equal.
8. Foreground colors.
9. Background colors.
10. `fontStyle` values: `bold`, `italic`, `underline`, `strikethrough`, combinations, and empty-string reset.
11. Default editor foreground/background fallback.
12. Rules without a `scope`, where applicable.
13. Deterministic behavior for malformed or unsupported rules, with validation errors during asset compilation rather than silent runtime fallback.

### 6.3 Selector compilation

Compile selectors into compact structures at build time or first load:

```rust
struct CompiledThemeRule {
    selector: CompiledSelector,
    foreground: Option<RgbColor>,
    background: Option<RgbColor>,
    modifiers: ModifierUpdate,
    source_order: u32,
}
```

Recommended optimizations:

- intern all selector scope atoms;
- index candidate rules by the final/innermost scope prefix;
- store ancestor constraints as compact atom sequences;
- resolve once per unique `ScopeStackRef` and theme generation;
- avoid string allocation in the render loop;
- invalidate only the resolved-style cache when the theme changes.

## 7. Implementation phases

## Phase 0 — Reproduce and freeze the defect

### Tasks

- Add the visible `hw2.tex` excerpt as a new LaTeX regression fixture.
- Add tokens that expose the known lossy mappings:
  - `\begin`, `\end`, `\textbf`, `\qquad`;
  - `\leftarrow` and other `constant.character.math.tex` symbols;
  - generic constants;
  - punctuation nested inside `support.function`;
  - bold, italic, raw, underline, and strikethrough scopes where supported.
- Capture the exact VS Code version, LaTeX grammar revision, GitHub theme version, settings, and semantic-highlighting state used for comparison.
- Record a baseline mismatch report before changing production code.

### Deliverables

- `crates/mark-syntax/tests/fixtures/textmate/latex/hw2-theme.tex`
- scope-stack golden generated by the existing oracle;
- resolved-style golden generated by a new theme oracle;
- a short reproduction document under `docs/`.

### Exit criteria

- The mismatch is deterministic and reproducible from committed files.
- The report identifies each token's source text, scope stack, expected selector, expected style, current class, and current style.

## Phase 1 — Add scope inspection and style-oracle tooling

### Tasks

Add a diagnostic command or example with output similar to:

```text
mark syntax inspect path/to/hw2.tex --line 134 --theme github-dark-high-contrast
```

For every token, print:

- byte and UTF-16 ranges;
- source text;
- full scope stack;
- current `SyntaxClass` fallback;
- matched theme selector;
- selector score;
- source rule index;
- resolved foreground/background;
- resolved modifiers;
- whether a diff or user override changed the final style.

Extend the Node oracle to load the pinned VS Code theme and emit final TextMate styles for every token. Keep the existing raw scope-stack goldens unchanged.

Suggested tools:

```text
tools/theme-oracle/
tools/theme-golden-dump.mjs
tools/generate-theme-goldens.mjs
tools/compare-theme-styles.py
```

### Exit criteria

- A developer can prove whether a mismatch originates in grammar tokenization, selector resolution, or render composition without using screenshots.

## Phase 2 — Transport complete scope stacks

### Tasks

- Update `crates/mark-syntax/src/types.rs` to retain a scope-stack reference per segment.
- Update `crates/mark-syntax/src/engine/scopes.rs` to export or snapshot the interned scope table safely.
- Update tokenizer/highlighter conversion so segment boundaries remain unchanged.
- Preserve `SyntaxClass` as a derived compatibility field during migration.
- Update line/file caches, queue accounting, equality, fingerprints, and memory-size calculations.
- Add serialization/debug formatting only for tests and diagnostics; production rendering must use interned IDs.

### Important constraint

This phase must not change visual output. It is a data-preservation change only.

### Tests

- Existing exact scope-stack tests remain green.
- Existing TUI render snapshots remain unchanged.
- A new test verifies that every nonempty segment can resolve its complete scope stack.
- Theme switching does not cause tokenization work.
- Scope tables are shared rather than duplicated per line.

### Exit criteria

- No scope information is lost before the theme layer.
- Existing behavior remains byte-for-byte stable.

## Phase 3 — Implement the Rust TextMate theme resolver

### Tasks

- Add a dedicated module, preferably in `mark-syntax` or a small new crate:

```text
crates/mark-syntax/src/theme/
  mod.rs
  asset.rs
  selector.rs
  resolver.rs
  style.rs
```

- Parse and validate the pinned theme JSON.
- Implement selector parsing and scoring.
- Implement ordered rule resolution.
- Implement modifier application and reset behavior.
- Add a cache keyed by `(theme_generation, ScopeStackRef)` or an equivalent stable key.
- Add an explicit default style from the theme's editor foreground/background.
- Fail asset compilation on unsupported constructs needed by bundled themes.

### Unit-test matrix

Cover at least:

- exact scope match;
- prefix match (`support` matching `support.function`);
- non-boundary rejection (`entity.name` must not match an unrelated string prefix);
- parent selector matching;
- deeper ancestor chains;
- multiple comma-separated selectors;
- array-valued scopes;
- specificity comparisons;
- equal-score later-rule precedence;
- foreground-only rule;
- background-only rule;
- modifier addition;
- empty modifier reset;
- no-match fallback;
- malformed selector diagnostics.

### Differential tests

Generate randomized valid scope stacks and selectors, then compare Mark's resolver with the pinned VS Code theme-resolution oracle.

### Exit criteria

- The resolver matches the oracle for the full selector conformance suite.
- Runtime resolution performs no heap allocation after cache warmup.

## Phase 4 — Integrate the official GitHub Dark High Contrast theme

### Tasks

- Keep the current GitHub palette for UI chrome, diff colors, gutters, status line, and notifications.
- Replace only the syntax-token mapping with the compiled official TextMate theme.
- Update `crates/mark-tui/src/theme/palettes.rs` so `SyntaxPalette::github(...)` is no longer the primary path for exact GitHub themes.
- Update `crates/mark-tui/src/theme/colorscheme.rs` to load both:
  - UI/diff palette;
  - TextMate syntax theme.
- Update `crates/mark-tui/src/render/diff/content.rs` to request a resolved style from the full scope stack rather than a color from `SyntaxClass`.
- Map style modifiers to Ratatui modifiers.
- Apply user syntax-color overrides after theme resolution.
- Preserve the existing class-based path for `system`, ANSI, and Base16 until those themes are migrated.

### Required LaTeX outcomes

With semantic highlighting disabled in VS Code and diff backgrounds excluded from the comparison:

- `support.function.*` commands resolve to the same blue as VS Code;
- `entity.name.function` remains purple;
- `constant.character.*` resolves separately from generic constants;
- punctuation uses the selector outcome produced from its complete stack;
- `markup.bold` is bold;
- `markup.italic` is italic;
- `markup.underline` is underlined;
- raw and link scopes match the theme's selector rules.

### Exit criteria

- The committed LaTeX reproduction has zero resolved-style mismatches.
- No language-specific color workaround is introduced.

## Phase 5 — Catalog-wide theme parity audit

### Tasks

Run every existing `basic` and `stress` fixture through both:

1. Mark's tokenizer + Mark's theme resolver;
2. the pinned `vscode-textmate` + VS Code theme oracle.

Compare every token's:

- range;
- full scope stack;
- foreground;
- background;
- modifiers.

Produce a machine-readable report grouped by:

- language;
- scope;
- expected selector;
- actual selector;
- mismatch type;
- frequency.

Suggested output:

```text
benchmarks/textmate/theme-parity.json
docs/theme-parity-status.md
```

### No-silent-divergence policy

- Do not add broad allowlists.
- A temporary divergence must name the exact language, fixture, line, token range, reason, owner, and tracking issue.
- Theme-resolution divergences block release for built-in exact themes.
- Semantic-token-only differences are excluded by running the VS Code oracle with semantic highlighting disabled.

### Languages expected to expose issues first

Prioritize:

- LaTeX/TeX/BibTeX;
- Markdown, MDX, AsciiDoc, and documentation formats;
- HTML, XML, JSX, TSX, Vue, Angular, PHP, and template languages;
- Java and languages with `storage.*` exceptions;
- languages with many `support.*` scopes;
- regular-expression grammars and embedded languages;
- Rust, TypeScript, C++, Python, Java, Go, and C# for semantic-token comparison controls.

### Exit criteria

- All bundled public languages have exact TextMate style parity for committed fixtures.
- The generated status document is checked in and verified in CI.

## Phase 6 — Grammar provenance and behavior audit

Theme parity does not prove that Mark uses the same grammar assets as the user's VS Code installation. Add a separate grammar audit.

### Tasks

1. Generate a canonical grammar manifest for every public and private grammar asset.
2. Record root scope, grammar hash, repository, revision, and dependencies.
3. For VS Code built-in languages, compare canonicalized grammar JSON against a pinned VS Code source tree.
4. For extension-backed languages, compare against the pinned upstream extension or Shiki source.
5. Flag transformed grammars whose behavior is equivalent but source hash differs.
6. Differentially tokenize both generated fixtures and representative real-world files.
7. Expand adversarial coverage for:
   - dynamic `end` expressions;
   - backreferences;
   - lookaround captures;
   - `\G` behavior;
   - injections;
   - embedded languages;
   - begin/while rules;
   - zero-width matches;
   - Unicode and UTF-16 boundary conversion;
   - very long lines;
   - malformed/incomplete source.
8. Keep `divergences.toml` empty for released exact-parity languages.

### Target selection policy

Define the expected comparison target explicitly:

- **VS Code built-in language:** pinned VS Code grammar.
- **Bundled non-VS-Code language:** pinned upstream extension/Shiki grammar.
- **User-installed extension grammar:** not part of the built-in parity guarantee until Mark supports loading external grammars.

### Exit criteria

- Every language has an explicit reference target.
- No language is described as "VS Code exact" when its grammar source differs without documented equivalence testing.

## Phase 7 — Performance and memory validation

### Measurements

Measure before and after each structural phase:

- process-cold tokenization throughput;
- warm tokenization throughput;
- viewport render latency;
- theme-cache hit rate;
- unique scope-stack count per corpus;
- scope-table bytes;
- resolved-style cache bytes;
- binary size;
- peak RSS on existing large corpora.

### Suggested gates

- Tokenization throughput regression: no more than 5% unless separately approved with evidence.
- Warm viewport render regression: no more than 5%.
- Cold first-render regression: no more than 10%.
- Peak RSS regression on the largest existing benchmark: no more than 15%.
- Theme cache hit rate after the first viewport: at least 99% on normal source files.
- No string allocation in the per-segment render hot path after cache warmup.

Any gate change must be committed with a benchmark report and rationale. Do not hide a correctness fix behind an unmeasured performance exception.

## Phase 8 — Configuration, compatibility, and rollout

### Built-in themes

Introduce an explicit distinction:

```text
Exact TextMate themes:
  github-dark-high-contrast
  github-dark
  github-light-high-contrast
  github-light

Class-based fallback themes during migration:
  system
  ansi
  base16
  catppuccin-*
  gruvbox-*
  tokyonight
```

Then migrate other built-in themes to real TextMate theme assets or generated equivalent selector rules. Avoid claiming VS Code parity for class-based themes.

### User overrides

Preserve existing overrides such as `keyword`, `string`, and `function`, but define them as coarse post-resolution overrides. Add optional scope-aware overrides later:

```toml
[[syntax_rules]]
scope = "support.function"
foreground = "#91cbff"

[[syntax_rules]]
scope = "entity.name.function"
foreground = "#dbb7ff"
font_style = "bold"
```

Scope-aware overrides should use the same selector engine as bundled themes.

### Compatibility

- Keep `SyntaxClass` in the public API for at least one release cycle.
- Mark it as a fallback/coarse classification rather than exact theme semantics.
- Avoid breaking external users until a replacement scope/style API is documented.

### Feature flag

During development, add a temporary comparison option:

```text
MARK_TEXTMATE_THEME_ENGINE=coarse|exact|compare
```

`compare` should render with the exact engine while logging differences from the coarse engine. Remove or hide this flag after rollout.

## 9. CI plan

Add distinct jobs so failures identify the broken contract.

### Job A — Grammar asset integrity

- verify source manifests and hashes;
- verify licenses;
- verify deterministic grammar/theme generation;
- verify no uncommitted generated diffs.

### Job B — Tokenizer parity

- existing exact scope-stack goldens;
- regex conformance;
- zero degradation/budget kills for committed fixtures;
- UTF-16/UTF-8 boundary checks.

### Job C — Theme selector conformance

- selector unit tests;
- randomized differential tests against the Node oracle;
- official theme parse/compile tests.

### Job D — Resolved-style parity

- all fixture tokens compared against the pinned theme oracle;
- foreground/background/modifier equality;
- no divergence allowlist for exact built-in themes.

### Job E — TUI composition snapshots

- context line;
- addition/deletion line;
- inline diff;
- search highlight;
- cursor line;
- custom override;
- transparent background;
- terminals with and without italic support represented at the style-buffer level.

### Job F — Performance policy

Keep absolute performance checks local if shared CI is too variable, but run deterministic structural checks in CI:

- cache bounds;
- no unexpected scope-table duplication;
- asset/binary size limits;
- benchmark report freshness.

## 10. Proposed pull-request sequence

### PR 1 — Reproduction and diagnostics

- Add the LaTeX fixture.
- Add theme oracle tooling.
- Add `syntax inspect` output.
- Commit the baseline mismatch report.

No production behavior change.

### PR 2 — Scope-stack transport

- Extend `SyntaxSegment`/`HighlightedLine` data.
- Share immutable scope tables.
- Preserve existing rendering via the coarse class.

No visual change.

### PR 3 — Theme selector engine

- Add parser, compiler, matcher, style model, cache, and differential tests.
- Vendor the pinned GitHub Dark High Contrast theme asset.

No default UI behavior change yet.

### PR 4 — GitHub theme integration

- Switch `github-dark-high-contrast` to exact TextMate theme resolution.
- Add font modifiers and style composition.
- Make the LaTeX reproduction pass.

### PR 5 — Full catalog style goldens

- Generate style goldens for all public languages.
- Fix selector/resolver defects.
- Add CI and generated status documentation.

### PR 6 — Grammar provenance audit

- Add canonical grammar source manifest.
- Compare VS Code built-ins and extension-backed grammars.
- Add missing adversarial fixtures.

### PR 7 — Cleanup and broader theme migration

- Remove GitHub syntax-class mappings that are no longer used.
- Clarify compatibility APIs.
- Migrate other built-in themes or relabel them as approximate.
- Update user documentation.

## 11. File-level work map

Likely files to modify or add:

```text
crates/mark-syntax/src/types.rs
crates/mark-syntax/src/engine/scopes.rs
crates/mark-syntax/src/engine/tokenizer.rs
crates/mark-syntax/src/highlight.rs
crates/mark-syntax/src/theme/mod.rs
crates/mark-syntax/src/theme/asset.rs
crates/mark-syntax/src/theme/selector.rs
crates/mark-syntax/src/theme/resolver.rs
crates/mark-syntax/src/theme/style.rs
crates/mark-tui/src/theme/colorscheme.rs
crates/mark-tui/src/theme/palettes.rs
crates/mark-tui/src/render/diff/content.rs
crates/mark-tui/src/tests/render.rs
assets/tm-themes/SOURCE.toml
assets/tm-themes/github-dark-high-contrast.json
assets/tm-themes/licenses.json
tools/theme-golden-dump.mjs
tools/generate-theme-goldens.mjs
tools/compare-theme-styles.py
crates/mark-syntax/tests/theme_selector.rs
crates/mark-syntax/tests/theme_golden.rs
crates/mark-syntax/tests/fixtures/textmate/latex/hw2-theme.tex
docs/theme-parity-status.md
docs/textmate-theme-engine.md
```

The exact module location may change after an architecture review, but theme resolution should not remain embedded in `palettes.rs`.

## 12. Risks and mitigations

### Risk: Scope-table ownership increases memory or complicates caches

**Mitigation:** Use immutable shared tables, intern stacks, track bytes in existing cache accounting, and benchmark unique-stack counts before selecting the final representation.

### Risk: Theme matching is subtly different from VS Code

**Mitigation:** Treat the Node implementation as a dev-only differential oracle. Do not rely only on hand-written selector tests.

### Risk: Theme changes force full rehighlighting

**Mitigation:** Separate tokenization output from resolved-style caches. Theme changes invalidate only style resolution.

### Risk: Diff backgrounds obscure token backgrounds

**Mitigation:** Define composition order explicitly and test both normal/context rendering and diff rendering.

### Risk: Semantic highlighting causes misleading screenshot comparisons

**Mitigation:** Add a documented comparison procedure and an oracle setting that forcibly disables semantic highlighting. Label semantic-token parity as unsupported until separately implemented.

### Risk: Mark and VS Code use different grammar revisions

**Mitigation:** Add grammar provenance manifests and compare canonicalized assets before diagnosing engine behavior.

### Risk: Coarse user overrides become surprising

**Mitigation:** Apply them last, document that they intentionally override multiple TextMate scopes, and later add scope-aware rules.

### Risk: Terminal style support varies

**Mitigation:** Test the emitted Ratatui style model, not only screenshots. Document terminal capability limitations separately from resolver correctness.

## 13. Definition of done

This project is complete when all of the following are true:

- [x] Mark retains complete TextMate scope stacks for every syntax segment.
- [x] GitHub Dark High Contrast uses the official pinned TextMate theme rules.
- [x] The Rust selector resolver matches the pinned VS Code oracle.
- [x] The LaTeX `hw2.tex` fixture has zero scope or resolved-style mismatches.
- [x] `support.function` and `entity.name.function` are styled independently.
- [x] `constant.character` and generic constants are styled independently.
- [x] Bold, italic, underline, and strikethrough survive the pipeline.
- [x] Theme switching does not retokenize source files.
- [x] All bundled public languages pass exact resolved-style fixture parity.
- [x] Grammar source and version are explicit for every bundled language.
- [x] VS Code built-in grammar differences are detected automatically.
- [x] Semantic highlighting is disabled and documented for TextMate parity tests.
- [x] Diff style-composition behavior is documented and snapshot-tested.
- [x] Performance and memory remain within the approved gates.
- [x] No broad divergence allowlist is present.
- [x] The shipped binary remains offline and Node-free.
- [x] Documentation no longer describes class-based GitHub syntax colors as exact VS Code theme parity.

## 14. Immediate next actions

1. Land the LaTeX screenshot excerpt as a fixture.
2. Build the resolved-style oracle using the already pinned `vscode-textmate` environment.
3. Add scope-stack transport without changing rendering.
4. Implement selector conformance before touching the palette mappings.
5. Switch only `github-dark-high-contrast` after the oracle suite is green.
6. Run the complete catalog and use the resulting mismatch report to prioritize follow-up fixes.
