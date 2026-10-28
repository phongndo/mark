# Terminal handoff

Run `open-review.mjs` beside this file with Node from the dirty worktree being reviewed, using its absolute path. It requires an interactive terminal with inherited input/output; request a terminal tool with `tty: true` when supported. Keep the process running until the human submits annotations or goes back. A tool timeout can leave the command running; resume that process rather than starting another.

If the host cannot release or expose an interactive terminal, have the human run Mark in a separate terminal and paste the selected Markdown into chat. Do not launch HTML or a browser. The helper returns only selected annotation Markdown; an empty result means Back.

The helper returns annotations as data. End the handoff and wait for the human’s next chat message before answering notes or changing code.
