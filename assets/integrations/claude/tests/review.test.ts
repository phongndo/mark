import { expect, mock, test } from "claude-code/testing";

async function setup($, on, { env = { REX_BLOCK: "block:claude" }, draft = "" } = {}) {
  const clock = mock.clock(on);
  const commands = [], filled = [], notices = [], copied = [], files = new Map();
  let session = "original", alive = true, fillAvailable = true;
  on("session.id", async () => ({ value: session }));
  on("session.cwd", async () => ({ value: "/dirty/project" }));
  on("env.get", async (_$, event) => ({ value: { MARK_BIN: "/local/mark", ...env }[event.name] }));
  on("ui.toast", async (_$, event) => { notices.push(event.text); return { value: undefined }; });
  on("ui.copy", async (_$, event) => { copied.push(event.text); return { value: { isCopied: true } }; });
  on("prompt.read", async () => ({ value: { text: draft, cursor: draft.length } }));
  on("prompt.fill", async (_$, event) => {
    if (!fillAvailable) return { isFilled: false };
    filled.push(event); draft += event.text;
    return { isFilled: true };
  });
  on("fs.exists", async (_$, event) => ({ value: files.has(event.path) }));
  on("fs.read", async (_$, event) => ({ value: files.get(event.path) }));
  on("process.run", async (_$, event) => {
    commands.push(event.argv);
    const [program, command] = event.argv;
    const stdout = program === "mktemp" ? "/tmp/mark-1\n"
      : program === "rex" && command === "split" ? JSON.stringify({ block_ids: ["block:mark"] })
      : program === "tmux" && command === "split-window" ? "%7\n" : "";
    const exitCode = (command === "block" || command === "display-message") && !alive ? 1 : 0;
    return { value: { exitCode, stdout, stderr: "" } };
  });
  const result = await $.command.run({ command: "mark", args: "" });
  return {
    result, clock, commands, filled, notices, copied,
    finish: markdown => files.set("/tmp/mark-1/result.json", JSON.stringify({ markdown })),
    close: () => { alive = false; },
    switchSession: () => { session = "other"; },
    setFillAvailable: available => { fillAvailable = available; },
  };
}

test("opens the real Mark TUI in a Rex split beside Claude", async ($, on) => {
  const host = await setup($, on);
  const split = host.commands.find(argv => argv[1] === "split");
  expect(split.slice(0, 11)).toEqual(["rex", "split", "--json", "-b", "block:claude", "-c", "/dirty/project", "--label", "mark", "--shell", "none"]);
  expect(split.slice(-7)).toEqual(["/local/mark", "bridge", "annotate", "--repo", "/dirty/project", "--result-file", "/tmp/mark-1/result.json"]);
  expect(host.result.text).toContain("Mark opened");
});

test("returned marks fill the chat draft without sending", async ($, on) => {
  const host = await setup($, on);
  host.finish("- `a:1` — What does this mean?");
  await host.clock.advance(600);
  expect(host.filled.map(event => ({ text: event.text, mode: event.mode }))).toEqual([{ text: "- `a:1` — What does this mean?", mode: "append" }]);
  expect(host.commands.at(-1)).toEqual(["rm", "-rf", "/tmp/mark-1"]);
});

test("marks append after an existing draft", async ($, on) => {
  const host = await setup($, on, { draft: "Explain these notes." });
  host.finish("- `a:1` — note");
  await host.clock.advance(600);
  expect(host.filled[0].text).toBe("\n\n- `a:1` — note");
});

test("tmux is used when Rex is absent", async ($, on) => {
  const host = await setup($, on, { env: { TMUX: "/tmp/tmux", TMUX_PANE: "%1" } });
  expect(host.commands.find(argv => argv[0] === "tmux").slice(0, 10)).toEqual(["tmux", "split-window", "-h", "-t", "%1", "-c", "/dirty/project", "-P", "-F", "#{pane_id}"]);
  host.close();
  await host.clock.advance(600);
  expect(host.filled.length).toBe(0);
  expect(host.commands.at(-1)).toEqual(["rm", "-rf", "/tmp/mark-1"]);
});

test("closing Mark without marks fills nothing and allows reopening", async ($, on) => {
  const closed = await setup($, on);
  closed.close();
  await closed.clock.advance(600);
  expect(closed.filled.length).toBe(0);
  expect((await $.command.run({ command: "mark", args: "" })).text).toContain("Mark opened");
});

test("a changed conversation is refused", async ($, on) => {
  const host = await setup($, on);
  host.switchSession();
  host.finish("- `a:1` — note");
  await host.clock.advance(600);
  expect(host.filled.length).toBe(0);
  expect(host.notices.at(-1)).toContain("conversation changed");
});

test("an unavailable chat input copies marks to the clipboard", async ($, on) => {
  const host = await setup($, on);
  host.setFillAvailable(false);
  host.finish("- `a:1` — note");
  await host.clock.advance(600);
  expect(host.copied).toEqual(["- `a:1` — note"]);
  expect(host.notices.at(-1)).toContain("clipboard");
});

test("without Rex or tmux, /mark explains how to run it", async ($, on) => {
  const host = await setup($, on, { env: {} });
  expect(host.result.text).toContain("Rex or tmux");
  expect(host.commands.at(-1)).toEqual(["rm", "-rf", "/tmp/mark-1"]);
});

test("a second /mark while one is open does not open another split", async ($, on) => {
  const host = await setup($, on);
  expect((await $.command.run({ command: "mark", args: "" })).text).toContain("already open");
  expect(host.commands.filter(argv => argv[1] === "split").length).toBe(1);
});
