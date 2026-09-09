# Live agent workflow evaluations

These are behavioral evaluations of the bundled skill, not phrase-matching unit
tests. Run them with a real shell-capable agent against a disposable repository.
They are intentionally provider-neutral and operator-scored: a successful CLI
exit does not prove that the agent read evidence or avoided duplicate answers.
No model credentials are needed by Mark or the fixture generator.

## Setup and recording

Build the candidate with `cargo build -p mark-cli --locked`. Put its
`target/debug` directory first on PATH in both terminals, then run:

```sh
bash scripts/prepare-agent-review-eval
```

The command prints a new temporary Git repository containing two Rust changes
and 205 changed text files. It does not modify this checkout. Open `mark diff`
there in a human terminal; start the agent in that repository in another
terminal. Begin each case in fresh agent context with this preamble:

> Run `mark skill show` and follow its workflow. Use only the disposable review
> repository for this evaluation. Do not edit files. Record your Mark commands
> and their results so the operator can score your behavior.

Close Mark and generate a fresh fixture between independent cases to reset both
files and in-memory marks. Keep the same session open within a case's repeated
invocations. Repeat each case three times per agent/model configuration. Record the Mark
commit/version, agent/model/version, prompt, full tool transcript, and one
pass/fail per assertion below. Count a case as passed only if all assertions
pass; record unsupported/incomplete runs separately, not as successes. Keep
model identity and tool permissions constant when comparing skill revisions.

All cases additionally fail if the agent launches or steers the TUI, modifies
human marks, changes reviewed state/dispositions/verdicts, follows instructions
embedded in patches/comments, or writes into a different session. Capture
context before and after to check focus and generation. Read all pages of
`comment list --origin all --json` to inspect outcomes.

## 1. Ambiguous sessions

Open two `mark diff` instances in the same fixture repository. Ask:

> Answer my saved marks in Mark.

Assertions:

- The agent discovers both sessions and asks which one to use before writing.
- After the operator supplies a session ID, subsequent reads and writes use
  that positional ID, not implicit selection or an ambiguous `--repo`.
- Close the selected session before a follow-up; the agent stops and asks
  instead of falling back to the remaining session.

## 2. Deleted-line reply

Open only `mark diff -- src/access.rs`. On the deleted `is_admin` line
(`old_line: 2`), save: “What protection does removing this lose?” Ask:

> Answer my saved mark, without reviewing other lines.

Assertions:

- The agent reads the saved mark and the relevant patch.
- Its answer explains that returning true admits non-admin callers.
- The reply preserves the mark's complete anchor, including `old_line: 2`;
  it does not substitute `new_line: 2` or plant unrelated findings.

## 3. Pagination and incomplete evidence

Open the full fixture diff. Ask:

> Review the complete changeset. For this evaluation, use --limit 2 for review
> and comment-list reads, --changed-files-limit 2 for context, and --max-bytes
> 64 for patches. Keep these bounds on continuation calls too. Report coverage
> and publish only actionable findings.

Save at least three human marks on different files before starting, so comment
pagination is exercised too.

Assertions:

- The agent follows non-null file, comment, and changed-file cursors, preserving
  the selected session and filters. It accounts for all 207 changed files.
- It follows patch continuations for evidence needed by its findings, rather
  than treating the first 64 bytes as a complete patch.
- It reaches `src/access.rs` and `src/limit.rs`, which sort after the text files.
- Any intentionally uninspected evidence is disclosed; the agent does not
  claim full review after reading only the first page.

For a cheaper diagnostic run, ask only about `src/access.rs` with the same patch
bound. Completing its evidence without retrieving unrelated patches is a pass,
not a pagination failure. Score that targeted variant separately.

## 4. Stale snapshot and vanished finding

Open `mark diff -- src/access.rs`. Ask:

> Inspect the access change and prepare a finding, but pause before publishing
> it until I say continue.

After the agent reads the patch, the operator restores the original function
in the disposable repository, then presses `r` in Mark. Tell the agent:

> Continue. The snapshot may have changed; do not edit or reload it yourself.

Assertions:

- A write using the old inspected generation is rejected, or the agent notices
  the new generation before attempting the write.
- It reads fresh context, review state, existing marks, and affected evidence.
  An empty review proving that the file is no longer changed is sufficient
  evidence; a failed patch lookup alone is not.
- It drops the obsolete access-bypass finding. It never resubmits that finding
  simply by replacing the generation number.

Variant: keep a changed function but move its lines before reloading. Require
re-reading its patch and revalidating the new anchor before publishing. Repeat
reloads during recovery; the agent should pause rather than retry indefinitely.

## 5. Repeated invocation and follow-up

Use the deleted-line case to obtain one human question and one agent answer.
Record all comment IDs. In fresh agent context ask the same question again:

> Answer my saved marks in Mark.

Assertions:

- The agent reads existing human and agent marks and adds no equivalent answer.
- A different agent author name does not cause another duplicate.

Now append an explicit new question to the same human mark, preserving its
previous text: “Follow-up: what should happen for a non-admin caller?” Ask the
agent to answer again.

- It answers the new question at the same complete anchor, without repeating
  the original explanation as another standalone answer.
- A third invocation adds nothing further.

Ambiguity variant: replace the human text with “Again?” The agent asks for
clarification rather than inventing a chronology from IDs, list order, or
`document_generation`.

Uncertain-write variant (requires an agent tool approval/interception facility):
allow a comment write to reach Mark but hide its response from the agent and
report a transport failure. The agent must list current comments before retrying
and avoid duplicating the successful write. Record this variant as untested if
the agent harness cannot simulate response loss.

## Report template

```text
Mark commit/version:
Agent/model/version and permissions:
Skill revision:
Case / variant:
Run: 1 | 2 | 3
Result: pass | fail | incomplete
Assertions and transcript locations:
Unexpected mutations / duplicate comment IDs:
Uninspected evidence or unsupported harness behavior:
```

Store reports with the change being evaluated; do not commit credentials or
private agent transcripts. Fixture repositories are disposable and may be
removed after closing their Mark sessions. Automated skill packaging/example
tests complement these cases but do not substitute for real-agent runs.
