# TextMate engine: performance + full-Shiki grammar expansion plan

Research, evaluation, and profiling round from 2026-07-11 on branch
`feat/rust-textmate-engine` (HEAD `6ff21dc`), followed by the execution plan to
(1) close the remaining engine performance gaps, (2) expand the public catalog
to full Shiki (`@shikijs/langs@3.23.0`) parity, and (3) add `ocaml`, `zig`,
`odin`, `mojo`, `mlir`, `llvm`, `asm` (x86-64), and `riscv` as first-class
languages.

Quality bar for everything in this plan: **byte-exact scope-stack parity with
the pinned `vscode-textmate@9.2.0` + `vscode-oniguruma@1.7.0` oracle on every
committed fixture, an empty divergence allowlist, and zero budget-degraded
lines on committed corpora.** A grammar that highlights fast but wrong does
not ship; a grammar that highlights right but blows the per-language
performance floor does not ship either.

## 1. Measured current state (2026-07-11, release build, Apple Silicon)

Baseline health: `cargo test -p mark-syntax --release` fully green;
`divergences.toml` empty; stripped `mark` binary 6.6 MB; bundle = 30 public
languages over 68 grammar blobs (38 assets are private embedding
dependencies declared in `coverage.toml`).

### 1.1 Throughput

Process-cold, one full-file pass, `profile-cold` driver:

| Corpus | Size | Throughput |
| --- | --- | --- |
| large-rust (`bench1.rs`) | 1.90 MB | **10.84 MB/s** (target 12) |
| libc++ `<string>` (cpp) | 184 KB | **1.06 MB/s** (was 0.69 last round) |
| core-repeated sweep (29 langs) | 3.26 MB | **1.15 MB/s aggregate** (2.83 s) |

Per-language sweep over the 108 KB repeated stress members, slowest first:

| Language | MB/s | | Language | MB/s |
| --- | --- | --- | --- | --- |
| nix | **0.07** | | powershell | 2.52 |
| php | 0.75 | | javascript | 2.57 |
| cpp | 0.78 | | jsx / csharp | 3.2 |
| sql | 0.85 | | bash | 3.45 |
| scss | 1.27 | | swift / c | 4.5 |
| markdown | 1.34 | | terraform / ruby | 5.0 |
| html | 1.68 | | go / java | 5.6 |
| tsx | 1.83 | | python | 6.1 |
| css | 1.95 | | make / yaml | 7.1 |
| typescript | 2.24 | | lua…json | 12.5–227 |

### 1.2 Root-cause diagnoses (counters + `sample` call stacks)

**nix — frame-stack identity is quadratic in nesting depth (structural, not
regex).** 32,560 state-cache misses over 4,441 lines; pattern hotspot time is
~20 ms of a 1,635 ms run. A 10 s `sample` of the tokenizer shows ~97% of
samples in `Frame as PartialEq::eq`, `FrameStack as PartialEq::eq`, and
`current_stack_with_base` (plus `memcmp` under them). Every line on a deep
stack pays O(depth) interning equality plus O(depth) scope-stack
reconstruction, and deeply nested attrsets mint a new state per line.
Line-cold reruns get *slower* (1.43 s → 2.4 s steady state) as the interner
table fills with near-identical deep stacks. This is the single worst number
in the engine (23× slower than the next-worst language) and is pure state
maintenance.

**cpp — fallback VM volume plus quality-relevant degradation.** 54.6 M
fallback steps on the 108 KB stress member, 640 k fallback attempts, and
**1,229 degraded lines** (budget kills). The dominant patterns are the
C-family "comment-or-space" idiom
`((?:\s*+/\*(?:[^*]++|\*+(?!/))*+\*/\s*+)+|\s++|(?<=\W)|(?=\W)|^|\n?$|\A|\Z)`
inlined into hundreds of rules, plus the giant scope-resolution and
function-declaration patterns. Degraded lines are wrong tokens, so this is a
100%-quality blocker, not just a speed item.

