import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, realpathSync, existsSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

test("annotate refuses a non-terminal without writing a result or review history", { skip: !process.env.MARK_TEST_BIN }, () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "mark-terminal-backend-")));
  try {
    const repository = join(root, "repo"); mkdirSync(repository);
    const git = args => execFileSync("git", args, { cwd: repository, stdio: "pipe" });
    git(["init", "--quiet"]); git(["config", "user.name", "Mark tests"]); git(["config", "user.email", "mark@example.invalid"]);
    writeFileSync(join(repository, "example.txt"), "before\n"); git(["add", "."]); git(["commit", "--quiet", "-m", "baseline"]);
    writeFileSync(join(repository, "example.txt"), "after\n"); writeFileSync(join(repository, "untracked.txt"), "new file\n");
    const env = { ...process.env, XDG_STATE_HOME: join(root, "state") };
    const result = join(root, "result.json");
    const nonTTY = spawnSync(process.env.MARK_TEST_BIN, ["bridge", "annotate", "--repo", repository, "--result-file", result], { env, encoding: "utf8", stdio: "pipe" });
    assert.notEqual(nonTTY.status, 0); assert.match(nonTTY.stderr, /interactive terminal/);
    assert.equal(existsSync(result), false);
    assert.equal(existsSync(join(root, "state/mark/reviews")), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
