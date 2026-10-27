import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createBridge, deliverFeedback } from "../common/bridge.mjs";

test("native adapter round trip against the real Mark backend", { skip: !process.env.MARK_TEST_BIN }, async () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "mark-adapter-backend-")));
  try {
    const git = args => execFileSync("git", args, { cwd: root, stdio: "pipe" });
    git(["init", "--quiet"]);
    git(["config", "user.name", "Mark integration test"]);
    git(["config", "user.email", "mark-test@example.invalid"]);
    writeFileSync(join(root, "example.txt"), "before\n");
    git(["add", "example.txt"]);
    git(["commit", "--quiet", "-m", "baseline"]);
    writeFileSync(join(root, "example.txt"), "after\n");
    const bridge = createBridge({ binary: process.env.MARK_TEST_BIN, spawn: (binary, args, options) => spawnSync(binary, args, { ...options, env: { ...process.env, XDG_STATE_HOME: join(root, "state") } }) });
    const review = bridge.open({ harness: "pi", conversation: "native-round-trip", repository: root });
    const mutation = () => ({ review_id: review.review_id, generation: review.generation, client_mutation_id: crypto.randomUUID() });
    const note = bridge.call("comment.add", { ...mutation(), anchor: { file: "example.txt", new_line: 1 }, origin: "human", body: "Preserve the previous behavior." });
    assert.equal(bridge.call("feedback.get", { review_id: review.review_id }).submissions.length, 0);
    const submitted = bridge.call("feedback.submit", { ...mutation(), thread_ids: [note.thread.thread_id] });
    bridge.call("review.finish", { ...mutation(), outcome: "submitted", submission_ids: [submitted.submission.submission_id] });
    let prompt;
    assert.equal(await deliverFeedback(bridge, review.review_id, "pi:native-round-trip", text => { prompt = text; }, () => true), true);
    assert.match(prompt, /Preserve the previous behavior/);
    assert.match(prompt, new RegExp(submitted.submission.submission_id));
    assert.equal(bridge.call("feedback.get", { review_id: review.review_id, consumer: "pi:native-round-trip" }).submissions.length, 0);
    assert.equal(bridge.open({ harness: "pi", conversation: "native-round-trip", repository: root }).review_id, review.review_id);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
