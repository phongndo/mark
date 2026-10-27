import { Plugin } from "@opencode/plugin/tui";
import { setupMark } from "./adapter.mjs";

export default Plugin.define({ id: "mark.review", setup: setupMark });
