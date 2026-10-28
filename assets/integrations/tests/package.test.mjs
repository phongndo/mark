import test from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, realpathSync, readFileSync, readdirSync, rmSync, lstatSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";
import { harnesses, packageFiles, parseArguments, workspaceVersion, writePackages } from "../package.mjs";

function allFiles(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory()
    ? allFiles(join(directory, entry.name), prefix + entry.name + "/") : [prefix + entry.name]).sort();
}

test("generated packages load outside the source tree and carry a versioned skill", async () => {
  const output = mkdtempSync(join(realpathSync(tmpdir()), "mark-packages-"));
  try {
    const source = dirname(fileURLToPath(import.meta.url));
    execFileSync(process.execPath, [join(source, "../package.mjs"), output]);
    const pi = await import(pathToFileURL(join(output, "pi/index.js")));
    const commands = [];
    pi.default({ on() {}, registerMcpServer() {}, registerCommand: name => commands.push(name) });
    assert.deepEqual(commands, ["mark"]);
    const opencode = await import(pathToFileURL(join(output, "opencode/adapter.mjs")));
    assert.equal(typeof opencode.setupMark, "function");
    for (const [host, name] of [["pi", "mark-review-loop"], ["claude", "review-loop"], ["codex", "mark"], ["opencode", "mark-review-loop"], ["fx", "mark"]]) {
      const path = join(output, host, "skills", name, "SKILL.md");
      assert.equal(lstatSync(path).isSymbolicLink(), false);
      assert.match(readFileSync(path, "utf8"), /feedback_wait/);
      assert.match(readFileSync(path, "utf8"), new RegExp(`name: ${name}\\n`));
    }
    const manifest = JSON.parse(readFileSync(join(output, "codex/.codex-plugin/plugin.json"), "utf8"));
    assert.equal(manifest.mcpServers, "./.mcp.json");
    assert.ok(manifest.version);
    const fx = await import(pathToFileURL(join(output, "fx/open-review.mjs")));
    assert.equal(typeof fx.openReview, "function");
    assert.match(readFileSync(join(output, "fx/skills/mark/HOST.md"), "utf8"), /no supported custom `\/mark`/);
  } finally { rmSync(output, { recursive: true, force: true }); }
});

test("fx skill is an executable detached consumer after its parent package is removed", async () => {
  const temporary = mkdtempSync(join(realpathSync(tmpdir()), "mark-detached-fx-"));
  try {
    const output = join(temporary, "packages");
    writePackages(output);
    const detached = join(temporary, "installed", "mark");
    mkdirSync(dirname(detached), { recursive: true });
    cpSync(join(output, "fx/skills/mark"), detached, { recursive: true });
    rmSync(output, { recursive: true });
    const helper = await import(pathToFileURL(join(detached, "open-review.mjs")));
    const binding = { harness: "fx", conversation: "current-fx-conversation" };
    const calls = [];
    const result = helper.openReview({ conversation: binding.conversation, repository: temporary }, {
      bridge: {
        open(input) { calls.push(input); return { review_id: "review-1", binding }; },
        web(review_id) { calls.push(review_id); return { review_id, url: "http://127.0.0.1:1234/review" }; },
      },
    });
    assert.equal(result.review_id, "review-1");
    assert.equal(calls[0].conversation, binding.conversation);
    assert.equal(calls[1], "review-1");
    assert.match(readFileSync(join(detached, "HOST.md"), "utf8"), /`\.\/open-review\.mjs`/);
    assert.ok(allFiles(detached).includes("common/bridge.mjs"));
    assert.ok(allFiles(detached).includes("LICENSE"));
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});

test("release version is explicit, deterministic, and no development files ship", () => {
  const temporary = mkdtempSync(join(realpathSync(tmpdir()), "mark-public-packages-"));
  try {
    const options = parseArguments([temporary, "--version", "12.34.56-rc.1"]);
    assert.equal(options.version, "12.34.56-rc.1");
    writePackages(options.output, options.version);
    for (const harness of harnesses) {
      const first = packageFiles(harness, options.version);
      const second = packageFiles(harness, options.version);
      assert.deepEqual([...first], [...second]);
      assert.ok([...first.keys()].every(path => !/(^|\/)(?:tests|__tests__|node_modules)(\/|$)/.test(path)));
      for (const [name, bytes] of first) assert.deepEqual(readFileSync(join(temporary, harness, name)), bytes);
      const path = harness === "claude" ? ".claude-plugin/plugin.json" : harness === "codex" ? ".codex-plugin/plugin.json" : "package.json";
      assert.equal(JSON.parse(first.get(path)).version, options.version);
    }
    const codex = packageFiles("codex", options.version);
    assert.equal(JSON.parse(codex.get("plugin.json")).version, options.version);
    assert.equal(JSON.parse(codex.get("mcp.json")).mcpServers.mark.type, "stdio");
    assert.ok(codex.has(".agents/plugins/marketplace.json"));
    assert.ok(packageFiles("claude", options.version).has(".claude-plugin/marketplace.json"));
    for (const version of ["latest", "file:../source", "1.2", "1.2.3-01", "1.2.3\n", "v1.2.3"]) assert.throws(() => parseArguments([temporary, "--version", version]));
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});

test("generated package drift is preserved rather than overwritten", () => {
  const temporary = mkdtempSync(join(realpathSync(tmpdir()), "mark-generator-drift-"));
  try {
    writePackages(temporary);
    writeFileSync(join(temporary, "pi/index.js"), "local edit");
    assert.throws(() => writePackages(temporary), /Preserving modified/);
    assert.equal(readFileSync(join(temporary, "pi/index.js"), "utf8"), "local edit");
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});

test("public packages and installed-binary exports have identical consumer files", { skip: !process.env.MARK_TEST_BIN && "MARK_TEST_BIN supplies the freshly built binary" }, () => {
  const temporary = mkdtempSync(join(realpathSync(tmpdir()), "mark-package-parity-"));
  try {
    const generated = join(temporary, "generated");
    writePackages(generated, workspaceVersion());
    for (const harness of harnesses) {
      const installed = join(temporary, "installed", harness);
      execFileSync(process.env.MARK_TEST_BIN, ["integration", "export", "--agent", harness, "--output", installed]);
      const expected = allFiles(join(generated, harness));
      const actual = allFiles(installed).filter(path => path !== ".mark-integration.json");
      assert.deepEqual(actual, expected, `${harness} file set`);
      for (const path of expected) assert.deepEqual(readFileSync(join(installed, path)), readFileSync(join(generated, harness, path)), `${harness}/${path}`);
    }
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});
