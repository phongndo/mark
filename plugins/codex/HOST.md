# Codex entry

Invoke this package's skill with `$mark`. Codex CLI does not expose a verified custom `/mark` command or terminal suspension interface.

For a new review, resolve `../../open-review.mjs` from this skill's directory and run that absolute script path with Node from the reviewed repository. The helper requires the host's `CODEX_THREAD_ID`; it fails rather than choosing an unrelated conversation. It starts Mark's local browser review and returns the review ID and URL. Open the URL with the host's built-in browser capability and present it as a clickable link in the conversation. A remote worker's localhost is not automatically reachable from the desktop client. If no local browser capability is available, report the unsupported UI handoff instead of claiming a complete review loop.

Use `feedback_wait` for the pinned review ID in bounded waits until the human submits, chooses Back, or cancels. After submission, use `feedback_get` for that same review ID and continue with the shared loop. Draft comments are not a submitted request. Resume the same review for the next pass. The CLI can use this browser route when a local browser is available; running the MCP server does not give it ownership of Codex's terminal.
