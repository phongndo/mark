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

[mark-session](../crates/mark-session/src/lib.rs) owns UI-independent transport,
protocol, and durable integrated review records. A live `session_id` identifies
one process endpoint; a durable `review_id` identifies a review bound to an
exact harness conversation, canonical worktree, and typed source. Standalone
TUI sessions retain their existing in-memory lifetime.

Checkpoint paths, schema validation, atomic replacement, and process locking
belong to `mark-session`. The TUI adapts annotations and progress through the
[integration](../crates/mark-tui/src/integration.rs) seam; viewport indexes,
render caches, and drafts being composed remain UI-owned. Snapshot generations
and review revisions serve separate purposes; feedback submissions retain
immutable message versions.

The CLI bridge loads source evidence through Mark's diff modules. Headless
refresh uses the existing review transition to preserve progress and re-anchor
comments consistently with a terminal reload. CLI, native panes, the local
browser viewer, and MCP use the shared record interface. `mark-mcp` owns MCP
framing, schemas, bounded responses, and the agent-facing projection; a callback
connects it to review behavior. Agents see submitted human feedback through
this projection. Human submission, navigation, reviewed state, dispositions,
and verdicts remain human-owned.

Harness adapters own presentation, exact conversation binding, and returning
accepted feedback to that conversation. MCP subprocesses do not own the user's
terminal. Browser viewers run on demand on loopback with a capability URL scoped
to one review and expire after inactivity. Lifecycle and installation are
described in [integrations](integrations.md).
