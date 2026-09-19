# Architecture boundaries

These are design constraints, not a file-by-file description of the code.
[scripts/check-architecture](../scripts/check-architecture) owns enforceable
import rules, dependency boundaries, and seam-file size budgets.

## UI ownership

`DiffApp` is the composition root, not the home for every feature. Keep behavior
with the state/controller that owns its invariant; use focused context traits
so event routing and rendering can be tested without the whole application.
The [app](../crates/mark-tui/src/app/) and
[render compositor](../crates/mark-tui/src/render/compositor.rs) are the entry
points for these boundaries.

Prepare mutable state before drawing. Leaf renderers should read it; the diff
viewport is the deliberate exception because visible rows warm lazy context,
syntax, and inline caches. Keep materialization limited to the visible window
where possible; that does not make wrapping or source lookup constant-time.
Retain Ratatui buffer diffing and owned viewport output unless measurements
justify the extra lifetime or terminal-state complexity of an alternative.

Queue [AppEffect](../crates/mark-tui/src/app/effect.rs) values for external work
such as editor launch, clipboard writes, and settings persistence. This keeps
terminal pauses and I/O out of domain mutation and event routing.

## Concurrency

Use the lazy process-wide [CPU pool](../crates/mark-runtime/src/lib.rs) rather
than per-operation pools. Async callers use `run_cpu` so CPU work cannot block
a Tokio worker. Syntax work retains a [dedicated queue](../crates/mark-tui/src/syntax/queue.rs)
because visible work must outrank speculative prefetch, not compete through
unprioritized work stealing.

## Syntax and themes

[Syntaxmate](https://github.com/phongndo/syntaxmate) owns tokenization, grammars,
and tokenizer compatibility. Mark consumes its public crates.io API;
[mark-syntax](../crates/mark-syntax/src/lib.rs) owns product settings, language
mappings, and theme adaptation. Keep engine fixes upstream rather than copying
internals into Mark. [Theme provenance](../assets/themes/SOURCE.toml) and
[scripts/ci/generated](../scripts/ci/generated) own asset pins and validation.

## Live sessions

[mark-session](../crates/mark-session/src/lib.rs) is a UI-independent transport
and protocol boundary. Review state belongs to the live TUI, not a daemon or
persistent review database. Agents contribute findings; human navigation,
reviewed state, dispositions, and verdicts remain human-owned. Session lifetime
and reload behavior are described in [usage](usage.md#live-agent-review-sessions).
