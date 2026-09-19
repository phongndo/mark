# Contributing

Use [development](docs/development.md) for setup and verification. For changes
across subsystems, read the [architecture boundaries](docs/architecture.md).
Changes to the bundled review skill also need the
[real-agent evaluations](docs/agent-review-evals.md).

In a pull request, explain the problem, behavior or compatibility changes, and
risks. Record the verification commands actually run and anything left untested.

## Documentation policy

Keep docs for user workflows, operational prerequisites, and reasons behind
constraints. Use code, tests, CLI help, and configuration as the authority for
facts they already express.

- Keep one home for each explanation. Link to it rather than copying it.
- Use small examples that demonstrate a choice, not dumps of every default.
- Link to the owning source for option inventories, dependency versions,
  runtime budgets, and CI rules.
- Put enforceable invariants in tests or checks; document the reason when it
  is not evident from the implementation.
- Keep benchmark evidence with the change: identify the revisions, host,
  commands, inputs, and limitations. Store raw local artifacts under `target/`.
- Remove completed plans, status snapshots, and obsolete instructions from the
  working tree; Git history retains them. Preserve any still-relevant rationale
  beside the code or in the existing architecture guide.

Background: OpenAI's [harness-engineering account](https://openai.com/index/harness-engineering/)
warns that monolithic manuals become stale and recommends a small map to
maintained sources—not eliminating documentation. Here, documentation must add
context rather than become a second description of the implementation.
