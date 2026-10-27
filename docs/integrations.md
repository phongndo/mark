# Harness integrations

Mark's native packages bundle the host command, review-loop skill, and local MCP
connection. They share review records while adapting presentation and feedback
delivery to each host. Install Mark 0.16.0 or later through your preferred
package manager; the packages run that executable. Node-based helpers require
Node 22.19 or later. npm extensions do not install the Mark executable.
`MARK_BIN` selects an explicit executable for Node-based adapters.

`mark integration path --agent NAME` materializes a version-matched package from
the installed binary and prints its directory. It needs no source checkout or
Node to materialize files. `mark integration export --agent NAME --output DIR`
creates a standalone package for inspection or distribution. Modified existing
files are preserved with an error; use the new versioned cache or a fresh export
directory when upgrading.

## Install and invoke

For Pi, install the npm extension, run `/reload` or start a new session, then
run `/mark`:

```sh
pi install npm:mark-pi
```

For Claude Code, add Mark's marketplace and install the plugin:

```text
/plugin marketplace add phongndo/mark#integrations
/plugin install mark@mark
```

Start a new session and invoke `/mark`. The package contains a native mod
command, a namespaced review-loop skill, and MCP.

For OpenCode V2, install the npm plugin, restart, and run `/mark` in a
conversation:

```sh
opencode plugin add mark-opencode
```

This package targets the V2 plugin interface and requires a renderer exposing
`suspend()` and `resume()`.

For Codex, add the marketplace, install Mark, and start a new conversation:

```sh
codex plugin marketplace add phongndo/mark --ref integrations
codex plugin add mark@mark
```

Invoke `$mark` or select its skill. The skill's host instructions open an exact
local review URL in the desktop app's built-in browser and wait for submitted
feedback. Its browser entry helper uses `CODEX_THREAD_ID` for binding.

For fx, materialize its skill/MCP package:

```sh
mark integration path --agent fx
```

In fx, install the printed directory's `skills/mark` subdirectory and add MCP:

```text
/skills install /absolute/printed/path/skills/mark
/mcp add mark -- mark mcp --harness fx
```

Invoke `$mark`. The skill carries its helper and shared module with it, and can
also use the CLI bridge when MCP is unavailable. Its `HOST.md` explains the
supported entry. fx currently does
not expose the active conversation ID to shell tools; its helper requires the
exact ID from `/status` once and pins the returned review. It refuses to guess
from recent sessions.

Every [stable release](https://github.com/phongndo/mark/releases/latest) also
includes standalone `mark-HOST-vVERSION.zip` packages with the skill, MCP
configuration, and host files. For a local installation without npm or a
marketplace, use `mark integration path --agent NAME`; for example:

```sh
pi install "$(mark integration path --agent pi)"
claude --plugin-dir "$(mark integration path --agent claude)"
```

Claude's generated directory can also be added as a local marketplace with
plugin `mark@mark-local`. OpenCode accepts the generated directory in its
`plugins` array; Codex accepts its local marketplace. Versioned npm specs such
as `npm:mark-pi@0.16.0` and `mark-opencode@0.16.0` pin a particular release.

## Host capabilities

Checked against available interfaces on 2026-10-27. A skill/MCP bundle cannot
supply missing host UI or session hooks.

| Host surface | Presentation | Validation and limits |
| --- | --- | --- |
| Pi interactive terminal | Existing Mark TUI in the same terminal | Real handoff, Back, reopen, saved notes, and Submit exercised with Pi 1.4.0. Delivery is acknowledged after Pi records the message. |
| Claude Code | Native `/mark` review pane | Form, patch retrieval, notes, replies, and submission exercised with Claude 2.1.312's plugin test runner; terminal, Desktop, and VS Code render trees checked. |
| OpenCode V2 terminal | Existing Mark TUI in the same terminal | Real renderer suspension and return exercised with OpenCode 2.2.0; server registration and adapter delivery also tested. |
| Codex desktop | Local browser review viewer | Viewer interaction tested in Chromium. The desktop app's host round trip requires verification there. |
| Codex CLI | Skill and MCP | The checked plugin interfaces do not establish a terminal handoff or custom `/mark`. Browser entry requires a browser-capable host. |
| fx | Skill/MCP and browser helper | Custom `/mark`, same-terminal handoff, and automatic conversation identity are unavailable through the checked interfaces. |

Claude's [mod interface](https://code.claude.com/docs/en/plugins/mods/interface)
uses native panes with host-controlled layouts. OpenCode's
[V2 plugin interface](https://dev.opencode.ai/v2/docs/build/plugins/cli),
Codex's [skills](https://learn.chatgpt.com/docs/build-skills) and
[browser](https://learn.chatgpt.com/docs/browser), and fx's
[slash commands](https://fx.sh/docs/using-fx/slash-commands) document their
respective entry points. These packages use existing host extension interfaces.

Host lifecycle tests use isolated fixtures without paid model calls. They verify
handoff and delivery behavior; they do not establish the quality of a particular
model's subsequent edits. Follow [agent evaluations](agent-review-evals.md) for
that separate check.

## Review loop

Opening from a native command binds the review to the originating conversation,
canonical Git worktree, and source. The default includes staged, unstaged, and
untracked worktree changes. A durable review ID is distinct from a live process
session ID. Another conversation or worktree receives a separate binding.

In the terminal, save a note with `Ctrl-S`. `q` returns and keeps the review;
`Q` submits saved human feedback and returns. Native panes and the browser
viewer provide Back and Send actions. Saving a note does not submit a request.
Unsaved compose text stays in the current view.

The adapter delivers selected feedback to the originating conversation. Each
submission contains immutable message versions and explicit thread/reply IDs.
Delivery failures preserve pending feedback; retry from that same conversation.
An acknowledgement records accepted delivery, not approval of the code.

Reopening loads the next source snapshot while preserving review identity. Mark
re-anchors comments where evidence is unambiguous, marks uncertain targets
stale, preserves progress for unchanged files, and clears a prior verdict when
changes require another pass. The agent uses its ordinary editing and
verification tools; final approval remains yours.

## Local state and machine access

Integrated records live under `$XDG_STATE_HOME/mark/reviews`, or
`~/.local/state/mark/reviews`. Checkpoints use private permissions, atomic writes,
and process locks. They retain snapshot evidence and feedback after the UI exits.
Standalone `mark diff` sessions retain their existing volatile lifetime.

Records are bounded: patches 2 MiB, checkpoints 4 MiB, 256 threads, 1,024
messages, 128 submissions, and 512 distinct mutations. Limit errors preserve
the previous checkpoint. A fresh harness conversation creates a separate
review. MCP patch and thread retrieval are paginated; follow continuation cursors.

`mark mcp` runs the local stdio server. `mark bridge --help` describes the shared
machine interface. MCP exposes submitted human feedback and agent findings,
with drafts and human-only controls kept in the UI. The CLI bridge is the full
local interface used by native adapters.

Browser viewers bind to loopback, use a capability URL scoped to one review,
reject foreign origins, and stop after an hour without requests. Open the exact
returned URL inside the host. No hosted backend or Mark account is required.
