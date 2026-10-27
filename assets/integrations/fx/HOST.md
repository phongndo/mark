# fx entry

Invoke the installed skill with `$mark`. This package supports Vercel Labs fx.

For the first review, use the exact current fx session ID supplied by the human or host. If it is unavailable, ask the human to copy it from `/status`. fx's documented shell environment identifies the harness with `AI_AGENT=fx`; it does not provide a current conversation ID. A remembered or latest session from `fx sessions` is insufficient when several sessions share a workspace. Keep the supplied conversation ID and returned review ID pinned for subsequent passes.

Resolve `./open-review.mjs` from this skill's directory. Run its absolute path with Node from the reviewed repository, passing `--conversation` and the exact session ID as separate arguments. The helper and its common module are included inside this skill, so installing this skill alone retains the handoff. The helper opens or refreshes the bound review and starts Mark's browser viewer. Present its URL as a clickable link; use an available browser capability to open it. If the local viewer is unreachable from the host, report that limitation. An MCP connection alone does not display the review.

Wait through `feedback_wait` with bounded waits for that review until submission, Back, or cancellation. If Mark MCP is unavailable after a skill-only install, send the same methods and JSON parameters through `mark bridge call METHOD --stdin --json`; for example, use `feedback.wait` with the pinned `review_id` and `timeout_ms`. On submission, read `feedback_get`/`feedback.get` and follow the shared review loop in this conversation. A timeout means the review is still open. Preserve submitted thread/message IDs when replying. Refresh and reopen this same bound review after fixes.

The stock fx CLI has no supported custom `/mark` registration or terminal suspension extension. This package uses the documented skill and MCP surfaces; it does not inject keyboard input into fx or resume a different session to deliver feedback.

Support checked 2026-10-27 against [fx skills](https://fx.sh/docs/capabilities/skills), [MCP configuration](https://fx.sh/docs/capabilities/mcp), and [slash commands](https://fx.sh/docs/using-fx/slash-commands). The checked [upstream alias handler](https://github.com/vercel-labs/fx/blob/7f2383af70984197ac29f6f5ab3240efc25f8e1c/src/core/app/app_commands.zig#L1253) explicitly reports that aliases are not configurable.
