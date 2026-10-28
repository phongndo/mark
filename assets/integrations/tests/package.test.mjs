import test from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdtempSync, realpathSync, readFileSync, readdirSync, rmSync, lstatSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";
import { harnesses, packageFiles, parseArguments, workspaceVersion, writePackages } from "../package.mjs";

function allFiles(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory()
    ? allFiles(join(directory, entry.name), prefix + entry.name + "/") : [prefix + entry.name]).sort();
}
const temporary = prefix => mkdtempSync(join(realpathSync(tmpdir()), prefix));

test("generated packages load outside the source tree with only the needed host entry", async () => {
  const output = temporary("mark-packages-");
  try {
    writePackages(output);
    const pi = await import(pathToFileURL(join(output, "pi/index.js")));
    const commands = [];
    pi.default({ on() {}, registerCommand: name => commands.push(name) });
    assert.deepEqual(commands, ["mark"]);
    const opencode = await import(pathToFileURL(join(output, "opencode/adapter.mjs")));
    assert.equal(typeof opencode.setupMark, "function");
    for (const host of harnesses) {
      assert.equal(existsSync(join(output, host, ".mcp.json")), false);
      assert.equal(existsSync(join(output, host, "mcp.json")), false);
      assert.equal(lstatSync(join(output, host, "common/review.mjs")).isSymbolicLink(), false);
      assert.equal(existsSync(join(output, host, "skills/mark/SKILL.md")), ["codex", "fx"].includes(host));
    }
    const manifest = JSON.parse(readFileSync(join(output, "codex/.codex-plugin/plugin.json"), "utf8"));
    assert.equal(manifest.mcpServers, undefined);
  } finally { rmSync(output, { recursive: true, force: true }); }
});

test("terminal skills execute after their parent packages are removed", () => {
  const task = temporary("mark-detached-skills-");
  try {
    const output = join(task, "packages"); writePackages(output);
    const binary = join(task, "mark");
    writeFileSync(binary, '#!/bin/sh\nprintf \'%s\\n\' \'{"markdown":"- feedback"}\' > "$6"\n', { mode: 0o700 });
    for (const host of ["codex", "fx"]) {
      const detached = join(task, host); cpSync(join(output, host, "skills/mark"), detached, { recursive: true });
      rmSync(join(output, host), { recursive: true });
      assert.equal(execFileSync(process.execPath, [join(detached, "open-review.mjs")], {
        env: { ...process.env, MARK_BIN: binary }, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
      }), "- feedback\n");
    }
  } finally { rmSync(task, { recursive: true, force: true }); }
});

test("release packages are deterministic and omit development resources", () => {
  const output = temporary("mark-public-packages-");
  try {
    const options = parseArguments([output, "--version", "12.34.56-rc.1"]); writePackages(options.output, options.version);
    for (const host of harnesses) {
      const files = packageFiles(host, options.version);
      assert.deepEqual([...files], [...packageFiles(host, options.version)]);
      assert.ok([...files.keys()].every(path => !/(^|\/)(?:tests|__tests__|node_modules)(\/|$)/.test(path)));
      for (const [path, bytes] of files) assert.deepEqual(readFileSync(join(output, host, path)), bytes);
      const manifest = host === "claude" ? ".claude-plugin/plugin.json" : host === "codex" ? ".codex-plugin/plugin.json" : "package.json";
      assert.equal(JSON.parse(files.get(manifest)).version, options.version);
    }
    for (const version of ["latest", "1.2", "1.2.3-01", "v1.2.3"]) assert.throws(() => parseArguments([output, "--version", version]));
  } finally { rmSync(output, { recursive: true, force: true }); }
});

test("modified generated packages are preserved", () => {
  const output = temporary("mark-generator-drift-");
  try {
    writePackages(output); writeFileSync(join(output, "pi/index.js"), "local edit");
    assert.throws(() => writePackages(output), /Preserving modified/);
    assert.equal(readFileSync(join(output, "pi/index.js"), "utf8"), "local edit");
  } finally { rmSync(output, { recursive: true, force: true }); }
});

test("public packages match installed-binary exports", { skip: !process.env.MARK_TEST_BIN }, () => {
  const task = temporary("mark-package-parity-");
  try {
    const generated = join(task, "generated"); writePackages(generated, workspaceVersion());
    for (const host of harnesses) {
      const installed = join(task, "installed", host);
      execFileSync(process.env.MARK_TEST_BIN, ["integration", "export", "--agent", host, "--output", installed]);
      const paths = allFiles(join(generated, host));
      assert.deepEqual(allFiles(installed).filter(path => path !== ".mark-integration.json"), paths);
      for (const path of paths) assert.deepEqual(readFileSync(join(installed, path)), readFileSync(join(generated, host, path)), host + "/" + path);
    }
  } finally { rmSync(task, { recursive: true, force: true }); }
});
