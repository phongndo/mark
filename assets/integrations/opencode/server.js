import { Plugin } from "@opencode/plugin";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export default Plugin.define({
  id: "mark.review",
  async setup(ctx) {
    const path = fileURLToPath(new URL("./skills/mark-review-loop/SKILL.md", import.meta.url));
    const text = readFileSync(path, "utf8");
    const skill = await ctx.skill.transform(editor => {
      editor.add({
        id: "mark-review-loop", name: "mark-review-loop", path,
        description: "Continue a Mark review, apply submitted feedback and answer review threads.",
        content: text.replace(/^---\n[\s\S]*?\n---\n/, ""),
      });
    });
    let mcp;
    try {
      mcp = await ctx.mcp.transform(editor => {
        // Honor a server explicitly configured by the user.
        if (!editor.get("mark")) editor.set("mark", { type: "local", command: [process.env.MARK_BIN || "mark", "mcp"] });
      });
    } catch (error) { await skill.dispose(); throw error; }
    return async () => { await mcp.dispose(); await skill.dispose(); };
  },
});
