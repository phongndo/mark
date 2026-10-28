import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

test("fx helper rejects obsolete conversation binding arguments", () => {
  assert.throws(() => execFileSync(process.execPath, [new URL("../open-review.mjs", import.meta.url).pathname, "--conversation", "old-id"], {
    stdio: "pipe", env: { ...process.env, MARK_BIN: "/must/not/start" },
  }), error => /Usage: node open-review.mjs/.test(error.stderr.toString()));
});
