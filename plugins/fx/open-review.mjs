import { pathToFileURL } from "node:url";
import { createBridge } from "./common/bridge.mjs";

export function openReview({ conversation, repository = process.cwd() }, { bridge = createBridge() } = {}) {
  if (typeof conversation !== "string" || !conversation.trim() || /\s|\0/.test(conversation) || conversation.length > 1024) {
    throw new Error("fx has not supplied the current conversation ID. Copy it from /status and pass --conversation ID; a latest or remembered session is not an exact binding.");
  }
  const review = bridge.open({ harness: "fx", conversation, repository });
  if (!review.review_id || review.binding?.harness !== "fx" || review.binding?.conversation !== conversation) {
    throw new Error("Mark returned a different conversation binding. No review viewer was opened.");
  }
  const browser = bridge.web(review.review_id);
  if (browser.review_id && browser.review_id !== review.review_id) {
    throw new Error("Mark returned a viewer for a different review.");
  }
  return { ...browser, review_id: review.review_id, conversation };
}

export function parseArguments(args) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const flag = args[index];
    if (!["--conversation", "--repo"].includes(flag) || index + 1 >= args.length || args[index + 1].startsWith("--")) {
      throw new Error("Usage: node open-review.mjs --conversation FX_SESSION_ID [--repo PATH]");
    }
    const key = flag === "--conversation" ? "conversation" : "repository";
    if (options[key] !== undefined) throw new Error(`${flag} may be supplied only once.`);
    options[key] = args[++index];
  }
  return options;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const result = openReview(parseArguments(process.argv.slice(2)));
    process.stdout.write(JSON.stringify({ ok: true, result }) + "\n");
  } catch (error) {
    process.stdout.write(JSON.stringify({ ok: false, error: { code: "fx_handoff_failed", message: error.message } }) + "\n");
    process.exitCode = 1;
  }
}
