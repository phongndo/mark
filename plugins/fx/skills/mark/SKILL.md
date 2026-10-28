---
name: mark
description: Annotate the current diff in the terminal and discuss selected notes in chat.
disable-model-invocation: true
---

# Mark

Use [the host entry instructions](HOST.md) to open Mark in an interactive terminal. Keep the command running while the human annotates. Never open a browser or substitute an HTML viewer. If this host cannot hand over a terminal, have the human open Mark in a separate terminal.

Saving an annotation does not send it. Shift-Q immediately returns all marks and closes Mark, saving any open draft. For a subset, the `n` marks list selects notes with Space (`j`/`k` to move) and returns them with Ctrl-S; with nothing selected, Ctrl-S returns the highlighted mark. Native adapters fill the chat input as a draft; the human edits it and presses Enter when ready. Back sends nothing. Each invocation starts from the current worktree, without a review ID or past review history.

Returning from Mark completes the annotation handoff. Wait for the human to send their chat message before interpreting the notes or taking action. A note can be a question, observation, or request. When asked, inspect its anchored code and respond according to its content. Answer and discuss in chat; change code when requested. Do not write replies in Mark, acknowledge submissions, or automatically reopen a review.
