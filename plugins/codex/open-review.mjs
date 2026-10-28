import { createBridge } from "./common/bridge.mjs";

const conversation = process.env.CODEX_THREAD_ID;
if (!conversation) {
  throw new Error("Codex did not expose CODEX_THREAD_ID. Use the current conversation ID from the host with `mark bridge open --harness codex --conversation ID --repo PATH --json`; keep the returned review ID pinned.");
}
const bridge = createBridge();
const review = bridge.open({ harness: "codex", conversation, repository: process.cwd() });
const browser = bridge.web(review.review_id);
process.stdout.write(JSON.stringify({ ok: true, result: { review_id: review.review_id, ...browser } }) + "\n");