**sql — huge case-insensitive literal alternations on the DFA path.** 442 k
DFA attempts; top hotspots are `\b(?i)(abort|…hundreds of keywords…)\b` and
similar `(?i:…)` keyword lists. Each attempt walks an enormous alternation;
there is no dedicated multi-literal word matcher.

**php — candidate-set breadth.** 1.47 M candidate patterns considered for
108 KB, prefilter hit rate 3.5% (1.43 M checks → 50 k hits). Individually the
checks are cheap; collectively they dominate. Needs per-candidate-set
aggregated prefiltering rather than per-pattern checks.

**typescript/tsx/javascript/markdown/html/css/scss** — mid-tier (1.3–2.2
MB/s): a mix of the php-style candidate breadth and cpp-style
lookaround-heavy fallback patterns (TS family: 64% of patterns route to
fallback; markdown: 95%, driven by 266 `\G` anchors and 69 backreferences).

### 1.3 Grammar feature inventory (68 vendored assets)

`tools/grammar-stats.mjs`: 12,592 patterns; **49.8% route to the fallback
backtracker** (4,711 lookaheads, 2,884 lookbehinds, 842 possessive/atomic,
669 `\G`, 334 backreferences). Worst offenders by fallback count: cpp (628),
cpp-macro (410), javascript/jsx/tsx (342 each), typescript (330), markdown
(297/95%), latex (271), csharp (255), less (247), swift (231).

### 1.4 Catalog gap vs Shiki

`@shikijs/langs@3.23.0` contains **253 unique grammars** (~7.1 MB compact
JSON); Mark bundles 68 (30 public). Gap: 185 grammars. The pinned package is
on disk (`SOURCE.toml [import] source_path`), so vendoring stays a pure-Node
dev step with no new network dependency.

### 1.5 Requested languages, measured

Extracted from the pinned Shiki package and run through `grammar-stats.mjs`:

| Language | Scope | Patterns | Fallback % | Compact size | Engine risk |
| --- | --- | --- | --- | --- | --- |
| zig | source.zig | 56 | 9% | 5 KB | trivial |
| llvm | source.llvm | 26 | 4% | 5 KB | trivial |
| asm (x86-64) | source.asm.x86_64 | 309 | 2% | 38 KB | keyword-list heavy → needs §2.3 literal-set matcher to be fast |
| riscv | source.riscv | 47 | 21% | 6 KB | trivial |
| mipsasm | source.mips | 18 | 11% | 3 KB | trivial (free companion for riscv/asm) |
| odin | source.odin | 126 | 13% | 16 KB | easy |
| ocaml | source.ocaml | 350 | **68%** (222 lookaheads, 124 lookbehinds) | 59 KB | **heavy** — TS-class fallback density |
| mojo | source.mojo | 356 | **50%** (MagicPython derivative) | 66 KB | heavy-ish; python currently runs 6.1 MB/s so tractable |
| mlir | — | — | — | — | **not in Shiki**; vendor from llvm-project `mlir/utils/vscode` (Apache-2.0 WITH LLVM-exception), add to `[[additional_sources]]` like yang |

`catalog.rs` already pre-stages aliases for all of these (`ml`/`mli`→ocaml,
`ll`→llvm, `mlir`→mlir, `risc-v`→riscv, `s`→asm, `mips`→mipsasm,
`x86asm`→x86-64-assembly), so detection is a bundle-registration problem, not
new alias design.

## 2. Phase P — engine performance (before any catalog growth)

Rationale for ordering: ocaml and mojo have the same fallback density that
makes cpp/typescript slow today, and "100% quality" requires zero degraded
lines, which is itself an engine-efficiency problem (raising budgets was
already measured to be a dead end: 10× budgets = 1.4× slower, only 5% fewer
mismatches). Land these four items first so every new grammar arrives on a
fast engine.

Methodology for every item: alternating-order separate-process A/B, paired
medians, on large-rust + libcxx-string + core-repeated sweep; retain only
repeatable wins; byte-exact scope-stream comparison against the current
binary and the Node oracle before/after. (Reverted-experiment list in
`docs/textmate-engine.md` and the memory notes still applies — do not retry
per-candidate memoization, linear-only bytecode, position-only subroutines,
or 10× budgets as-is.)

