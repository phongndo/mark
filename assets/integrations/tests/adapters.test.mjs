import test from "node:test";
import assert from "node:assert/strict";
import { createBridge, deliverFeedback } from "../common/bridge.mjs";
import { registerMark } from "../pi/index.js";
import { setupMark } from "../opencode/adapter.mjs";

function backend(outcome = "submitted") {
  const calls = [];
  let pending = false;
  return {
    calls,
    open(binding) { calls.push(["open", binding]); return { review_id: "r1" }; },
    review(id) { calls.push(["review", id]); pending = outcome === "submitted"; return { review_id: id, outcome }; },
    call(method, input) {
      calls.push([method, input]);
      if (method === "feedback.get") return { generation: 3, submissions: pending ? [{ submission_id: "s1", threads: [{ body: "Fix the race" }] }] : [] };
      if (method === "feedback.ack") pending = false;
      return {};
    },
  };
}

function piHost(bridge, { recordMessages = true } = {}) {
  const log = [];
  let session = "conversation-1";
  let handler;
  const events = new Map();
  const ctx = {
    mode: "tui", cwd: "/project", sessionManager: { getSessionId: () => session },
    ui: {
      notify: (...args) => log.push(["notify", ...args]),
      custom: async factory => {
        let result;
        factory({ stop: () => log.push(["stop"]), start: () => log.push(["start"]), requestRender: () => log.push(["render"]) }, {}, {}, value => { result = value; });
        return result;
      },
    },
  };
  const pi = {
    on(name, handler) { events.set(name, handler); }, registerMcpServer: (...args) => log.push(["mcp", ...args]),
    registerCommand: (name, options) => { assert.equal(name, "mark"); handler = options.handler; },
    sendUserMessage: (...args) => {
      log.push(["send", ...args]);
      if (recordMessages) queueMicrotask(() => events.get("message_start")({ message: { role: "user", content: [{ type: "text", text: args[0] }] } }, ctx));
    },
  };
  registerMark(pi, { bridge, terminal: { stdin: { isTTY: true }, stdout: { isTTY: true } } });
  return { log, ctx, run: () => handler("", ctx), switchSession: () => { session = "conversation-2"; } };
}

test("Pi hands off this terminal and returns submitted feedback to the bound conversation", async () => {
  const bridge = backend();
  const host = piHost(bridge);
  await host.run();
  assert.deepEqual(bridge.calls[0], ["open", { harness: "pi", conversation: "conversation-1", repository: "/project" }]);
  assert.deepEqual(host.log.filter(x => ["stop", "start", "render"].includes(x[0])).map(x => x[0]), ["stop", "start", "render"]);
  assert.equal(host.log.find(x => x[0] === "send")[2].deliverAs, "followUp");
  assert.equal(bridge.calls.at(-1)[0], "feedback.ack");
  assert.equal(bridge.calls.at(-1)[1].consumer, "pi:conversation-1");
});

test("Back keeps draft feedback out of the agent conversation", async () => {
  const bridge = backend("back");
  const host = piHost(bridge);
  await host.run();
  assert.equal(host.log.some(x => x[0] === "send"), false);
  assert.equal(bridge.calls.some(x => x[0] === "feedback.ack"), false);
});

test("Pi leaves feedback pending when a provider error prevents the user message being recorded", async () => {
  const bridge = backend();
  const host = piHost(bridge, { recordMessages: false });
  await host.run();
  assert.equal(host.log.some(x => x[0] === "send"), true);
  assert.equal(bridge.calls.some(x => x[0] === "feedback.ack"), false);
});

test("Pi restores its renderer when the Mark child fails", async () => {
  const bridge = backend();
  bridge.review = () => { throw new Error("child failed"); };
  const host = piHost(bridge);
  await host.run();
  assert.deepEqual(host.log.filter(x => ["stop", "start", "render"].includes(x[0])).map(x => x[0]), ["stop", "start", "render"]);
  assert.match(host.log.find(x => x[0] === "notify")[1], /child failed/);
});

test("a changed Pi session leaves submitted feedback unacknowledged", async () => {
  const bridge = backend();
  const original = bridge.review.bind(bridge);
  const host = piHost(bridge);
  bridge.review = id => { const result = original(id); host.switchSession(); return result; };
  await host.run();
  assert.equal(host.log.some(x => x[0] === "send"), false);
  assert.equal(bridge.calls.some(x => x[0] === "feedback.ack"), false);
  assert.match(host.log.find(x => x[0] === "notify")[1], /conversation changed/);
});

test("a rejected host message does not acknowledge submitted feedback", async () => {
  const bridge = backend();
  bridge.review("r1");
  await assert.rejects(deliverFeedback(bridge, "r1", "agent", () => { throw new Error("busy"); }, () => true), /busy/);
  assert.equal(bridge.calls.some(x => x[0] === "feedback.ack"), false);
});

test("OpenCode releases and restores the renderer before sending to the captured session", async () => {
  const bridge = backend();
  const events = [];
  let command;
  const context = {
    location: { directory: "/project" },
    keymap: { layer: fn => { command = fn().commands[0]; } },
    ui: { router: { current: () => ({ type: "session", sessionID: "oc-1" }) }, toast: { show: x => events.push(["toast", x]) } },
    renderer: { suspend: async () => events.push(["suspend"]), resume: async () => events.push(["resume"]) },
    client: { session: { prompt: async input => { events.push(["prompt", input]); return {}; } } },
  };
  setupMark(context, { bridge, terminal: { stdin: { isTTY: true }, stdout: { isTTY: true } } });
  await command.run();
  assert.equal(command.slash.name, "mark");
  assert.deepEqual(events.map(x => x[0]), ["suspend", "resume", "prompt"]);
  assert.equal(events[2][1].sessionID, "oc-1");
});

test("bridge uses an argv array and JSON stdin without executing shell input", () => {
  const calls = [];
  const bridge = createBridge({ binary: "/path with space/mark", spawn: (binary, args, options) => {
    calls.push({ binary, args, options }); return { status: 0, stdout: JSON.stringify({ ok: true, result: { review_id: "r1" } }) };
  } });
  bridge.open({ harness: "pi", conversation: "$(touch /tmp/unwanted)", repository: "/project with spaces" });
  bridge.call("comment.add", { body: "`echo unsafe`", review_id: "r1" });
  assert.equal(calls[0].binary, "/path with space/mark");
  assert.equal(calls[0].args[5], "$(touch /tmp/unwanted)");
  assert.equal(calls[0].options.shell, undefined);
  assert.equal(JSON.parse(calls[1].options.input).body, "`echo unsafe`");
});
