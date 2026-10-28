import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
export const harnesses = ["pi", "claude", "codex", "opencode", "fx"];

function runtimeAsset(path) {
  return !path.split(/[\\/]/).some(part => ["tests", "__tests__", "node_modules", ".git"].includes(part))
    && !/(?:^|\/)(?:package-lock\.json|pnpm-lock\.yaml|yarn\.lock|\.DS_Store)$/.test(path)
    && !/\.(?:test|spec)\.[^/]+$/.test(path);
}

function readTree(directory, prefix = "", filter = true) {
  const result = new Map();
  for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name, "en"))) {
    const name = prefix + entry.name;
    if (filter && !runtimeAsset(name)) continue;
    if (entry.isSymbolicLink()) throw new Error(`Integration source may not be a symlink: ${name}`);
    if (entry.isDirectory()) {
      for (const item of readTree(join(directory, entry.name), name + "/", filter)) result.set(...item);
    } else if (entry.isFile()) result.set(name, readFileSync(join(directory, entry.name)));
  }
  return result;
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  return value;
}

function jsonBytes(value) { return Buffer.from(JSON.stringify(canonical(value), null, 2) + "\n"); }

function validateVersion(version) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(version)) {
    throw new Error(`Invalid package version: ${version}`);
  }
  const prerelease = version.split("+")[0].split("-").slice(1).join("-");
  if (prerelease.split(".").some(part => /^0\d+$/.test(part))) throw new Error(`Invalid package version: ${version}`);
  return version;
}

export function workspaceVersion() {
  return /\[workspace\.package\][\s\S]*?version\s*=\s*"([^"]+)"/.exec(readFileSync(join(root, "../../Cargo.toml"), "utf8"))[1];
}

export function packageFiles(harness, version = workspaceVersion()) {
  if (!harnesses.includes(harness)) throw new Error(`Unknown integration: ${harness}`);
  validateVersion(version);
  const files = readTree(join(root, harness));
  for (const [name, bytes] of readTree(join(root, "common"), "common/")) files.set(name, bytes);
  files.set("LICENSE", readFileSync(join(root, "../../LICENSE")));
  files.set("README.md", readFileSync(join(root, "README.md")));
  for (const [name, bytes] of files) {
    if (!name.includes("/") && /\.(mjs|js)$/.test(name)) files.set(name, Buffer.from(bytes.toString("utf8").replaceAll('"../common/', '"./common/')));
  }
  if (["codex", "fx"].includes(harness)) {
    const skillDir = "skills/mark";
    files.set(`${skillDir}/SKILL.md`, readFileSync(join(root, "../skills/annotation-review/SKILL.md")));
    for (const name of ["HOST.md", "open-review.mjs", "LICENSE", "README.md"]) files.set(`${skillDir}/${name}`, files.get(name));
    for (const [path, bytes] of [...files]) if (path.startsWith("common/")) files.set(`${skillDir}/${path}`, bytes);
  }
  const manifestPath = harness === "claude" ? ".claude-plugin/plugin.json" : harness === "codex" ? ".codex-plugin/plugin.json" : "package.json";
  const manifest = JSON.parse(files.get(manifestPath));
  manifest.version = version;
  files.set(manifestPath, jsonBytes(manifest));
  if (harness === "claude") files.set(".claude-plugin/marketplace.json", jsonBytes({
    name: "mark-local", owner: { name: "Mark" },
    plugins: [{ name: manifest.name, source: "./", description: manifest.description }],
  }));
  if (harness === "codex") {
    const portable = { $schema: "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", license: "MIT", repository: "https://github.com/phongndo/mark" };
    for (const key of ["name", "version", "description", "author", "homepage", "repository", "license", "keywords"]) if (manifest[key] !== undefined && manifest[key] !== null) portable[key] = manifest[key];
    files.set("plugin.json", jsonBytes(portable));
    files.set(".agents/plugins/marketplace.json", jsonBytes({
      name: "mark-local", interface: { displayName: "Mark" },
      plugins: [{ name: manifest.name, source: { source: "local", path: "./" }, policy: { installation: "AVAILABLE", authentication: "ON_INSTALL" }, category: "Productivity" }],
    }));
  }
  return new Map([...files].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0));
}

function assertSafePath(path) {
  for (let current = path; ; current = dirname(current)) {
    if (lstatSync(current, { throwIfNoEntry: false })?.isSymbolicLink()) throw new Error(`Output path may not traverse a symlink: ${current}`);
    if (dirname(current) === current) break;
  }
}

export function writePackages(output, version = workspaceVersion()) {
  output = resolve(output);
  assertSafePath(output);
  const packages = harnesses.map(harness => [harness, packageFiles(harness, version)]);
  for (const [harness, expected] of packages) {
    const target = join(output, harness);
    assertSafePath(target);
    if (existsSync(target)) for (const [name, bytes] of readTree(target, "", false)) {
      if (!expected.has(name) || !bytes.equals(expected.get(name))) throw new Error(`Preserving modified generated package: ${join(target, name)}`);
    }
  }
  for (const [harness, files] of packages) for (const [name, bytes] of files) {
    const path = join(output, harness, ...name.split("/"));
    assertSafePath(path);
    mkdirSync(dirname(path), { recursive: true });
    try { writeFileSync(path, bytes, { flag: "wx" }); }
    catch (error) { if (error.code !== "EEXIST" || !readFileSync(path).equals(bytes)) throw error; }
  }
  return output;
}

export function parseArguments(args) {
  let output;
  let version;
  for (let index = 0; index < args.length; index++) {
    if (args[index] === "--version" && version === undefined && args[index + 1] !== undefined) version = validateVersion(args[++index]);
    else if (!args[index].startsWith("-") && output === undefined) output = args[index];
    else throw new Error("Usage: node package.mjs [OUTPUT_DIR] [--version VERSION]");
  }
  return { output: resolve(output || "dist/integrations"), version: version || workspaceVersion() };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const { output, version } = parseArguments(process.argv.slice(2));
    process.stdout.write(`${writePackages(output, version)}\n`);
  } catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
}
