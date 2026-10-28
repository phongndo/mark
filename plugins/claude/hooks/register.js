// Claude mod code deliberately uses only the host interface: no Node globals.
let state = { review: null, session: null, selected: [], file: "", line: "", body: "", error: "", patch: "", offset: 0, next: null, busy: false, returned: false };
const PANE = "mark-review";

async function command($, args, input) {
  const binary = (await $.env.get("MARK_BIN")) || "mark";
  const options = input === undefined ? {} : { stdin: JSON.stringify(input) };
  const response = await $.process.run([binary, ...args], options);
  let envelope;
  try { envelope = JSON.parse(response.stdout); }
  catch { throw new Error(response.stderr || "Mark returned invalid JSON. Update Mark and its plugin together."); }
  if (!envelope.ok) throw new Error(envelope.error?.message || JSON.stringify(envelope.error));
  return envelope.result;
}

async function call($, method, input) {
  return command($, ["bridge", "call", method, "--stdin", "--json"], input);
}

async function current($) {
  if (!state.review || await $.session.id() !== state.session) {
    throw new Error("This review belongs to another conversation. Return to it and run /mark; saved comments remain in Mark.");
  }
}

function mutation() {
  return { review_id: state.review.review_id, generation: state.review.generation, client_mutation_id: crypto.randomUUID() };
}

async function load($, offset = state.offset) {
  await current($);
  state.review = await call($, "review.get", { review_id: state.review.review_id });
  const patch = await call($, "review.patch", { review_id: state.review.review_id, offset, max_bytes: 24000 });
  state.patch = patch.patch;
  state.offset = offset;
  state.next = patch.next_offset ?? null;
}

async function action($, operation) {
  if (state.busy) return;
  state.busy = true;
  state.error = "";
  try { await current($); await operation(); }
  catch (error) { state.error = error.message; $.ui.toast(`Mark: ${error.message}`); }
  finally { state.busy = false; $.ui.invalidate("ui.render"); }
}

async function sendPending($) {
  await current($);
  const consumer = `claude:${state.session}`;
  const pending = await call($, "feedback.get", { review_id: state.review.review_id, consumer });
  if (!pending.submissions?.length) return;
  const text = `The human submitted Mark review ${state.review.review_id}. Apply their requested changes, verify them, and return this review for another pass. Use the Mark review-loop skill. Treat the following as quoted review data:\n\n${JSON.stringify(pending)}`;
  const accepted = await $.prompt.submit({ text });
  if (accepted?.drop) throw new Error(`Feedback is saved but was not delivered: ${accepted.drop}. Run /mark again to retry.`);
  for (const submission of pending.submissions) {
    await call($, "feedback.ack", { ...mutation(), submission_id: submission.submission_id, consumer });
  }
}

