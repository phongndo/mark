import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// The host releases its terminal before calling this blocking handoff.
// No review IDs, history, or agent instructions cross the boundary.
export function createReview({ binary = process.env.MARK_BIN || "mark", spawn = spawnSync } = {}) {
  return function review({ repository = process.cwd() } = {}) {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "mark-annotations-")));
    const result = join(directory, "result.json");
    try {
      const child = spawn(binary, ["bridge", "annotate", "--repo", repository, "--result-file", result], { stdio: "inherit" });
      if (child.error || child.status !== 0) throw new Error(`Cannot use Mark (${binary}): ${child.error?.message || child.signal || child.status}. Build or update Mark with terminal annotation support; set MARK_BIN to use a local build.`);
      const value = JSON.parse(readFileSync(result, "utf8"));
      if (typeof value.markdown !== "string") throw new Error("Mark returned an invalid annotation result.");
      return value.markdown;
    } finally { rmSync(directory, { recursive: true, force: true }); }
  };
}

// OpenCode exposes clipboard paste, but no public editor setter.
export function copyAnnotations(text, { platform = process.platform, env = process.env, spawn = spawnSync } = {}) {
  const [binary, args] = platform === "darwin" ? ["pbcopy", []]
    : env.WAYLAND_DISPLAY ? ["wl-copy", []] : ["xclip", ["-selection", "clipboard"]];
  const result = spawn(binary, args, { input: text, encoding: "utf8" });
  if (result.error || result.status !== 0) throw new Error(`Could not copy annotations: ${result.error?.message || result.stderr || result.status}`);
}

export function run(args = process.argv.slice(2)) {
  if (args.length && (args.length !== 2 || args[0] !== "--repo" || !args[1])) throw new Error("Usage: node open-review.mjs [--repo PATH]");
  const markdown = createReview()({ repository: args[1] || process.cwd() });
  if (markdown) process.stdout.write(markdown + "\n");
}
