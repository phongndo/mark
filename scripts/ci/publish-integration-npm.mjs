import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const integrity = bytes => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const registry = "https://registry.npmjs.org";

function compareVersions(left, right) {
  if (!/^\d+\.\d+\.\d+$/.test(left || "")) return -1;
  const a = left.split(".").map(Number), b = right.split(".").map(Number);
  for (let index = 0; index < 3; index++) if (a[index] !== b[index]) return Math.sign(a[index] - b[index]);
  return 0;
}

export async function publishNpmPackages(index, {
  directory,
  fetchRegistry = fetch,
  runNpm = args => execFileSync("npm", args, { stdio: "inherit" }),
  delay = pause,
} = {}) {
  assert.match(index.version, /^\d+\.\d+\.\d+$/, "only stable workspace versions can be promoted");
  assert.match(index.source_sha, /^[0-9a-f]{40}$/, "release must identify its qualified source");
  assert.equal(index.npm.length, 2, "release must contain both Pi and OpenCode npm tarballs");
  assert.equal(new Set(index.npm.map(item => item.host)).size, 2);
  assert(index.npm.some(item => item.host === "pi") && index.npm.some(item => item.host === "opencode"));
  assert.equal(new Set(index.npm.map(item => item.name)).size, 2, "npm package names must differ");

  async function metadata(name, version) {
    const response = await fetchRegistry(`${registry}/${encodeURIComponent(name)}${version ? `/${version}` : ""}`, {
      headers: { accept: "application/vnd.npm.install-v1+json" }, signal: AbortSignal.timeout(30000),
    });
    if (response.status === 404) return undefined;
    assert(response.ok, `npm registry verification failed (${response.status}) for ${name}`);
    return response.json();
  }

  async function verify(item) {
    const published = await metadata(item.name, item.version);
    if (!published) return false;
    assert.equal(published.name, item.name, "registry returned another package");
    assert.equal(published.version, item.version, "registry returned another version");
    assert.equal(published.dist?.integrity, item.integrity, `${item.name}@${item.version} already exists with different tarball integrity; never overwrite or promote it`);
    const url = new URL(published.dist.tarball);
    assert.equal(url.protocol, "https:", "registry tarball must use HTTPS");
    const tarball = await fetchRegistry(url.href, { signal: AbortSignal.timeout(30000) });
    assert(tarball.ok, `could not download published ${item.name}@${item.version}`);
    assert.equal(integrity(Buffer.from(await tarball.arrayBuffer())), item.integrity, `published tarball bytes differ for ${item.name}@${item.version}`);
    return true;
  }

  async function eventually(task, label) {
    for (let attempt = 0; attempt < 20; attempt++) {
      if (await task()) return;
      await delay(1500);
    }
    throw Error(`npm registry did not confirm ${label}; do not promote the release`);
  }

  // Preflight every existing version before the first publication. A partial
  // rerun must not hide an immutable-version conflict behind a successful skip.
  const states = [];
  for (const item of index.npm) {
    assert.equal(item.version, index.version, "npm version differs from release");
    assert.match(item.name, /^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/, "invalid npm package name");
    assert.equal(basename(item.file), item.file, "npm filename must stay inside release assets");
    assert.equal(integrity(readFileSync(join(directory, "assets", item.file))), item.integrity, "local npm artifact changed after preparation");
    const tags = (await metadata(item.name))?.["dist-tags"] || {};
    assert(compareVersions(tags.latest, item.version) <= 0, `refusing to move ${item.name} latest backwards from ${tags.latest}`);
    states.push({ item, existing: await verify(item) });
  }
  for (const { item, existing } of states) {
    if (!existing) {
      try {
        await runNpm(["publish", join(directory, "assets", item.file), "--ignore-scripts", "--access", "public", "--tag", "next", "--provenance"]);
      } catch (error) {
        throw new Error(`Publishing ${item.name}@${item.version} failed. Configure npm trusted publishing for release.yml in environment release with npm publish and npm dist-tag allowed, or provide the optional NPM_TOKEN secret. Publication was not skipped.`, { cause: error });
      }
    }
    if (!existing) await eventually(() => verify(item), `${item.name}@${item.version} integrity`);
  }
  // Only verified immutable bytes for BOTH packages can become the default.
  for (const { item } of states) {
    const tags = (await metadata(item.name))?.["dist-tags"] || {};
    if (tags.latest !== item.version) {
      assert(compareVersions(tags.latest, item.version) <= 0, "a newer npm release was published during this run");
      await runNpm(["dist-tag", "add", `${item.name}@${item.version}`, "latest"]);
      await eventually(async () => (await metadata(item.name))?.["dist-tags"]?.latest === item.version, `${item.name} latest`);
    }
  }
  return states.map(({ item, existing }) => ({ name: item.name, version: item.version, integrity: item.integrity, existing, latest: true }));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const directory = resolve(process.argv[2] || "dist/integrations-release");
  const npmVersion = execFileSync("npm", ["--version"], { encoding: "utf8" }).trim();
  assert(Number(process.versions.node.split(".")[0]) >= 24, "publication requires Node 24+");
  assert(npmVersion.startsWith("11.") ? compareVersions(npmVersion, "11.21.0") >= 0 : compareVersions(npmVersion, "12.2.0") >= 0, "OIDC dist-tag promotion requires npm 11.21.0+ or 12.2.0+ (pin npm@11.21.0 on the release runner)");
  const index = JSON.parse(readFileSync(join(directory, "release-index.json"), "utf8"));
  process.stdout.write(JSON.stringify(await publishNpmPackages(index, { directory })) + "\n");
}
