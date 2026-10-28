import { acknowledgeFeedback, createBridge, deliverFeedback } from "./common/bridge.mjs";

export function registerMark(pi, { bridge = createBridge(), terminal = process } = {}) {
  let activeContext;
  let running = false;
  const pending = new Map();
  pi.on("session_start", (_event, ctx) => { activeContext = ctx; });
  // sendUserMessage is void: a provider/input error can prevent it from ever
  // entering the transcript. Receipt is the matching user-message event.
  pi.on("message_start", (event, ctx) => {
    if (event.message.role !== "user") return;
    const content = event.message.content;
    const text = typeof content === "string" ? content : content.filter(part => part.type === "text").map(part => part.text).join("");
    const delivery = pending.get(text);
    if (!delivery || delivery.consumer !== `pi:${ctx.sessionManager.getSessionId()}`) return;
    try { acknowledgeFeedback(bridge, delivery); pending.delete(text); }
    catch (error) { ctx.ui.notify(`Mark saved feedback but could not record delivery: ${error.message}`, "warning"); }
  });
  // Pi's own MCP connection owns discovery and transport lifetime.
  if (typeof pi.registerMcpServer === "function") {
    pi.registerMcpServer("mark", { command: process.env.MARK_BIN || "mark", args: ["mcp"] });
  }
  pi.registerCommand("mark", {
    description: "Review changes in Mark, then send submitted feedback to this conversation",
    handler: async (_args, ctx) => {
      if (running) return ctx.ui.notify("A Mark review is already open.", "warning");
      if (ctx.mode !== "tui" || !terminal.stdin.isTTY || !terminal.stdout.isTTY) {
        return ctx.ui.notify("/mark needs Pi's interactive terminal. In RPC/print mode use Mark MCP and a browser review from `mark bridge web <review_id> --json`.", "error");
      }
      activeContext = ctx;
      const conversation = ctx.sessionManager.getSessionId();
      if (!conversation) return ctx.ui.notify("Pi has no current session ID. Start a session before /mark.", "error");
      const consumer = `pi:${conversation}`;
      const isCurrent = () => activeContext?.sessionManager.getSessionId() === conversation;
      running = true;
      try {
        const review = bridge.open({ harness: "pi", conversation, repository: ctx.cwd });
        const send = async (text, delivery) => {
          pending.set(text, delivery);
          pi.sendUserMessage(text, { deliverAs: "followUp" });
        };
        if (await deliverFeedback(bridge, review.review_id, consumer, send, isCurrent, { acknowledge: false })) return;
        const result = await ctx.ui.custom((tui, _theme, _keys, done) => {
          let outcome;
          try {
            tui.stop();
            outcome = { result: bridge.review(review.review_id) };
          } catch (error) { outcome = { error }; }
          finally {
            tui.start();
            tui.requestRender(true);
          }
          done(outcome);
          return { render: () => [], invalidate() {} };
        });
        if (result.error) throw result.error;
        if (result.result.outcome === "submitted") {
          await deliverFeedback(bridge, review.review_id, consumer, send, isCurrent, { acknowledge: false });
        }
      } catch (error) { ctx.ui.notify(`Mark: ${error.message}`, "error"); }
      finally { running = false; }
    },
  });
}

export default function mark(pi) { registerMark(pi); }
