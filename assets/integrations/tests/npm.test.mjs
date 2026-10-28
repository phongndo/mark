import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, realpathSync, mkdirSync, writeFileSync, readFileSync, rmSync, lstatSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createReview } from "../common/review.mjs";

const source = dirname(fileURLToPath(import.meta.url));

test("npm tarballs include the standalone runtime and exclude internal package files", async () => {
  const task = mkdtempSync(join(realpathSync(tmpdir()), "mark-npm-pack-"));
  const env = { ...process.env, npm_config_cache: join(task, "npm-cache"), npm_config_update_notifier: "false" };
  try {
    const generated = join(task, "generated");
    execFileSync(process.execPath, [join(source, "../package.mjs"), generated], { env });
    const expected = {
      pi: ["LICENSE", "README.md", "common/review.mjs", "index.js", "package.json"],
      opencode: ["LICENSE", "README.md", "adapter.mjs", "common/review.mjs", "package.json", "server.js", "tui.js"],
    };
    const extracted = {};
    for (const host of Object.keys(expected)) {
      const directory = join(generated, host);
      mkdirSync(join(directory, "tests"), { recursive: true });
      writeFileSync(join(directory, "tests/internal.js"), "throw new Error('not a runtime file')");
      writeFileSync(join(directory, "common/internal.js"), "throw new Error('not a runtime file')");
      writeFileSync(join(directory, ".mark-integration.json"), "private installation seal");
      const [pack] = JSON.parse(execFileSync("npm", ["pack", "--json", "--ignore-scripts", "--pack-destination", task], { cwd: directory, env, encoding: "utf8" }));
      assert.deepEqual(pack.files.map(file => file.path).sort(), expected[host].sort());
      const target = join(task, "extracted", host);
      mkdirSync(target, { recursive: true });
      execFileSync("tar", ["-xzf", join(task, pack.filename), "-C", target]);
      extracted[host] = join(target, "package");
      function checkFiles(path) {
        for (const entry of readdirSync(path)) {
          const file = join(path, entry);
          assert.equal(lstatSync(file).isSymbolicLink(), false, file);
          if (lstatSync(file).isDirectory()) checkFiles(file);
        }
      }
      checkFiles(extracted[host]);
      const manifest = JSON.parse(readFileSync(join(extracted[host], "package.json"), "utf8"));
      assert.equal(manifest.license, "MIT");
      assert.equal(manifest.publishConfig.access, "public");
      for (const file of expected[host].filter(path => /\.(?:js|mjs)$/.test(path) && !path.includes("/"))) {
        assert.doesNotMatch(readFileSync(join(extracted[host], file), "utf8"), /\.\.\/common\//);
      }
    }

    const previousBinary = process.env.MARK_BIN;
    try {
      process.env.MARK_BIN = "/temporary path/mark";
      const pi = await import(pathToFileURL(join(extracted.pi, "index.js")));
      const registered = [];
      pi.default({ on() {}, registerCommand: name => registered.push(name) });
      assert.deepEqual(registered, ["mark"]);
    } finally {
      if (previousBinary === undefined) delete process.env.MARK_BIN;
      else process.env.MARK_BIN = previousBinary;
    }

    // Opt in to actual host/network checks when preparing a release. They run
    // entirely in temporary directories; no user package configuration is used.
    if (process.env.MARK_TEST_BIN && process.env.PI_TEST_BIN) {
      execFileSync("python3", [resolve(source, "../../../scripts/test-pi-review"), "--pi", process.env.PI_TEST_BIN,
        "--mark", process.env.MARK_TEST_BIN, "--package-dir", extracted.pi], { env, timeout: 90000, stdio: "pipe" });
    }
    if (process.env.MARK_NPM_VERIFY === "1") {
      execFileSync("npm", ["install", "--ignore-scripts", "--no-audit", "--no-fund", "--package-lock=false"], {
        cwd: extracted.opencode, env, timeout: 120000, stdio: "pipe",
      });
      const server = await import(pathToFileURL(join(extracted.opencode, "server.js")));
      const tui = await import(pathToFileURL(join(extracted.opencode, "tui.js")));
      assert.ok(server.default);
      assert.ok(tui.default);
    }
  } finally { rmSync(task, { recursive: true, force: true }); }
});

test("a missing Mark binary gives an actionable error", async () => {
  assert.throws(() => createReview({ binary: "/missing/mark" })(), /MARK_BIN/);
});
