import assert from "node:assert/strict";
import test from "node:test";

import extension, { dxInvocationNeedsGit, parseCommandLine } from "../extensions/pi-dx.ts";

test("extension registers /diff", () => {
  let registered;
  extension({
    registerCommand(name, options) {
      registered = { name, description: options.description };
    },
  });

  assert.deepEqual(registered, { name: "diff", description: "Open the current diff in dx" });
});

test("parseCommandLine splits whitespace", () => {
  assert.deepEqual(parseCommandLine("--staged --base main"), ["--staged", "--base", "main"]);
});

test("parseCommandLine preserves quoted arguments", () => {
  assert.deepEqual(parseCommandLine('--patch "changes with spaces.diff"'), [
    "--patch",
    "changes with spaces.diff",
  ]);
});

test("parseCommandLine rejects unterminated quotes", () => {
  assert.throws(() => parseCommandLine('--patch "missing'), /Unterminated double quote/);
});

test("dxInvocationNeedsGit allows patch files", () => {
  assert.equal(dxInvocationNeedsGit(["--patch", "changes.diff"]), false);
  assert.equal(dxInvocationNeedsGit(["--patch=changes.diff"]), false);
});

test("dxInvocationNeedsGit allows full GitHub pull request URLs", () => {
  assert.equal(dxInvocationNeedsGit(["--pr", "https://github.com/owner/repo/pull/123"]), false);
  assert.equal(dxInvocationNeedsGit(["--pr", "https://github.com/owner/repo/pull/123/"]), false);
});

test("dxInvocationNeedsGit requires git for regular diffs and pull request numbers", () => {
  assert.equal(dxInvocationNeedsGit([]), true);
  assert.equal(dxInvocationNeedsGit(["--staged"]), true);
  assert.equal(dxInvocationNeedsGit(["--pr", "123"]), true);
});
