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
than per-operation pools, and keep CPU-bound work off Tokio workers. Syntax work retains a [dedicated queue](../crates/mark-tui/src/syntax/queue.rs)
because visible work must outrank speculative prefetch, not compete through
unprioritized work stealing.

## Syntax and themes

[Syntaxmate](https://github.com/phongndo/syntaxmate) owns tokenization, grammars,
and tokenizer compatibility. Mark consumes its public crates.io API;
[mark-syntax](../crates/mark-syntax/src/lib.rs) owns product settings, language
mappings, and theme adaptation. Keep engine fixes upstream rather than copying
internals into Mark. [Theme provenance](../assets/themes/SOURCE.toml) and
[scripts/ci/generated](../scripts/ci/generated) own asset pins and validation.

[Scope adaptation](../crates/mark-syntax/src/scope_table.rs) retains one public
token per distinct document scope stack. Mark caches resolved property-match
flags for the base theme and user overrides in two bounded slots; Syntaxmate's
diagnostic resolver does not cache those flags. Scope memory is estimated from
exposed content and Mark's allocations, since engine storage is private.

## Live sessions

[mark-session](../crates/mark-session/src/lib.rs) owns UI-independent live
session transport and the existing durable machine-review interface. Standalone
TUI sessions retain their in-memory lifetime. Checkpoint validation, atomic
replacement, and process locking remain behind that module.

Harness commands use a one-shot terminal annotation session. Pi and OpenCode
release their renderer while Mark owns the inherited terminal; Claude uses a
native terminal pane. Shift-Q immediately returns all marks as Markdown; the marks
menu separately supports returning a selection. Back returns an empty result.
Neither the snapshot nor annotations are written to the durable store.

Native adapters put that text in the initiating conversation’s input as a draft.
The human sends it with Enter; the handoff starts no agent turn. They register no
MCP and manage no review IDs, acknowledgements, history, or subsequent agent
work. The existing bridge/MCP machine interfaces continue to use durable
records when called explicitly. The optional legacy browser viewer remains an
explicit machine API, never an integration fallback. Installation and host
limits are described in [integrations](integrations.md).
