import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { crc32 } from "node:zlib";

const project = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const hosts = ["pi", "claude", "codex", "opencode", "fx"];
const json = path => JSON.parse(readFileSync(path, "utf8"));
const writeJson = (path, value) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, JSON.stringify(value, null, 2) + "\n"); };
const hash = (bytes, algorithm) => createHash(algorithm).update(bytes).digest(algorithm === "sha512" ? "base64" : "hex");

export function workspaceVersion(sourceRoot) {
  const cargo = readFileSync(join(sourceRoot, "Cargo.toml"), "utf8");
  const section = /^\[workspace\.package\]\s*\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(cargo)?.[1];
  const version = /^version\s*=\s*"([^"]+)"/m.exec(section || "")?.[1];
  assert.match(version || "", /^\d+\.\d+\.\d+$/, "stable release requires a three-part workspace version");
  return version;
}

function files(directory, base = directory) {
  return readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0).flatMap(entry => {
    const path = join(directory, entry.name);
    assert(!entry.isSymbolicLink(), `release package cannot contain a symlink: ${path}`);
    return entry.isDirectory() ? files(path, base) : [relative(base, path).replaceAll("\\", "/")];
  });
}

// Stored ZIP entries avoid platform-dependent compressors and timestamps. All
// content, including dotfiles, is rooted at the standalone plugin directory.
export function zipPackage(directory) {
  const bodies = [], entries = [];
  let offset = 0;
  for (const path of files(directory)) {
    const name = Buffer.from(path), body = readFileSync(join(directory, path));
    assert(name.length <= 65535 && body.length <= 0xffffffff, "plugin exceeds ZIP32 limits");
    const header = Buffer.alloc(30);
    header.writeUInt32LE(0x04034b50); header.writeUInt16LE(20, 4); header.writeUInt16LE(0x800, 6);
    header.writeUInt16LE(0x21, 12); header.writeUInt32LE(crc32(body), 14);
    header.writeUInt32LE(body.length, 18); header.writeUInt32LE(body.length, 22); header.writeUInt16LE(name.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50); central.writeUInt16LE(0x314, 4); central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x800, 8); central.writeUInt16LE(0x21, 14); central.writeUInt32LE(crc32(body), 16);
    central.writeUInt32LE(body.length, 20); central.writeUInt32LE(body.length, 24); central.writeUInt16LE(name.length, 28);
    const mode = lstatSync(join(directory, path)).mode & 0o111 ? 0o100755 : 0o100644;
    central.writeUInt32LE((mode << 16) >>> 0, 38); central.writeUInt32LE(offset, 42);
    bodies.push(header, name, body); entries.push(central, name);
    offset += header.length + name.length + body.length;
  }
  const central = Buffer.concat(entries), end = Buffer.alloc(22), count = entries.length / 2;
  assert(count <= 65535 && offset + central.length <= 0xffffffff, "plugin exceeds ZIP32 limits");
  end.writeUInt32LE(0x06054b50); end.writeUInt16LE(count, 8); end.writeUInt16LE(count, 10);
  end.writeUInt32LE(central.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...bodies, central, end]);
}

function manifestPaths(host) {
  if (host === "claude") return [".claude-plugin/plugin.json"];
  if (host === "codex") return ["plugin.json", ".codex-plugin/plugin.json"];
  return ["package.json"];
}

function validatePackage(directory, host, version) {
  const paths = files(directory);
  assert(paths.includes("LICENSE") && paths.includes("README.md"), `${host} must include license and documentation`);
  assert(paths.includes("common/review.mjs"), `${host} must include its shared bridge`);
  if (["codex", "fx"].includes(host)) assert(paths.includes("skills/mark/SKILL.md"), host + " must include its annotation skill");
  assert(!paths.some(path => /(^|\/)(?:node_modules|tests?|fixtures)(?:\/|$)/.test(path)), `${host} contains development files`);
  const manifests = manifestPaths(host).filter(path => existsSync(join(directory, path)));
  assert(manifests.length, `${host} has no host manifest`);
  for (const path of manifests) assert.equal(json(join(directory, path)).version, version, `${host} manifest version mismatch`);
  return { manifest: json(join(directory, manifests[0])), paths };
}