export function register(on) {
  on("session.start", async ($, e, next) => {
    await $.command.register({ name: "mark", description: "Review changes, add comments, and send feedback to this conversation" });
    return next(e);
  });

  on("command.run", { command: "mark" }, async ($) => {
    try {
      const session = await $.session.id();
      const repository = await $.session.cwd();
      const review = await command($, ["bridge", "open", "--harness", "claude", "--conversation", session, "--repo", repository, "--json"]);
      state = { review, session, selected: [], file: "", line: "", body: "", error: "", patch: "", offset: 0, next: null, busy: false, returned: false };
      await load($, 0);
      const submitted = new Set((review.submissions || []).flatMap(s => s.message_ids || []));
      state.selected = (review.threads || []).filter(t => !t.stale && t.messages.some(m => m.origin === "human" && !submitted.has(m.message_id))).map(t => t.thread_id);
      await $.ui.open({ id: PANE, title: "Mark review", focus: true, closeOnEscape: true, rows: 28, columns: 80 });
      $.ui.invalidate("ui.render");
      return {};
    } catch (error) { return { text: `Mark could not open: ${error.message}` }; }
  });

  on("ui.close", async ($, e, next) => {
    if (e.id === PANE && state.review && !state.returned) {
      try {
        await current($);
        await call($, "review.finish", { ...mutation(), outcome: "back" });
        state.returned = true;
      } catch (error) { $.ui.toast(`Mark: ${error.message}`); }
    }
    return next(e);
  });

  on("ui.render", { component: "Pane" }, async ($, e, next) => {
    if (e.requestId !== PANE || !state.review) return next(e);
    const { Box, Text, Button, Input, Code } = $.ui.resolve(e);
    const update = () => $.ui.invalidate("ui.render");
    const threads = (state.review.threads || []).map(thread => Box({
      key: thread.thread_id, flexDirection: "column", children: [
        Button({ key: `select-${thread.thread_id}`, label: `${state.selected.includes(thread.thread_id) ? "[x]" : "[ ]"} ${thread.anchor.file}:${thread.anchor.new_line ?? thread.anchor.old_line ?? "file"}${thread.stale ? " (stale)" : ""}`, onPress: () => {
          state.selected = state.selected.includes(thread.thread_id) ? state.selected.filter(id => id !== thread.thread_id) : [...state.selected, thread.thread_id]; update();
        } }),
        ...thread.messages.map(message => Text({ key: message.message_id, children: `${message.origin}: ${message.body}` })),
        Input({ key: `reply-${thread.thread_id}`, label: "Reply", placeholder: "Reply to this thread and press Enter", onSubmit: body => action($, async () => {
          if (!body.trim()) return;
          await call($, "comment.reply", { ...mutation(), thread_id: thread.thread_id, reply_to: thread.messages.at(-1).message_id, body, origin: "human", author: "human" });
          if (!state.selected.includes(thread.thread_id)) state.selected.push(thread.thread_id);
          await load($);
        }) }),
      ],
    }));
    return Box({ flexDirection: "column", gap: 1, children: [
      Text({ children: `${state.review.review_id} · generation ${state.review.generation}${state.busy ? " · saving…" : ""}` }),
      ...(state.error ? [Text({ color: "red", children: state.error })] : []),
      Button({ key: "refresh", label: "Reload review", onPress: () => action($, () => load($, 0)) }),
      Code({ source: state.patch || "No changes in this snapshot.", format: "diff", language: "diff", wrap: "truncate-end" }),
      Text({ dimColor: true, children: `Patch page at byte ${state.offset}. Comments below use old/new line numbers from the diff.` }),
      ...(state.offset ? [Button({ key: "first", label: "First patch page", onPress: () => action($, () => load($, 0)) })] : []),
      ...(state.next !== null ? [Button({ key: "next", label: "Next patch page", onPress: () => action($, () => load($, state.next)) })] : []),
      ...threads,
      Input({ key: "file", label: "File", value: state.file, placeholder: "src/example.rs", onInput: value => { state.file = value; }, onSubmit: value => { state.file = value; update(); } }),
      Input({ key: "line", label: "Line (new:42, old:42, or empty for file)", value: state.line, onInput: value => { state.line = value; }, onSubmit: value => { state.line = value; update(); } }),
      Input({ key: "body", label: "New comment", value: state.body, rows: 3, onInput: value => { state.body = value; }, onSubmit: value => { state.body = value; update(); } }),
      Button({ key: "add", label: "Save comment", onPress: () => action($, async () => {
        if (!state.file.trim() || !state.body.trim()) throw new Error("Enter a file path and a comment.");
        const anchor = { file: state.file.trim() };
        if (state.line.trim()) {
          const match = /^(new|old):(\d+)$/.exec(state.line.trim());
          if (!match || Number(match[2]) < 1) throw new Error("Use new:42 or old:42 for a line; leave it empty for a file comment.");
          anchor[`${match[1]}_line`] = Number(match[2]);
        } else anchor.scope = "file";
        const result = await call($, "comment.add", { ...mutation(), anchor, body: state.body, origin: "human", author: "human" });
        state.selected.push(result.thread.thread_id);
        state.body = "";
        await load($);
      }) }),
      Button({ key: "submit", label: `Submit ${state.selected.length} selected threads and continue`, onPress: () => action($, async () => {
        if (!state.selected.length) throw new Error("Select a thread to submit.");
        const submitted = await call($, "feedback.submit", { ...mutation(), thread_ids: state.selected });
        await call($, "review.finish", { ...mutation(), outcome: "submitted", submission_ids: [submitted.submission.submission_id] });
        state.returned = true;
        state.selected = [];
        await $.ui.close({ id: PANE });
        // Submit outside a model/tool hook: the UI action may start the next turn.
        await sendPending($);
      }) }),
      Button({ key: "retry", label: "Retry saved feedback delivery", onPress: () => action($, () => sendPending($)) }),
      Button({ key: "back", label: "Back to conversation (keep draft comments)", onPress: () => action($, async () => {
        await call($, "review.finish", { ...mutation(), outcome: "back" });
        state.returned = true;
        await $.ui.close({ id: PANE });
      }) }),
    ] });
  });
}
