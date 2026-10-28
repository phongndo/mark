# Harness integrations

Mark is a terminal annotation tool. Open the current diff, save notes, then
return selected annotations as a Markdown draft in the chat input. A note can be a
question, observation, or request; Mark adds no instruction to change code.

## Install and invoke

Install a matching Mark executable and harness package. npm packages do not
include the executable. `MARK_BIN` selects a local binary. Node helpers require
Node 22.19 or later.

| Host | Install | Invoke |
| --- | --- | --- |
| Pi | `pi install npm:mark-pi` | Restart or `/reload`, then `/mark` |
| OpenCode V2 | `opencode plugin add mark-opencode` | Restart, then `/mark` in a conversation |
| Claude Code | `/plugin marketplace add phongndo/mark#integrations`, then `/plugin install mark@mark` | New session, then `/mark` |
| Codex | `codex plugin marketplace add phongndo/mark --ref integrations`, then `codex plugin add mark@mark` | `$mark:mark` |
| fx | `mark integration path --agent fx`, then install its `skills/mark` directory | `$mark` |

Standalone packages are available through `mark integration path --agent NAME`,
`mark integration export`, and stable release ZIPs. Export to a fresh directory
when testing modified files; existing local edits are preserved.

## Annotate, then return to chat

The diff includes staged, unstaged, and untracked changes in the worktree where
you invoked Mark. In the TUI, select a file, hunk, or line and press Enter to
annotate. **Ctrl-S saves a note without sending anything.**

**Shift-Q immediately returns all marks to the chat draft and closes Mark.**
It also saves an open annotation draft. There is no confirmation or selection
screen. With no marks, it simply returns to chat.

For a subset, open the **marks list with `n`**. Move with `j`/`k`, press Space to
select (it moves to the next mark), `a` to select all, and Ctrl-S to return the
selection; with nothing selected, Ctrl-S returns the highlighted mark. Enter
edits a note, `d` deletes it, and `/` filters. Escape closes the list so you can
add more. In the note editor, Enter adds a line and Ctrl-S saves; Escape keeps a
new mark rather than discarding it. Shift-Q always returns all marks, including when the list is open. `q`
in the diff returns without adding notes to chat.

Submitting annotations starts no agent turn. Edit the draft, add context, or
discard it. **Press Enter in chat when you want to send it.**

The draft contains only Markdown, for example:

```markdown
- `CMakeLists.txt:17` — What does this mean?
```

Answers, discussion, and implementation reports belong in chat. An annotation
does not inherently request a fix. Every invocation starts fresh; the handoff
uses no durable review ID, history, acknowledgement, or automatic reopen. The
packages register no Mark MCP server. Existing live-session and machine APIs
remain available separately.

## Terminal ownership

Pi stops its renderer while Mark owns the terminal, then restores it before
filling the initiating conversation’s input. OpenCode uses its
renderer suspend/resume interface and its Paste command to insert annotations
through the clipboard (macOS `pbcopy`, Wayland `wl-copy`, or X11 `xclip`). Both
use an inherited interactive terminal. Existing chat drafts are preserved.
Claude Code cannot lend its terminal to a child process, so its `/mark` opens
Mark in a split beside the chat: a Rex block when Claude runs in Rex, otherwise a
tmux pane. When Mark closes, returned notes are appended to the chat input; if
the input is unavailable they go to the clipboard. Outside Rex and tmux, run
`mark diff` in another terminal.

Codex and fx use a helper that requires inherited terminal input/output. A host
must expose an interactive terminal and keep the process running while you
annotate. Their helper returns Markdown through the tool result; it does not
submit a chat message. Wait for the human to send their next message before
responding to annotations. If the host cannot expose a terminal, open Mark in a
separate terminal and paste the Markdown into chat. There is no automatic HTML fallback.

## Local development in a dirty worktree

Build once in the Mark checkout:

```sh
nix develop -c cargo build -p mark-cli --locked
```

From the worktree you want to annotate, launch the source Pi extension:

```sh
/absolute/path/to/mark/scripts/pi-mark
```

This pins the local binary and extension and disables installed extensions and
skills for that invocation, so an old installed `mark-pi` cannot override the
changed code. It does not modify Pi settings. Start a new Pi process with this
command after changing the binary or extension. Then run `/mark`.

The equivalent manual command is:

```sh
MARK_BIN=/absolute/path/to/mark/target/debug/mark \
pi --no-extensions --no-skills -e /absolute/path/to/mark/assets/integrations/pi/index.js
```

Checks: `just ci-integrations`, `just test-pi-review`, and
`claude plugin test assets/integrations/claude`. They use local fixtures without
paid model calls. `scripts/test-browser-review` covers the separate, explicitly
invoked legacy browser viewer; integrations do not open it.
