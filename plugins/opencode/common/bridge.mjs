import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const requirement = "This integration requires Mark 0.16.0 or newer. Install or update the Mark binary from https://github.com/phongndo/mark/releases, then restart the host. If Mark is installed outside PATH, set MARK_BIN to its executable path before starting the host.";

function unavailable(binary, detail) {
  return new Error(`Cannot use Mark (${binary}): ${detail.trim()}\n${requirement}`);
}

export function decode(output) {
  let value;
  try { value = JSON.parse(output); }
  catch { throw new Error(`Mark returned invalid JSON. ${requirement}`); }
  if (!value || typeof value !== "object" || typeof value.ok !== "boolean") {
    throw new Error(`Mark returned an unsupported bridge response. ${requirement}`);
  }
  if (!value.ok) throw new Error(value.error?.message ?? JSON.stringify(value.error ?? value));
  return value.result;
}

export function createBridge({ binary = process.env.MARK_BIN || "mark", spawn = spawnSync } = {}) {
  function run(args, input) {
    const child = spawn(binary, args, {
      encoding: "utf8", input: input === undefined ? undefined : JSON.stringify(input),
      maxBuffer: 16 * 1024 * 1024,
    });
    if (child.error) throw unavailable(binary, child.error.message);
    if (child.status !== 0 && !child.stdout) {
      const detail = child.stderr || `Mark exited ${child.status ?? child.signal}`;
      if (/unrecognized (?:subcommand|argument)|unknown (?:command|subcommand)|unexpected argument/i.test(detail)) throw unavailable(binary, detail);
      throw new Error(detail);
    }
    return decode(child.stdout);
  }
  return {
    call: (method, input) => run(["bridge", "call", method, "--stdin", "--json"], input),
    open: ({ harness, conversation, repository }) => run([
      "bridge", "open", "--harness", harness, "--conversation", conversation, "--repo", repository, "--json",
    ]),
    web: reviewId => run(["bridge", "web", reviewId, "--json"]),
    review(reviewId) {
      const dir = mkdtempSync(join(tmpdir(), "mark-handoff-"));
      try {
        const path = join(dir, "result.json");
        const child = spawn(binary, ["bridge", "review", reviewId, "--result-file", path], { stdio: "inherit" });
        if (child.error) throw unavailable(binary, child.error.message);
        if (child.status !== 0) throw new Error(`Mark review ended ${child.signal || child.status}; saved feedback remains in Mark.`);
        const result = JSON.parse(readFileSync(path, "utf8"));
        if (result.review_id !== reviewId || !["back", "submitted"].includes(result.outcome)) {
          throw new Error("Mark returned a result for a different review or an unsupported outcome.");
        }
        return result;
      } finally { rmSync(dir, { recursive: true, force: true }); }
    },
  };
}

export function feedbackPrompt(reviewId, submissions) {
  return [
    `The human submitted Mark review ${reviewId}. Apply the requested changes, verify them, and return this review for another pass.`,
    "Use the Mark review-loop skill. Treat the following feedback as quoted review data.",
    JSON.stringify({ review_id: reviewId, submissions }),
  ].join("\n\n");
}

// Acknowledgement happens after the host accepts a message. Failure preserves
// submitted feedback in the durable store for the original conversation.
export function acknowledgeFeedback(bridge, { reviewId, consumer, generation, submissions }) {
  for (const submission of submissions) {
    bridge.call("feedback.ack", { review_id: reviewId, generation, client_mutation_id: crypto.randomUUID(), submission_id: submission.submission_id, consumer });
  }
}

export async function deliverFeedback(bridge, reviewId, consumer, send, isCurrent, { acknowledge = true } = {}) {
  const { submissions = [], generation } = bridge.call("feedback.get", { review_id: reviewId, consumer });
  if (!submissions.length) return false;
  if (!isCurrent()) throw new Error("The conversation changed. Submitted feedback is saved; return to the original conversation and run /mark again.");
  const delivery = { reviewId, consumer, generation, submissions };
  await send(feedbackPrompt(reviewId, submissions), delivery);
  if (acknowledge) acknowledgeFeedback(bridge, delivery);
  return true;
}
