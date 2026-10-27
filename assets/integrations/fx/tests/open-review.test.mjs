import test from "node:test";
import assert from "node:assert/strict";
import { openReview, parseArguments } from "../open-review.mjs";

test("missing identity stops before opening or guessing a conversation", () => {
  let calls = 0;
  const bridge = { open() { calls++; }, web() { calls++; } };
  for (const conversation of [undefined, "", " ", "session\nother"]) {
    assert.throws(() => openReview({ conversation }, { bridge }), /current conversation ID/);
  }
  assert.equal(calls, 0);
});

test("handoff uses exact fx binding and the review returned by Mark", () => {
  const calls = [];
  const bridge = {
    open(input) {
      calls.push(input);
      return { review_id: "review-1", binding: { harness: "fx", conversation: input.conversation } };
    },
    web(id) { calls.push(id); return { review_id: id, url: "http://127.0.0.1:1234/review" }; },
  };
  const result = openReview({ conversation: "exact-fx-id", repository: "/repo with spaces" }, { bridge });
  assert.deepEqual(calls, [{ harness: "fx", conversation: "exact-fx-id", repository: "/repo with spaces" }, "review-1"]);
  assert.deepEqual(result, { review_id: "review-1", conversation: "exact-fx-id", url: "http://127.0.0.1:1234/review" });
});

test("a mismatched binding never opens the browser viewer", () => {
  const bridge = {
    open() { return { review_id: "review-1", binding: { harness: "fx", conversation: "another-id" } }; },
    web() { assert.fail("viewer must not open after identity mismatch"); },
  };
  assert.throws(() => openReview({ conversation: "exact-fx-id" }, { bridge }), /different conversation binding/);
});

test("argument parsing preserves paths and rejects ambiguous options", () => {
  assert.deepEqual(parseArguments(["--conversation", "fx-id", "--repo", "/repo with spaces"]), { conversation: "fx-id", repository: "/repo with spaces" });
  for (const args of [["--conversation"], ["--unknown", "x"], ["--repo", "--conversation"], ["--conversation", "a", "--conversation", "b"]]) {
    assert.throws(() => parseArguments(args));
  }
});
