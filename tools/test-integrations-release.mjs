import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32 } from "node:zlib";
import { prepareIntegrations, validatePacklist } from "../scripts/ci/prepare-integrations.mjs";
import { publishNpmPackages } from "../scripts/ci/publish-integration-npm.mjs";

const project = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sha = "a".repeat(40);
const sri = bytes => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
const json = path => JSON.parse(readFileSync(path, "utf8"));

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "mark-release-test-"));
  const source = join(root, "source");
  mkdirSync(join(source, "assets/skills"), { recursive: true });
  cpSync(join(project, "assets/integrations"), join(source, "assets/integrations"), { recursive: true });
  cpSync(join(project, "assets/skills/agent-review"), join(source, "assets/skills/agent-review"), { recursive: true });
  cpSync(join(project, "LICENSE"), join(source, "LICENSE"));
  writeFileSync(join(source, "Cargo.toml"), '[workspace.package]\nversion = "1.2.3"\nrepository = "https://github.com/phongndo/mark"\n\n[workspace.dependencies]\n');
  return { root, source, output: join(root, "release"), cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

function entries(archive) {
  const result = new Map();
  let offset = 0;
  while (archive.readUInt32LE(offset) === 0x04034b50) {
    assert.equal(archive.readUInt16LE(offset + 8), 0, "ZIP uses stored entries");
    const length = archive.readUInt32LE(offset + 22), nameLength = archive.readUInt16LE(offset + 26), extra = archive.readUInt16LE(offset + 28);
    const name = archive.subarray(offset + 30, offset + 30 + nameLength).toString();
    const dataOffset = offset + 30 + nameLength + extra;
    const bytes = archive.subarray(dataOffset, dataOffset + length);
    assert.equal(crc32(bytes), archive.readUInt32LE(offset + 14), `ZIP CRC for ${name}`);
    result.set(name, bytes);
    offset = dataOffset + length;
  }
  const end = archive.length - 22;
  assert.equal(archive.readUInt32LE(end), 0x06054b50);
  assert.equal(archive.readUInt16LE(end + 10), result.size);
  assert.equal(archive.readUInt32LE(end + 16), offset);
  return result;
}

test("one-version artifacts contain detached packages, npm packlists, and valid branch catalogs", () => {
  const f = fixture();
  try {
    const index = prepareIntegrations({ sourceRoot: f.source, output: f.output, version: "1.2.3", sourceSha: sha });
    assert.equal(index.version, "1.2.3"); assert.equal(index.source_sha, sha);
    assert.equal(index.plugins.length, 5); assert.equal(index.npm.length, 2);
    for (const plugin of index.plugins) {
      assert.equal(plugin.version, index.version);
      const archive = readFileSync(join(f.output, "assets", plugin.file));
      assert.equal(createHash("sha256").update(archive).digest("hex"), plugin.sha256);
      const content = entries(archive);
      assert(content.has("LICENSE") && content.has("README.md") && content.has("common/bridge.mjs"));
      assert([...content.keys()].some(path => /^skills\/[^/]+\/SKILL\.md$/.test(path)));
      assert(![...content.keys()].some(path => path.includes("/tests/") || path.startsWith("tests/")));
    }
    for (const item of index.npm) {
      assert.equal(item.version, index.version);
      assert.equal(sri(readFileSync(join(f.output, "assets", item.file))), item.integrity);
      assert(item.files.includes("common/bridge.mjs"));
      assert(item.files.includes("skills/mark-review-loop/SKILL.md"));
    }
    const distribution = join(f.output, "marketplace");
    const claude = json(join(distribution, ".claude-plugin/marketplace.json"));
    const codex = json(join(distribution, ".agents/plugins/marketplace.json"));
    assert.equal(claude.name, "mark"); assert.equal(codex.name, "mark");
    assert.deepEqual(codex.plugins, [{ name: "mark", source: { source: "local", path: "./plugins/codex" }, policy: { installation: "AVAILABLE", authentication: "ON_INSTALL" }, category: "Productivity" }], "public Codex catalog must declare the native installation and authentication policy");
    assert.equal(json(join(distribution, claude.plugins[0].source, ".claude-plugin/plugin.json")).version, "1.2.3");
    const codexRoot = join(distribution, codex.plugins[0].source.path);
    assert.equal(json(join(codexRoot, "plugin.json")).version, "1.2.3");
    assert.equal(json(join(distribution, "release.json")).source_sha, sha);
    assert.match(readFileSync(join(distribution, "README.md"), "utf8"), /phongndo\/mark#integrations/);
    const second = prepareIntegrations({ sourceRoot: f.source, output: join(f.root, "repeat"), version: "1.2.3", sourceSha: sha });
    assert.deepEqual(second, index, "regeneration must preserve every npm SRI and ZIP checksum");
  } finally { f.cleanup(); }
});

test("preparation rejects source/version mismatches instead of publishing an override", () => {
  const f = fixture();
  try {
    assert.throws(() => prepareIntegrations({ sourceRoot: f.source, output: f.output, version: "1.2.4", sourceSha: sha }), /differs from Cargo/);
    assert.throws(() => prepareIntegrations({ sourceRoot: f.source, output: f.output, sourceSha: "main" }), /exact source commit/);
  } finally { f.cleanup(); }
});

test("npm packlist validation rejects an omitted runtime skill", () => {
  const root = mkdtempSync(join(tmpdir(), "mark-packlist-test-"));
  try {
    mkdirSync(join(root, "skills/review"), { recursive: true });
    writeFileSync(join(root, "skills/review/SKILL.md"), "review");
    writeFileSync(join(root, "package.json"), JSON.stringify({ name: "test-pi", version: "1.2.3", license: "MIT", repository: "git+https://github.com/phongndo/mark.git", publishConfig: { access: "public" }, pi: { extensions: ["./index.js"] } }));
    const metadata = { name: "test-pi", version: "1.2.3", files: ["package.json", "LICENSE", "README.md", "common/bridge.mjs", "index.js"].map(path => ({ path })) };
    assert.throws(() => validatePacklist(metadata, root, "pi", "1.2.3"), /omits runtime file skills\/review\/SKILL.md/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

function registryFixture() {
  const directory = mkdtempSync(join(tmpdir(), "mark-publish-test-"));
  mkdirSync(join(directory, "assets"));
  const index = { version: "1.2.3", source_sha: sha, npm: ["pi", "opencode"].map(host => {
    const bytes = Buffer.from(`immutable ${host} package bytes`), file = `${host}.tgz`;
    writeFileSync(join(directory, "assets", file), bytes);
    return { host, name: `@fixture/mark-${host}`, version: "1.2.3", file, integrity: sri(bytes) };
  }) };
  const published = new Map(), tags = new Map(), events = [], writes = [];
  const fetchRegistry = async input => {
    const url = new URL(input);
    if (url.pathname.startsWith("/tarballs/")) {
      const name = decodeURIComponent(url.pathname.slice("/tarballs/".length));
      events.push(`download:${name}`);
      return new Response(published.get(name).bytes);
    }
    const [encoded, version] = url.pathname.slice(1).split("/"), name = decodeURIComponent(encoded);
    if (!version) return new Response(JSON.stringify({ "dist-tags": tags.get(name) || {} }));
    if (!published.has(name)) return new Response("not found", { status: 404 });
    const item = index.npm.find(item => item.name === name), value = published.get(name);
    return new Response(JSON.stringify({ name, version: item.version, dist: { integrity: value.integrity, tarball: `https://registry.npmjs.org/tarballs/${encodeURIComponent(name)}` } }));
  };
  const runNpm = async args => {
    writes.push(args);
    if (args[0] === "publish") {
      const item = index.npm.find(item => args[1].endsWith(item.file));
      published.set(item.name, { bytes: readFileSync(join(directory, "assets", item.file)), integrity: item.integrity });
      tags.set(item.name, { next: item.version });
      events.push(`publish:${item.name}`);
    } else {
      assert.equal(args[0], "dist-tag");
      const item = index.npm.find(item => args[2] === `${item.name}@${item.version}`);
      assert(index.npm.every(other => events.includes(`download:${other.name}`)), "all tarballs must verify before any latest promotion");
      tags.set(item.name, { ...(tags.get(item.name) || {}), latest: item.version });
      events.push(`promote:${item.name}`);
    }
  };
  return { directory, index, published, tags, writes, events, fetchRegistry, runNpm, delay: async () => {}, cleanup: () => rmSync(directory, { recursive: true, force: true }) };
}

test("new npm versions stage first and both published tarballs verify before latest promotion", async () => {
  const f = registryFixture();
  try {
    const result = await publishNpmPackages(f.index, f);
    assert.equal(result.length, 2); assert(result.every(item => item.latest && !item.existing));
    assert.equal(f.writes.filter(args => args[0] === "publish").length, 2);
    assert(f.writes.filter(args => args[0] === "publish").every(args => args.includes("next") && args.includes("--provenance")));
    assert.equal(f.writes.filter(args => args[0] === "dist-tag").length, 2);
  } finally { f.cleanup(); }
});

test("reruns verify existing bytes and do not republish or retag completed versions", async () => {
  const f = registryFixture();
  try {
    for (const item of f.index.npm) {
      f.published.set(item.name, { bytes: readFileSync(join(f.directory, "assets", item.file)), integrity: item.integrity });
      f.tags.set(item.name, { latest: item.version });
    }
    const result = await publishNpmPackages(f.index, f);
    assert(result.every(item => item.existing)); assert.equal(f.writes.length, 0);
    assert(f.index.npm.every(item => f.events.includes(`download:${item.name}`)));
  } finally { f.cleanup(); }
});

test("a conflicting existing sibling version blocks all publication and promotion", async () => {
  const f = registryFixture();
  try {
    const item = f.index.npm[1];
    f.published.set(item.name, { bytes: Buffer.from("other bytes"), integrity: sri(Buffer.from("other bytes")) });
    await assert.rejects(publishNpmPackages(f.index, f), /already exists with different tarball integrity/);
    assert.equal(f.writes.length, 0);
  } finally { f.cleanup(); }
});

test("published tarball bytes are checked even when registry metadata claims matching integrity", async () => {
  const f = registryFixture();
  try {
    const item = f.index.npm[0];
    f.published.set(item.name, { bytes: Buffer.from("corrupt download"), integrity: item.integrity });
    await assert.rejects(publishNpmPackages(f.index, f), /published tarball bytes differ/);
    assert.equal(f.writes.length, 0);
  } finally { f.cleanup(); }
});

test("missing publication authorization fails clearly and never promotes", async () => {
  const f = registryFixture();
  try {
    await assert.rejects(publishNpmPackages(f.index, { ...f, runNpm: async () => { throw Error("ENEEDAUTH"); } }), /Publication was not skipped/);
    assert.equal(f.writes.length, 0);
  } finally { f.cleanup(); }
});

test("registry outages are failures rather than evidence of absent packages", async () => {
  const f = registryFixture();
  try {
    await assert.rejects(publishNpmPackages(f.index, { ...f, fetchRegistry: async () => new Response("outage", { status: 503 }) }), /registry verification failed \(503\)/);
    assert.equal(f.writes.length, 0);
  } finally { f.cleanup(); }
});

test("an older release cannot move a package's latest tag backwards", async () => {
  const f = registryFixture();
  try {
    f.tags.set(f.index.npm[0].name, { latest: "1.3.0" });
    await assert.rejects(publishNpmPackages(f.index, f), /latest backwards/);
    assert.equal(f.writes.length, 0);
  } finally { f.cleanup(); }
});