export function validatePacklist(metadata, directory, host, version) {
  const manifest = json(join(directory, "package.json"));
  assert.equal(metadata.name, manifest.name, "npm pack name differs from manifest");
  assert.equal(metadata.version, version, "npm pack version differs from workspace");
  assert(!manifest.private, `${host} cannot publish a private package`);
  assert.equal(manifest.publishConfig?.access, "public", `${host} must declare public publication`);
  assert.equal(manifest.license, "MIT", `${host} must declare its license`);
  const repository = typeof manifest.repository === "string" ? manifest.repository : manifest.repository?.url;
  assert.equal(repository?.replace(/^git\+/, "").replace(/\.git\/?$/, "").replace(/\/$/, ""), "https://github.com/phongndo/mark", `${host} repository must match the public release source for provenance`);
  const packed = new Set(metadata.files.map(file => file.path));
  const required = ["package.json", "LICENSE", "README.md", "common/review.mjs"];
  for (const path of files(directory).filter(path => /^skills\/[^/]+\/SKILL\.md$/.test(path))) required.push(path);
  if (host === "pi") required.push(...manifest.pi.extensions.map(path => path.replace(/^\.\//, "")));
  if (host === "opencode") required.push(...Object.values(manifest.exports).map(path => path.replace(/^\.\//, "")), "adapter.mjs");
  for (const path of required) assert(packed.has(path), `${host} npm tarball omits runtime file ${path}`);
  for (const path of packed) assert(!/(^|\/)(?:tests?|fixtures|node_modules)(?:\/|$)/.test(path), `npm tarball includes development file ${path}`);
}

export function prepareIntegrations({ sourceRoot = project, output, version, sourceSha }) {
  assert.equal(Number(process.versions.node.split(".")[0]) >= 24, true, "release preparation requires Node 24+");
  const workspace = workspaceVersion(sourceRoot);
  version ||= workspace;
  assert.equal(version, workspace, "release version differs from Cargo.toml");
  assert.match(sourceSha || "", /^[0-9a-f]{40}$/, "release requires an exact source commit SHA");
  output = resolve(output);
  if (existsSync(output)) assert.equal(readdirSync(output).length, 0, "release output directory must be empty");
  mkdirSync(output, { recursive: true });
  const distribution = join(output, "marketplace"), plugins = join(distribution, "plugins"), assets = join(output, "assets");
  mkdirSync(assets, { recursive: true });
  execFileSync(process.execPath, [join(sourceRoot, "assets/integrations/package.mjs"), plugins, "--version", version], { stdio: "pipe" });
  const index = { schema_version: 1, version, source_sha: sourceSha, release_tag: `v${version}`, plugins: [], npm: [] };
  const npmCache = mkdtempSync(join(tmpdir(), "mark-release-npm-"));
  try {
    for (const host of hosts) {
      const directory = join(plugins, host);
      const { manifest } = validatePackage(directory, host, version);
      const filename = `mark-${host}-v${version}.zip`;
      const bytes = zipPackage(directory);
      writeFileSync(join(assets, filename), bytes);
      index.plugins.push({ host, name: manifest.name, version, file: filename, sha256: hash(bytes, "sha256") });
      if (["pi", "opencode"].includes(host)) {
        const packed = JSON.parse(execFileSync("npm", ["pack", "--json", "--ignore-scripts", "--pack-destination", assets], {
          cwd: directory, encoding: "utf8", env: { ...process.env, npm_config_cache: npmCache },
        }))[0];
        validatePacklist(packed, directory, host, version);
        const bytes = readFileSync(join(assets, packed.filename));
        const integrity = `sha512-${hash(bytes, "sha512")}`;
        assert.equal(packed.integrity, integrity, "npm pack integrity does not match its tarball");
        index.npm.push({ host, name: packed.name, version, file: packed.filename, integrity, sha256: hash(bytes, "sha256"), files: packed.files.map(file => file.path).sort() });
      }
    }
  } finally { rmSync(npmCache, { recursive: true, force: true }); }
  const claude = index.plugins.find(plugin => plugin.host === "claude"), codex = index.plugins.find(plugin => plugin.host === "codex");
  writeJson(join(distribution, ".claude-plugin/marketplace.json"), { name: "mark", owner: { name: "Mark maintainers" }, plugins: [{ name: claude.name, source: "./plugins/claude", description: "Review agent changes with Mark" }] });
  writeJson(join(distribution, ".agents/plugins/marketplace.json"), { name: "mark", interface: { displayName: "Mark" }, plugins: [{ name: codex.name, source: { source: "local", path: "./plugins/codex" }, policy: { installation: "AVAILABLE", authentication: "ON_INSTALL" }, category: "Productivity" }] });
  writeJson(join(distribution, "release.json"), { schema_version: 1, version, source_sha: sourceSha, release_tag: index.release_tag, npm: index.npm.map(({ host, name, integrity }) => ({ host, name, version, integrity })) });
  writeFileSync(join(distribution, "README.md"), `# Mark integrations\n\nGenerated from [${index.release_tag}](https://github.com/phongndo/mark/releases/tag/${index.release_tag}) at ${sourceSha}. Authoritative source and setup: [Mark integrations](https://github.com/phongndo/mark/blob/${sourceSha}/docs/integrations.md).\n\nClaude Code:\n\n\`\`\`text\n/plugin marketplace add phongndo/mark#integrations\n/plugin install ${claude.name}@mark\n\`\`\`\n\nCodex:\n\n\`\`\`sh\ncodex plugin marketplace add phongndo/mark --ref integrations\ncodex plugin add ${codex.name}@mark\n\`\`\`\n`);
  cpSync(join(sourceRoot, "LICENSE"), join(distribution, "LICENSE"));
  writeJson(join(output, "release-index.json"), index);
  const checksums = [...index.plugins, ...index.npm].map(item => `${item.sha256}  ${item.file}`).join("\n") + "\n";
  writeFileSync(join(assets, `mark-integrations-v${version}.checksums`), checksums);
  return index;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const values = {};
  for (let index = 2; index < process.argv.length; index += 2) {
    const key = process.argv[index];
    assert(["--version", "--source-sha", "--output", "--source-root"].includes(key) && process.argv[index + 1], `unknown or missing argument ${key}`);
    values[key.slice(2)] = process.argv[index + 1];
  }
  const output = values.output || "dist/integrations-release";
  const index = prepareIntegrations({ sourceRoot: values["source-root"] || project, output, version: values.version, sourceSha: values["source-sha"] });
  process.stdout.write(JSON.stringify({ output: resolve(output), version: index.version, source_sha: index.source_sha, npm: index.npm.map(({ name, file, integrity }) => ({ name, file, integrity })) }) + "\n");
}