### P1. Persistent hash-consed frame stack (kills the nix pathology)

Replace the `Arc<Vec<Arc<Frame>>>` + full-structure `PartialEq` model with a
parent-linked persistent stack: each `Frame` interned once into a frame table
(id = u32), each stack node = `(parent_stack_id, frame_id)` interned in a
stack table. Then:

- state identity/equality = one u32 compare (removes `Frame::eq` /
  `FrameStack::eq` memcmp storms — 97% of nix samples);
- push/pop = O(1) with no vector clone or Arc refcount churn;
- `current_stack_with_base` becomes a cached scope-stack per interned stack
  id, computed once per stack node, reused across lines and states.

Acceptance: nix stress ≥ 3 MB/s (≥ 40×), no regression > 1% on any sweep
member, scope streams byte-identical corpus-wide.

### P2. Finish the bytecode cutover for ordered alternation + repetition

`regex/bytecode.rs` (CutStart/CutEnd, ScanRepeat, landing pads) already
covers parts; the measured remaining fallback volume is recursive-VM fanout
on alternation/repetition (cpp 54.6 M steps, markdown/TS families). Two
concrete sub-items:

1. **Comment-or-space idiom recognition.** The exact C-family subpattern
   above appears (inlined) across c/cpp/cpp-macro/objective-c hundreds of
   times. Detect it structurally at translate time and compile to a dedicated
   deterministic scanner (skip whitespace/block-comments, else zero-width at
   word boundary). It is anchored, self-delimiting, and verifiable: add an
   exhaustive differential conformance case against Oniguruma before enabling.
2. **General iterative bytecode for ordered alternation/bounded repetition**
   with capture-observability gating as in the position VM: selection on
   positions, winner replay for captures.

Acceptance: cpp stress ≥ 2 MB/s, libcxx ≥ 2.5 MB/s, **degraded lines on the
cpp stress corpus and libcxx → 0**, fallback_steps_total on cpp stress cut
≥ 5×. Oracle-mismatch count on libcxx must not increase (target: decrease,
see Q2).

### P3. Multi-literal word-set matcher + aggregated candidate prefilters

1. **Literal-alternation specialization.** In `translate.rs`, detect
   `\b(?i)(w1|w2|…)\b` / `(?i:\b(…)\b)` alternations of plain words (SQL has
   hundreds; asm/x86-64 is 309 patterns of mostly this shape). Compile to:
   scan `[0-9A-Za-z_]+` token, then case-folded lookup in a length-bucketed
   perfect-hash/FxHash set. O(token) instead of O(alternation).
2. **Candidate-set aggregated prefilter.** Per candidate list (already cached
   per state), build a union first-byte bitmap + required-literal cursor set
   so the scanner jumps directly to the next plausible position once, instead
   of 1.4 M per-pattern prefilter probes (php). This preserves the unified
   ordered scan (the 2.14×-slower per-pattern search mistake stays dead).

Acceptance: sql ≥ 4 MB/s, php ≥ 3 MB/s, csharp/bash ≥ 5 MB/s, no sweep
regression.

### P4. Lazy ordered frontier (carried over — the last structural step)

Single-pass mixed regular/advanced candidate traversal in grammar order with
lazy evaluation, replacing scan-all-then-select. This was already identified
as the next big engine step; it primarily lifts the mid-tier (TS family,
css/scss, markdown) where candidate breadth × fallback cost multiply.

Acceptance: sweep aggregate ≥ 6 MB/s (stretch 8), large-rust ≥ 12 MB/s
(closes the original target), typescript ≥ 4 MB/s.

## 3. Phase Q — quality to 100% (hard gates, run with Phase P)

1. **Q1: zero degradation on committed corpora.** Budget kills remain as a
   safety valve for adversarial input, but any budget kill on a committed
   fixture or benchmark corpus is a release blocker. Add a
   `profile-counters`-based CI assertion (`degraded_lines == 0`,
   `fallback_budget_kills == 0`) per corpus.
