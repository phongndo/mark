import { copyAnnotations, createReview } from "../common/review.mjs";

export function setupMark(context, { review = createReview(), copy = copyAnnotations } = {}) {
  let running = false;
  context.keymap.layer(() => ({
    mode: "global",
    commands: [{
      id: "mark.review", title: "Review with Mark", slash: { name: "mark" }, palette: true,
      run: async () => {
        if (running) return;
        const route = context.ui.router.current();
        if (route.type !== "session") return context.ui.toast.show({ message: "Open a conversation before /mark.", variant: "error" });
        if (!context.renderer?.suspend || !context.renderer?.resume) return context.ui.toast.show({ message: "Mark needs OpenCode’s interactive terminal with renderer suspension.", variant: "error" });
        const location = context.location ?? context.data.location.default();
        if (!location?.directory) return context.ui.toast.show({ message: "OpenCode did not expose a workspace directory.", variant: "error" });
        const conversation = route.sessionID;
        running = true;
        try {
          let markdown;
          await context.renderer.suspend();
          try { markdown = review({ repository: location.directory }); }
          finally { await context.renderer.resume(); }
          const current = context.ui.router.current();
          if (current.type !== "session" || current.sessionID !== conversation) throw Error("The conversation changed. Run /mark in the conversation you want to review.");
          if (markdown) {
            copy(markdown);
            context.keymap.dispatch("prompt.paste");
          }
        } catch (error) { context.ui.toast.show({ message: `Mark: ${error.message}`, variant: "error" }); }
        finally { running = false; }
      },
    }],
  }));
}
