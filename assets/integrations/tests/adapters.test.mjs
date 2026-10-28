import test from "node:test";
import assert from "node:assert/strict";
import { writeFileSync, existsSync } from "node:fs";
import { copyAnnotations, createReview } from "../common/review.mjs";
import { registerMark } from "../pi/index.js";
import { setupMark } from "../opencode/adapter.mjs";

const markdown = "- `CMakeLists.txt:17` — What does this mean?";
function piHost(review) {
  const log = [], events = new Map();
  let session = "conversation-1", handler, draft = "";
  const ctx = { mode: "tui", cwd: "/dirty/project", sessionManager: { getSessionId: () => session },
    ui: { getEditorText: () => draft, setEditorText: text => { draft = text; log.push(["draft", text]); }, notify: (...args) => log.push(["notify", ...args]),
      custom: factory => new Promise((resolve, reject) => {
        try { factory({ stop: () => log.push(["stop"]), start: () => log.push(["start"]), requestRender: () => log.push(["render"]) }, {}, {}, resolve); }
        catch (error) { reject(error); }
      }) } };
  const pi = { on: (name, fn) => events.set(name, fn),
    registerCommand: (name, options) => { assert.equal(name, "mark"); handler = options.handler; },
    sendUserMessage: (...args) => log.push(["send", ...args]),
  };
  registerMark(pi, { review });
  return { log, ctx, run: () => handler("", ctx),
    switchSession: () => { session = "conversation-2"; events.get("session_start")({}, ctx); } };
}

test("Pi restores the terminal and fills the editor without starting a turn", async () => {
  const host = piHost(({ repository }) => { assert.equal(repository, "/dirty/project"); assert.deepEqual(host.log, [["stop"]]); return markdown; });
  await host.run();
  assert.deepEqual(host.log, [["stop"], ["start"], ["render"], ["draft", markdown]]);
});

test("Pi appends annotations to an existing draft", async () => {
  const host = piHost(() => markdown);
  host.ctx.ui.setEditorText("Please explain these annotations.");
  await host.run();
  assert.equal(host.ctx.ui.getEditorText(), "Please explain these annotations.\n\n" + markdown);
  assert.equal(host.log.some(item => item[0] === "send"), false);
});

test("Pi Back starts fresh and failures always restore its renderer", async () => {
  let opens = 0;
  const host = piHost(() => { if (++opens === 2) throw Error("terminal failed"); return ""; });
  await host.run(); await host.run(); await host.run();
  assert.equal(opens, 3);
  assert.equal(host.log.filter(item => item[0] === "start").length, 3);
  assert.equal(host.ctx.ui.getEditorText(), "");
  assert.equal(host.log.some(item => item[0] === "send"), false);
  assert.match(host.log.find(item => item[0] === "notify")[1], /terminal failed/);
});

test("a changed Pi conversation receives no feedback", async () => {
  const host = piHost(() => { host.switchSession(); return markdown; });
  await host.run();
  assert.equal(host.ctx.ui.getEditorText(), "");
  assert.equal(host.log.some(item => item[0] === "send"), false);
});

function openCodeHost(review) {
  let command, session = "oc-1";
  const log = [];
  const context = { location: { directory: "/dirty/project" },
    renderer: { suspend: async () => { log.push(["suspend"]); }, resume: async () => { log.push(["resume"]); } },
    keymap: { layer: fn => { command = fn().commands[0]; }, dispatch: id => log.push(["dispatch", id]) },
    ui: { router: { current: () => ({ type: "session", sessionID: session }) }, toast: { show: item => log.push(["toast", item]) } },
    client: { session: { prompt: async input => { log.push(["prompt", input]); } } },
  };
  setupMark(context, { review, copy: text => log.push(["copy", text]) });
  return { log, run: () => command.run(), switchSession: () => { session = "oc-2"; } };
}

test("OpenCode restores its terminal and pastes Markdown without submitting", async () => {
  const host = openCodeHost(({ repository }) => { assert.equal(repository, "/dirty/project"); return markdown; });
  await host.run();
  assert.deepEqual(host.log, [["suspend"], ["resume"], ["copy", markdown], ["dispatch", "prompt.paste"]]);
});

test("OpenCode Back, errors, and changed conversations restore without sending", async () => {
  for (const result of ["", "error", "switch"]) {
    const host = openCodeHost(() => {
      if (result === "error") throw Error("terminal failed");
      if (result === "switch") host.switchSession();
      return result === "switch" ? markdown : "";
    });
    await host.run();
    assert.deepEqual(host.log.slice(0, 2), [["suspend"], ["resume"]]);
    assert.equal(host.log.some(item => item[0] === "prompt"), false);
  }
});

test("clipboard transport passes annotations as input without a shell", () => {
  for (const [platform, env, binary, args] of [
    ["darwin", {}, "pbcopy", []],
    ["linux", { WAYLAND_DISPLAY: "wayland-0" }, "wl-copy", []],
    ["linux", {}, "xclip", ["-selection", "clipboard"]],
  ]) {
    copyAnnotations(markdown, { platform, env, spawn: (...input) => {
      assert.deepEqual(input, [binary, args, { input: markdown, encoding: "utf8" }]);
      return { status: 0 };
    } });
  }
  assert.throws(() => copyAnnotations(markdown, { spawn: () => ({ status: 1, stderr: "clipboard unavailable" }) }), /clipboard unavailable/);
});

test("shared handoff inherits the terminal, reads its result, and cleans up", () => {
  let result;
  const review = createReview({ binary: "/path with spaces/mark", spawn: (binary, args, options) => {
    assert.equal(binary, "/path with spaces/mark");
    assert.deepEqual(args.slice(0, 4), ["bridge", "annotate", "--repo", "/dirty repo/$(not shell)"]);
    assert.deepEqual(options, { stdio: "inherit" });
    result = args.at(-1); writeFileSync(result, JSON.stringify({ markdown }));
    return { status: 0 };
  } });
  assert.equal(review({ repository: "/dirty repo/$(not shell)" }), markdown);
  assert.equal(existsSync(result), false);
});

test("shared handoff rejects missing, malformed, and failed child results", () => {
  for (const response of [undefined, "not json", "{}", "error"]) {
    let result;
    const review = createReview({ spawn: (_binary, args) => {
      result = args.at(-1);
      if (response !== undefined) writeFileSync(result, response);
      return { status: response === "error" ? 1 : 0 };
    } });
    assert.throws(() => review()); assert.equal(existsSync(result), false);
  }
});
