import { createBridge, deliverFeedback } from "../common/bridge.mjs";

export function setupMark(context, { bridge = createBridge(), terminal = process } = {}) {
  let running = false;
  context.keymap.layer(() => ({
    mode: "global",
    commands: [{
      id: "mark.review", title: "Review with Mark", slash: { name: "mark" }, palette: true,
      run: async () => {
        if (running) return;
        const route = context.ui.router.current();
        if (route.type !== "session") return context.ui.toast.show({ message: "Open a conversation before /mark.", variant: "error" });
        if (!terminal.stdin.isTTY || !terminal.stdout.isTTY || !context.renderer?.suspend || !context.renderer?.resume) {
          return context.ui.toast.show({ message: "Mark needs the OpenCode V2 interactive terminal with renderer suspension. Use the Codex/browser package for graphical hosts.", variant: "error" });
        }
        const conversation = route.sessionID;
        const location = context.location ?? context.data.location.default();
        const repository = location?.directory;
        if (!repository) return context.ui.toast.show({ message: "OpenCode did not expose a workspace directory. Update the Mark integration for this OpenCode version.", variant: "error" });
        const isCurrent = () => context.ui.router.current().type === "session" && context.ui.router.current().sessionID === conversation;
        const consumer = `opencode:${conversation}`;
        running = true;
        try {
          const review = bridge.open({ harness: "opencode", conversation, repository });
          const send = async (text) => {
            await context.client.session.prompt({ sessionID: conversation, text, delivery: "queue" });
          };
          if (await deliverFeedback(bridge, review.review_id, consumer, send, isCurrent)) return;
          let result;
          await context.renderer.suspend();
          try { result = bridge.review(review.review_id); }
          finally { await context.renderer.resume(); }
          if (result.outcome === "submitted") await deliverFeedback(bridge, review.review_id, consumer, send, isCurrent);
        } catch (error) { context.ui.toast.show({ message: `Mark: ${error.message}`, variant: "error" }); }
        finally { running = false; }
      },
    }],
  }));
}
