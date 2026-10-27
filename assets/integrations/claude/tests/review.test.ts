import { expect, test } from "claude-code/testing";

test("/mark opens a review for this conversation through the real mod host", async ($, on) => {
  const commands = [];
  let opened = false;
  let prompt = "";
  let pending = false;
  const record = { review_id: "r1", generation: 1, threads: [], submissions: [] };
  on("session.id", async () => ({ value: "claude-test-conversation" }));
  on("session.cwd", async () => ({ value: "/test/project" }));
  on("env.get", async () => ({ value: "mark" }));
  on("ui.invalidate", async () => ({ value: undefined }));
  on("ui.close", async () => ({ value: undefined }));
  on("prompt.submit", async (_$, e) => { prompt = e.text; return { text: e.text }; });
  on("process.run", async (_$, e) => {
    commands.push(e.argv);
    const method = e.argv[3];
    const input = JSON.parse(e.init?.stdin || "{}");
    let result;
    if (e.argv[2] === "open" || method === "review.get") result = record;
    else if (method === "review.patch") result = { review_id: "r1", generation: 1, patch: "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1 @@\n+fixed\n", next_offset: null };
    else if (method === "comment.add") {
      expect(input.origin).toBe("human");
      expect(input.anchor).toEqual({ file: "a", new_line: 1 });
      expect(typeof input.client_mutation_id).toBe("string");
      const thread = { thread_id: "t1", anchor: input.anchor, messages: [{ message_id: "m1", origin: "human", body: input.body }], stale: false };
      record.threads.push(thread);
      result = { thread };
    } else if (method === "feedback.submit") {
      expect(input.thread_ids).toEqual(["t1"]);
      pending = true;
      result = { submission: { submission_id: "s1", threads: record.threads } };
    } else if (method === "feedback.get") result = { submissions: pending ? [{ submission_id: "s1", threads: record.threads }] : [] };
    else if (method === "feedback.ack") { pending = false; result = {}; }
    else if (method === "review.finish") result = {};
    else throw new Error(`Unexpected bridge method ${method}`);
    return { value: { exitCode: 0, stdout: JSON.stringify({ ok: true, result }), stderr: "" } };
  });
  on("ui.open", async () => { opened = true; return { value: { isPlaced: true } }; });
  const response = await $.command.run({ command: "mark", args: "" });
  expect(response.text).toBeUndefined();
  expect(opened).toBe(true);
  expect(commands.some(args => args.includes("--conversation"))).toBe(true);
  expect(commands.some(args => args.includes("review.patch"))).toBe(true);
  const ui = await $.ui.mount({ plugin: "mark", surface: "terminal", component: "Pane", requestId: "mark-review", props: { title: "Mark", isFocused: true, bodyColumns: 80, placement: "inline" } });
  expect((await ui.find({ key: "submit" }))?.text).toContain("Submit");
  expect((await ui.find({ type: "Code" }))?.text).toContain("+fixed");
  await ui.input({ key: "file", text: "a" });
  await ui.input({ key: "line", text: "new:1" });
  await ui.input({ key: "body", text: "Fix this race." });
  await ui.press({ key: "add" });
  await ui.redraw();
  expect(record.threads.length, (await ui.findAll({ type: "Text" })).map(item => item.text).join("\n")).toBe(1);
  expect((await ui.find({ key: "submit" }))?.text).toContain("1 selected");
  await ui.press({ key: "submit" });
  expect(prompt).toContain("Fix this race.");
  expect(pending).toBe(false);
  await ui.unmount();
  for (const surface of ["desktop", "vscode"]) {
    const rendered = await $.ui.mount({ plugin: "mark", surface, component: "Pane", requestId: "mark-review", props: { title: "Mark", isFocused: true, bodyColumns: 80, placement: "inline" } });
    expect((await rendered.find({ key: "add" }))?.text).toContain("Save comment");
    await rendered.unmount();
  }
});

test("a pane from another conversation cannot mutate or deliver feedback", async ($, on) => {
  let session = "original";
  const methods = [];
  let notice = "";
  on("session.id", async () => ({ value: session }));
  on("session.cwd", async () => ({ value: "/test/project" }));
  on("env.get", async () => ({ value: "mark" }));
  on("ui.invalidate", async () => ({ value: undefined }));
  on("ui.open", async () => ({ value: { isPlaced: true } }));
  on("ui.toast", async (_$, e) => { notice = e.text; return { value: undefined }; });
  on("process.run", async (_$, e) => {
    methods.push(e.argv[3]);
    const result = e.argv[3] === "review.patch" ? { patch: "", next_offset: null } : { review_id: "r1", generation: 1, threads: [], submissions: [] };
    return { value: { exitCode: 0, stdout: JSON.stringify({ ok: true, result }), stderr: "" } };
  });
  await $.command.run({ command: "mark", args: "" });
  const ui = await $.ui.mount({ plugin: "mark", surface: "terminal", component: "Pane", requestId: "mark-review", props: { title: "Mark", isFocused: true, bodyColumns: 80, placement: "inline" } });
  session = "different";
  await ui.press({ key: "retry" });
  expect(notice).toContain("another conversation");
  expect(methods.includes("feedback.get")).toBe(false);
  expect(methods.includes("feedback.ack")).toBe(false);
  await ui.unmount();
});
