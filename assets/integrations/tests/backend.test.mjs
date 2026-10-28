import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createBridge, deliverFeedback } from "../common/bridge.mjs";
import { harnesses } from "../package.mjs";

for (const harness of harnesses) test(`${harness} round trip with an older checkpoint against the real Mark backend`, { skip: !process.env.MARK_TEST_BIN }, async () => {
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
    const directory = join(root, "state/mark/reviews");
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const olderCheckpoint = join(directory, "e2400d07f9833faa1abca4ee4bfd502b.json");
    const olderBytes = JSON.stringify({ schema: 1, identity: {}, comments: [] });
    writeFileSync(olderCheckpoint, olderBytes, { mode: 0o600 });
    const bridge = createBridge({ binary: process.env.MARK_TEST_BIN, spawn: (binary, args, options) => spawnSync(binary, args, { ...options, env: { ...process.env, XDG_STATE_HOME: join(root, "state") } }) });
    const review = bridge.open({ harness, conversation: "native-round-trip", repository: root });
    assert.match(review.review_id, /^[0-9a-f]{24}$/);
    assert.equal(review.binding.harness, harness);
    const mutation = () => ({ review_id: review.review_id, generation: review.generation, client_mutation_id: crypto.randomUUID() });
    const note = bridge.call("comment.add", { ...mutation(), anchor: { file: "example.txt", new_line: 1 }, origin: "human", body: "Preserve the previous behavior." });
    assert.equal(bridge.call("feedback.get", { review_id: review.review_id }).submissions.length, 0);
    const submitted = bridge.call("feedback.submit", { ...mutation(), thread_ids: [note.thread.thread_id] });
    bridge.call("review.finish", { ...mutation(), outcome: "submitted", submission_ids: [submitted.submission.submission_id] });
    let prompt;
    const consumer = `${harness}:native-round-trip`;
    assert.equal(await deliverFeedback(bridge, review.review_id, consumer, text => { prompt = text; }, () => true), true);
    assert.match(prompt, /Preserve the previous behavior/);
    assert.match(prompt, new RegExp(submitted.submission.submission_id));
    assert.equal(bridge.call("feedback.get", { review_id: review.review_id, consumer }).submissions.length, 0);
    bridge.call("comment.reply", { ...mutation(), origin: "agent", thread_id: note.thread.thread_id,
      reply_to: note.message.message_id, body: "Restored the previous behavior and checked the result." });
    writeFileSync(join(root, "example.txt"), "fixed\n");
    const nextPass = bridge.open({ harness, conversation: "native-round-trip", repository: root });
    assert.equal(nextPass.review_id, review.review_id);
    assert.equal(nextPass.generation, review.generation + 1);
    assert.match(nextPass.snapshot.patch, /\+fixed/);
    assert.equal(nextPass.threads[0].thread_id, note.thread.thread_id);
    assert.equal(nextPass.threads[0].messages.at(-1).origin, "agent");
    assert.deepEqual(bridge.call("review.list", {}).reviews.map(review => review.review_id), [review.review_id]);
    assert.equal(readFileSync(olderCheckpoint, "utf8"), olderBytes);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
