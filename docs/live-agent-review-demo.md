# Live agent review demonstration

This is the plain process-level dogfood flow used for the first live-session
release. The human owns the first terminal and the agent uses a separate shell.

```text
human$ mark diff

agent$ mark session list --json
{"protocol":1,"id":"cli",..."sessions":[{"session_id":"…","source":"worktree"}]}

agent$ mark session context --repo . --json
..."generation":0,"source_changed":false...

agent$ mark session review --repo . --json
..."files":[{"new_path":"a.txt","hunks":[{"index":1,...}]}]...

agent$ mark session patch --repo . --file a.txt --hunk 1 --json
..."patch":"diff --git a/a.txt b/a.txt\n...\n+new\n"...

agent$ printf '%s' '{
  "generation": 0,
  "comments": [{
    "file": "a.txt",
    "new_line": 1,
    "summary": "Check the new state transition",
    "author": "review-agent"
  }]
}' | mark session comment apply --repo . --stdin --json
..."ids":["agent-1"]...
```

The comment appears inline in the already-open TUI with an `Agent ·
review-agent` title. `mark session navigate --repo . --next-comment --json`
moves focus to it without adding state.

When `a.txt` is edited again, `session context` reports
`"source_changed":true`, while `session patch` still returns the original
snapshot. The explicit command below advances the generation and exposes the
new patch without restarting Mark:

```sh
mark session reload --repo . --json -- diff -- a.txt
```

Closing the TUI removes its private socket and registry record. No daemon or
network service remains.