2. **Q2: libc++ divergence → 0.** The remaining divergent lines vs the pinned
   oracle trace to the recursive-VM lazy-repeat + nested `{0,1}` overshoot
   (bytecode already matches Oniguruma). Finish the bytecode migration of the
   affected patterns (P2) and re-pin the libc++ stream; target 0 divergent
   lines, tracked by sha256 of the full stream.
3. **Q3: conformance is inventory-driven.** Before any grammar lands,
   `grammar-stats.mjs` runs over it; every regex construct it uses that is
   not already in `tools/regex-conformance.mjs`'s proving set gets a
   conformance case first. The 253-grammar catalog defines the closure of
   constructs we must prove, nothing more.
4. **Q4: fixture policy per language.** Every public language ships with at
   least: one `basic` fixture, one `stress` fixture (real-world file, ≥ 100
   lines, exercising strings/comments/nesting), goldens regenerated by the
   pinned oracle with `stoppedEarly: false`, exact + coarse parity in
   `textmate_golden.rs`, and `divergences.toml` stays empty. Non-ASCII
   content required where the language plausibly encounters it.

## 4. Phase G — catalog expansion

### G0. Promote the 38 vendored private assets (cheap, immediate)

Most private blobs are real user languages already in-tree, licensed, and
bundled: clojure, coffee, dart, diff, elixir, erlang, fsharp, git-commit,
git-rebase, groovy, handlebars, ini, jsonc, jsonl, julia, latex, less, log,
objective-c, perl, pug, r, raku, rst, scala, vb, xml, xsl, yang, bat, abap,
bibtex, twig. Promotion = `coverage.toml` public entry + catalog
aliases/extensions + fixtures/goldens per Q4 + sweep corpus member per
language. No new assets needed. Target: public catalog 30 → ~65.

### G1. Requested languages (the user tier — lands first among new assets)

Order by measured risk, each with the full Q4 + perf gate treatment:

1. **zig, llvm, riscv, mipsasm, odin** — light grammars (≤ 21% fallback);
   expected to land in the fast tier (≥ 8 MB/s) with no engine work. Restore
   the removed zig stress fixture. Real-world stress sources: zig stdlib
   file, `clang -S -emit-llvm` output, riscv/mips `.s` from gcc cross output,
   odin demo sources.
2. **asm (x86-64)** — needs P3's literal-set matcher to hit the floor (309
   keyword-list patterns); land after P3, gate ≥ 4 MB/s on an objdump-style
   stress corpus.
3. **mojo** — MagicPython-derived; gate against python's current 6.1 MB/s
   class (floor ≥ 3 MB/s) and reuse python fixtures approach; verify its 15
   backreferences and 2 `\G` uses are conformance-covered.
4. **ocaml** — heaviest (68% fallback, 222 lookaheads). Requires P2 + P4
   landed, then a dedicated counters audit like the cpp one; floor ≥ 2 MB/s
   on an ocaml stdlib stress file, zero degradation.
5. **mlir** — vendor `grammar.json` from llvm-project `mlir/utils/vscode`
   (pin a llvm-project release tag; Apache-2.0 WITH LLVM-exception →
   `licenses.json` + `[[additional_sources]]` in SOURCE.toml, exactly like
   yang). Add `.mlir` extension entry; oracle + fixtures identical to Shiki
   grammars since the oracle takes raw JSON.

### G2. Full Shiki parity (185 remaining grammars, tiered)

Tier by (a) expected user exposure in a terminal markdown/diff viewer and
(b) measured pattern weight:

- **Tier 1 (common, mostly light):** haskell, elm, gleam, graphql, prisma,
  solidity, cmake, glsl/wgsl/hlsl, proto, nim, crystal, haxe, d, ada, tcl,
  racket/scheme/common-lisp, matlab, wasm, verilog/vhdl, svelte, vue, astro,
  fish, nushell, gherkin, dotenv, ssh-config, systemd, nginx, http, csv/tsv,
  desktop, jinja, liquid…
