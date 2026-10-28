import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, realpathSync, mkdirSync, writeFileSync, readFileSync, rmSync, lstatSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createBridge } from "../common/bridge.mjs";

const source = dirname(fileURLToPath(import.meta.url));

test("npm tarballs include the standalone runtime and exclude internal package files", async () => {
  const task = mkdtempSync(join(realpathSync(tmpdir()), "mark-npm-pack-"));
  const env = { ...process.env, npm_config_cache: join(task, "npm-cache"), npm_config_update_notifier: "false" };
  try {
    const generated = join(task, "generated");
    execFileSync(process.execPath, [join(source, "../package.mjs"), generated], { env });
    const expected = {
      pi: ["LICENSE", "README.md", "common/bridge.mjs", "index.js", "package.json", "skills/mark-review-loop/SKILL.md"],
      opencode: ["LICENSE", "README.md", "adapter.mjs", "common/bridge.mjs", "package.json", "server.js", "skills/mark-review-loop/SKILL.md", "tui.js"],
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
      assert.match(readFileSync(join(extracted[host], "skills/mark-review-loop/SKILL.md"), "utf8"), /feedback_wait/);
      for (const file of expected[host].filter(path => /\.(?:js|mjs)$/.test(path) && !path.includes("/"))) {
        assert.doesNotMatch(readFileSync(join(extracted[host], file), "utf8"), /\.\.\/common\//);
      }
    }

    const previousBinary = process.env.MARK_BIN;
    try {
      process.env.MARK_BIN = "/temporary path/mark";
      const pi = await import(pathToFileURL(join(extracted.pi, "index.js")));
      const registered = [];
      pi.default({ on() {}, registerCommand: name => registered.push(name), registerMcpServer: (name, config) => {
        assert.equal(name, "mark");
        assert.equal(config.command, process.env.MARK_BIN);
        assert.deepEqual(config.args, ["mcp"]);
      } });
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

test("missing and incompatible Mark binaries give an actionable dependency error", () => {
  const missing = createBridge({ binary: "/missing/mark", spawn: () => ({ error: new Error("ENOENT") }) });
  assert.throws(() => missing.open({ harness: "pi", conversation: "s1", repository: "/repo" }), /Mark 0\.16\.0 or newer[\s\S]*MARK_BIN/);
  const outdated = createBridge({ spawn: () => ({ status: 2, stdout: "", stderr: "error: unrecognized subcommand 'bridge'" }) });
  assert.throws(() => outdated.open({ harness: "pi", conversation: "s1", repository: "/repo" }), /releases[\s\S]*restart the host/);
  const unsupported = createBridge({ spawn: () => ({ status: 0, stdout: "{}" }) });
  assert.throws(() => unsupported.call("review.get", {}), /unsupported bridge response[\s\S]*Mark 0\.16\.0/);
  const backendError = createBridge({ spawn: () => ({ status: 1, stdout: JSON.stringify({ ok: false, error: { message: "review not found" } }) }) });
  assert.throws(() => backendError.call("review.get", {}), /^Error: review not found$/);
});
