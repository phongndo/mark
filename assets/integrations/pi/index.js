import { createReview } from "../common/review.mjs";

export function registerMark(pi, { review = createReview() } = {}) {
  let activeContext, running = false;
  pi.on("session_start", (_event, ctx) => { activeContext = ctx; });
  pi.registerCommand("mark", {
    description: "Annotate the diff in the terminal and prepare selected notes in the chat input",
    handler: async (_args, ctx) => {
      if (running) return ctx.ui.notify("A Mark review is already open.", "info");
      if (ctx.mode !== "tui") return ctx.ui.notify("/mark needs Pi’s interactive terminal.", "error");
      activeContext = ctx;
      const conversation = ctx.sessionManager.getSessionId();
      running = true;
      try {
        const outcome = await ctx.ui.custom((tui, _theme, _bindings, done) => {
          let result;
          try { tui.stop(); result = { markdown: review({ repository: ctx.cwd }) }; }
          catch (error) { result = { error }; }
          finally { tui.start(); tui.requestRender(true); }
          done(result);
          return { render: () => [], invalidate() {} };
        });
        if (outcome.error) throw outcome.error;
        const markdown = outcome.markdown;
        if (activeContext?.sessionManager.getSessionId() !== conversation) throw Error("The conversation changed. Run /mark in the conversation you want to annotate.");
        if (markdown) {
          const draft = ctx.ui.getEditorText();
          ctx.ui.setEditorText(draft.trim() ? `${draft}\n\n${markdown}` : markdown);
        }
      } catch (error) { ctx.ui.notify(`Mark: ${error.message}`, "error"); }
      finally { running = false; }
    },
  });
}

export default function mark(pi) { registerMark(pi); }