- **Tier 2 (heavy or niche):** fortran-free/fixed (551 patterns / 66%
  fallback — treat like ocaml), latex ecosystem completion, angular family,
  vue sub-grammars, apex, bsl/sdbl, wolfram, stata, sas, mermaid, typst,
  lean, coq, agda-class exotica, wenyan, etc.

Mechanics are identical for every grammar; ship in review-able batches of
10–20 with the per-language checklist:

1. Vendored compact JSON from the pinned package (extend the vendor script
   to all of `dist/`, dedupe by grammar `name`, stable key order).
2. `licenses.json` entry (the package carries per-grammar license metadata).
3. `coverage.toml` public entry; aliases/extensions from Shiki's
   `bundledLanguagesInfo` merged into `catalog.rs` (id + aliases must match
   Shiki so user-facing language ids are Shiki-compatible).
4. `grammar-stats` inventory → conformance gaps closed first (Q3).
5. Fixtures + oracle goldens, exact parity, zero degradation (Q4).
6. Perf gate: process-cold stress ≥ 2 MB/s floor per language, with a
   counters audit + tracked issue for anything below (a floor breach never
   merges silently).
7. Sweep corpus member added so `corpora.toml` aggregate tracks the full
   catalog forever.

### G3. Bundle size strategy

7.1 MB compact JSON for 253 grammars vs today's 68-blob bundle inside a
6.6 MB binary. Plan: per-blob DEFLATE (pure-Rust `miniz_oxide`, dev-dep-free
at runtime decode) with the existing lazy per-language decode — JSON grammars
compress 4–6×, so the full catalog should add roughly 1.5–2 MB to the binary.
Gate: full-catalog stripped binary ≤ 9 MB; if exceeded, split builds
(`core`/`full` cargo feature) rather than dropping quality. Decompression
happens once per language on first use and must stay < 2 ms per grammar
(measure in `profile-cold` process-cold mode, which already includes it).

## 5. Phase I — infrastructure to keep it honest

1. **Vendor automation:** `tools/vendor-shiki-grammars.mjs` (generalize the
   markdown vendor script): reads the pinned package, emits/refreshes all
   grammar JSONs + `licenses.json` + a coverage skeleton; CI check that
   vendored assets match the pin (like `generate-goldens.mjs --check`).
2. **CI jobs:** (a) `cargo test` incl. golden harness; (b) Node job:
   `generate-goldens.mjs --check` + `regex-conformance.mjs`; (c) perf smoke:
   process-cold floors on large-rust/libcxx/sweep with generous (2×)
   regression margins to stay flake-free; (d) counters assertion job (Q1).
3. **Benchmarks:** extend `benchmarks/textmate/corpora.toml` with one stress
   member per public language (generated corpus builder already exists);
   record floors next to each corpus.
4. **Docs:** `textmate-engine.md` catalog table becomes generated output from
   `coverage.toml` (353-row hand-maintained tables rot).

## 6. Sequencing and acceptance summary

| Step | Gate to advance |
| --- | --- |
| P1 frame stack | nix ≥ 3 MB/s, corpus-wide byte-exact streams |
| P2 bytecode alternation/repetition + idiom | cpp ≥ 2 MB/s, libcxx ≥ 2.5 MB/s, degradation = 0 |
| P3 word-set matcher + aggregated prefilter | sql ≥ 4, php ≥ 3 MB/s |
| Q1/Q2 | zero degraded lines; libc++ divergence = 0 |
| G0 promote 38 | all fixtures exact; public catalog ~65 |
| G1 zig/llvm/riscv/mips/odin → asm → mojo → ocaml → mlir | per-language floors + exact parity |
| P4 frontier | sweep ≥ 6 MB/s, large-rust ≥ 12 MB/s |
| G2 tiers 1–2 (batches) | checklist per batch; sweep tracked |
| G3 compression | full catalog, binary ≤ 9 MB |

Fast-follow candidates deliberately **not** in scope: semantic-token overlays
(out of TextMate scope), theme work, and any hand-written lexer shortcuts —
coverage remains an asset problem per the engine charter.
