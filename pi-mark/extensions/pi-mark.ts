import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { accessSync, constants, statSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, parse, resolve as resolvePath } from "node:path";
import type {
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
  Theme,
} from "@earendil-works/pi-coding-agent";
import { Box, Text } from "@earendil-works/pi-tui";

const INSTALL_HINT =
  "Install mark with:\n" +
  "curl -fsSL https://raw.githubusercontent.com/phongndo/mark/main/scripts/install.sh | sh";
const MARK_ANNOTATIONS_PATH_ENV = "MARK_ANNOTATIONS_PATH";
const ANNOTATION_CARD_TYPE = "pi-mark-annotations";
const ANNOTATION_CONTEXT_TYPE = "pi-mark-annotations-context";
const ANNOTATION_CONSUMED_TYPE = "pi-mark-annotations-consumed";

type NotifyLevel = "info" | "warning" | "error";

type MarkRunResult =
  | { kind: "exited"; status: number; annotations?: MarkAnnotations }
  | { kind: "signaled"; signal: string }
  | { kind: "failed"; error: string };

type MarkAnnotation = {
  path: string;
  old_line?: number;
  new_line?: number;
  body: string;
};

type MarkAnnotations = {
  version: 1;
  marks: MarkAnnotation[];
};

type AnnotationCard = MarkAnnotations & {
  id: string;
  createdAt: number;
};

type ConsumedAnnotations = {
  ids: string[];
};

type MarkCommand = "diff" | "show" | "review" | "patch" | "help";

type MarkInvocation = {
  command: MarkCommand;
  argv: string[];
  cliArgs: string[];
  label: string;
};

type RepoArg = { kind: "absent" } | { kind: "missing-value" } | { kind: "value"; path: string };

export default function piMark(pi: ExtensionAPI) {
  const pendingAnnotations = new Map<string, AnnotationCard>();

  pi.registerEntryRenderer<AnnotationCard>(ANNOTATION_CARD_TYPE, (entry, { expanded }, theme) =>
    renderAnnotationCard(entry.data, expanded, theme),
  );

  const reconstructPendingAnnotations = (ctx: ExtensionContext) => {
    pendingAnnotations.clear();
    for (const entry of ctx.sessionManager.getBranch()) {
      if (entry.type !== "custom") {
        continue;
      }
      if (entry.customType === ANNOTATION_CARD_TYPE && isAnnotationCard(entry.data)) {
        pendingAnnotations.set(entry.data.id, entry.data);
      } else if (
        entry.customType === ANNOTATION_CONSUMED_TYPE &&
        isConsumedAnnotations(entry.data)
      ) {
        for (const id of entry.data.ids) {
          pendingAnnotations.delete(id);
        }
      }
    }
  };

  const takePendingAnnotations = (): AnnotationCard[] => {
    const cards = [...pendingAnnotations.values()];
    if (cards.length > 0) {
      pi.appendEntry<ConsumedAnnotations>(ANNOTATION_CONSUMED_TYPE, {
        ids: cards.map((card) => card.id),
      });
      pendingAnnotations.clear();
    }
    return cards;
  };

  pi.on("session_start", async (_event, ctx) => reconstructPendingAnnotations(ctx));
  pi.on("session_tree", async (_event, ctx) => reconstructPendingAnnotations(ctx));
  pi.on("before_agent_start", async () => {
    const cards = takePendingAnnotations();
    if (cards.length === 0) {
      return;
    }
    return {
      message: annotationContextMessage(cards),
    };
  });

  pi.registerCommand("mark", {
    description: "Open mark diff reviewer (or /mark send|clear)",
    handler: async (args, ctx) => {
      let commandArgs: string[];
      try {
        commandArgs = parseCommandLine(args);
      } catch {
        commandArgs = [];
      }
      if (commandArgs[0] === "clear" || commandArgs[0] === "send") {
        const action = commandArgs[0];
        if (commandArgs.length !== 1) {
          report(ctx, `Usage: /mark ${action}`, "warning");
          return;
        }
        const cards = takePendingAnnotations();
        const count = cards.reduce((total, card) => total + card.marks.length, 0);
        if (count === 0) {
          report(ctx, "No pending mark annotations.", "warning");
          return;
        }
        if (action === "send") {
          pi.sendMessage(annotationContextMessage(cards), {
            triggerTurn: true,
            ...(ctx.isIdle() ? {} : { deliverAs: "followUp" as const }),
          });
          report(ctx, `Sent ${count} mark annotation${count === 1 ? "" : "s"}.`, "info");
        } else {
          report(ctx, `Cleared ${count} pending mark annotation${count === 1 ? "" : "s"}.`, "info");
        }
        return;
      }

      const annotations = await handleMarkCommand(args, ctx);
      if (!annotations) {
        return;
      }

      const card: AnnotationCard = {
        ...annotations,
        id: randomUUID(),
        createdAt: Date.now(),
      };
      pendingAnnotations.set(card.id, card);
      pi.appendEntry<AnnotationCard>(ANNOTATION_CARD_TYPE, card);
      report(
        ctx,
        `${card.marks.length} review annotation${card.marks.length === 1 ? "" : "s"} attached to your next prompt. Run /mark send to send now.`,
        "info",
      );
    },
  });
}

async function handleMarkCommand(
  args: string,
  ctx: ExtensionCommandContext,
): Promise<MarkAnnotations | undefined> {
  if (ctx.mode !== "tui") {
    report(ctx, "/mark requires Pi interactive TUI mode.", "error");
    return;
  }

  let argv: string[];
  try {
    argv = parseCommandLine(args);
  } catch (error) {
    report(ctx, errorMessage(error), "error");
    return;
  }

  const invocation = markInvocation(argv);

  if (stdinPatchRequested(invocation.command, invocation.argv)) {
    report(
      ctx,
      `${stdinPatchSource()} cannot read a patch from stdin inside Pi. Write the patch to a file and run /mark patch <file>.`,
      "error",
    );
    return;
  }

  const mark = markBinary();
  const markError = checkMarkBinary(mark);
  if (markError) {
    report(ctx, markError, "error");
    return;
  }

  if (markInvocationNeedsGit(invocation.command, invocation.argv)) {
    const repoArg = repoPathFromArgs(invocation.argv);
    if (repoArg.kind === "missing-value") {
      report(ctx, `${invocation.label} --repo requires a repository path.`, "error");
      return;
    }

    const gitError = checkGitRepository(
      invocation.label,
      ctx.cwd,
      repoArg.kind === "value" ? repoArg.path : undefined,
    );
    if (gitError) {
      report(ctx, gitError, "error");
      return;
    }
  }

  const result = await runMarkInTerminal(ctx, mark, markInvocationArgs(invocation));
  if (!result) {
    report(ctx, "mark did not return a result.", "error");
    return;
  }

  switch (result.kind) {
    case "failed":
      report(ctx, `Failed to run mark: ${result.error}`, "error");
      return;
    case "signaled":
      report(ctx, `mark terminated by signal ${result.signal}.`, "warning");
      return;
    case "exited":
      if (result.status !== 0) {
        report(ctx, `mark exited with status ${result.status}.`, "error");
        return;
      }
      return result.annotations;
  }
}

function markInvocation(argv: string[]): MarkInvocation {
  const first = argv[0];
  if (
    first === "diff" ||
    first === "show" ||
    first === "review" ||
    first === "patch" ||
    first === "help"
  ) {
    return {
      command: first,
      argv: argv.slice(1),
      cliArgs: argv,
      label: `/mark ${first}`,
    };
  }

  return {
    command: "diff",
    argv,
    cliArgs: argv,
    label: "/mark",
  };
}

function markBinary(): string {
  return process.env.PI_MARK_BIN?.trim() || "mark";
}

function checkMarkBinary(mark: string): string | undefined {
  if (!mark) {
    return `PI_MARK_BIN is empty.\n\n${INSTALL_HINT}`;
  }

  if (!executableAvailable(mark)) {
    return `mark executable was not found (${mark}).\n\n${INSTALL_HINT}`;
  }
}

function executableAvailable(command: string): boolean {
  if (looksLikePath(command)) {
    return executablePathAvailable(command);
  }

  for (const directory of (process.env.PATH ?? "").split(delimiter)) {
    if (executablePathAvailable(join(directory || ".", command))) {
      return true;
    }
  }

  return false;
}

function executablePathAvailable(path: string): boolean {
  return executablePathCandidates(path).some(canExecute);
}

function executablePathCandidates(path: string): string[] {
  if (process.platform !== "win32") {
    return [path];
  }

  const extensions = (process.env.PATHEXT || ".COM;.EXE;.BAT;.CMD").split(";").filter(Boolean);
  const lowerPath = path.toLowerCase();
  if (extensions.some((extension) => lowerPath.endsWith(extension.toLowerCase()))) {
    return [path];
  }

  return [path, ...extensions.map((extension) => `${path}${extension}`)];
}

function canExecute(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function looksLikePath(command: string): boolean {
  return command.includes("/") || command.includes("\\");
}

function checkGitRepository(
  commandLabel: string,
  cwd: string,
  repoPath: string | undefined,
): string | undefined {
  if (hasGitMarker(cwd, repoPath)) {
    return undefined;
  }

  const gitArgs = repoPath
    ? ["-C", repoPath, "rev-parse", "--is-inside-work-tree"]
    : ["rev-parse", "--is-inside-work-tree"];
  const result = spawnSync("git", gitArgs, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });

  if (result.error) {
    return `git is required for ${commandLabel} but was not found: ${errorMessage(result.error)}`;
  }

  if (result.status !== 0 || result.stdout.trim() !== "true") {
    const target = repoPath ? `repository path ${repoPath}` : cwd;
    return (
      `No Git repository found at ${target}.\n\n` +
      "/mark, /mark diff, /mark show, and numeric /mark review targets use Git-backed mark sources unless you run /mark patch <file> or /mark review <full GitHub PR URL>. " +
      "Agent turn diffs are not implemented yet."
    );
  }
}

function hasGitMarker(cwd: string, repoPath: string | undefined): boolean {
  let current = resolvePath(cwd, repoPath ?? ".");
  if (!isDirectory(current)) {
    return false;
  }

  const root = parse(current).root;

  while (true) {
    if (canAccess(join(current, ".git"))) {
      return true;
    }

    if (current === root) {
      break;
    }
    current = dirname(current);
  }

  return false;
}

function isDirectory(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

function canAccess(path: string): boolean {
  try {
    accessSync(path, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

async function runMarkInTerminal(
  ctx: ExtensionCommandContext,
  mark: string,
  argv: string[],
): Promise<MarkRunResult | undefined> {
  let outputDirectory: string;
  try {
    outputDirectory = await mkdtemp(join(tmpdir(), "pi-mark-"));
  } catch (error) {
    return { kind: "failed", error: `could not create annotations output: ${errorMessage(error)}` };
  }

  const annotationsPath = join(outputDirectory, "annotations.json");
  try {
    const result = await ctx.ui.custom<MarkRunResult>(async (tui, _theme, _keybindings, done) => {
      let childResult: MarkRunResult;

      try {
        tui.stop();
        process.stdout.write("\x1b[2J\x1b[H");

        const child = spawn(mark, argv, {
          cwd: ctx.cwd,
          env: { ...process.env, [MARK_ANNOTATIONS_PATH_ENV]: annotationsPath },
          stdio: "inherit",
        });

        childResult = await waitForChild(child);
      } catch (error) {
        childResult = {
          kind: "failed",
          error: errorMessage(error),
        };
      } finally {
        tui.start();
        tui.requestRender(true);
      }

      done(childResult);
      return { render: () => [], invalidate: () => {} };
    });

    if (result?.kind !== "exited" || result.status !== 0) {
      return result;
    }

    try {
      const annotations = await readSubmittedAnnotations(annotationsPath);
      return annotations ? { ...result, annotations } : result;
    } catch (error) {
      return {
        kind: "failed",
        error: `could not read submitted annotations: ${errorMessage(error)}`,
      };
    }
  } finally {
    await rm(outputDirectory, { recursive: true, force: true });
  }
}

function waitForChild(child: ReturnType<typeof spawn>): Promise<MarkRunResult> {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result: MarkRunResult) => {
      if (settled) {
        return;
      }
      settled = true;
      resolve(result);
    };

    child.once("error", (error) => {
      finish({ kind: "failed", error: errorMessage(error) });
    });
    child.once("exit", (status, signal) => {
      if (signal) {
        finish({ kind: "signaled", signal });
      } else if (typeof status === "number") {
        finish({ kind: "exited", status });
      } else {
        finish({ kind: "failed", error: "child exited without status or signal" });
      }
    });
  });
}

async function readSubmittedAnnotations(path: string): Promise<MarkAnnotations | undefined> {
  let contents: string;
  try {
    contents = await readFile(path, "utf8");
  } catch (error) {
    if (isNodeError(error) && error.code === "ENOENT") {
      return undefined;
    }
    throw error;
  }

  if (Buffer.byteLength(contents) > 2 * 1024 * 1024) {
    throw new Error("annotations output exceeds 2 MiB");
  }
  return parseMarkAnnotations(JSON.parse(contents));
}

export function parseMarkAnnotations(value: unknown): MarkAnnotations {
  if (!isRecord(value) || value.version !== 1 || !Array.isArray(value.marks)) {
    throw new Error("expected mark annotations schema version 1");
  }

  const marks = value.marks.map((candidate, index): MarkAnnotation => {
    if (
      !isRecord(candidate) ||
      typeof candidate.path !== "string" ||
      candidate.path.length === 0 ||
      typeof candidate.body !== "string" ||
      !optionalPositiveInteger(candidate.old_line) ||
      !optionalPositiveInteger(candidate.new_line) ||
      (candidate.old_line === undefined && candidate.new_line === undefined)
    ) {
      throw new Error(`invalid annotation at index ${index}`);
    }
    return {
      path: candidate.path,
      ...(candidate.old_line === undefined ? {} : { old_line: candidate.old_line as number }),
      ...(candidate.new_line === undefined ? {} : { new_line: candidate.new_line as number }),
      body: candidate.body,
    };
  });

  if (marks.length === 0) {
    throw new Error("annotations output contains no marks");
  }
  return { version: 1, marks };
}

function annotationContextMessage(cards: AnnotationCard[]) {
  const annotations: MarkAnnotations = {
    version: 1,
    marks: cards.flatMap((card) => card.marks),
  };
  return {
    customType: ANNOTATION_CONTEXT_TYPE,
    content:
      "The user attached review annotations from mark. Treat them as requested code-review changes and address each one.\n\n" +
      JSON.stringify(annotations, null, 2),
    display: false,
    details: annotations,
  };
}

function renderAnnotationCard(card: AnnotationCard | undefined, expanded: boolean, theme: Theme) {
  const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
  if (!card) {
    box.addChild(new Text(theme.fg("warning", "mark annotations unavailable"), 0, 0));
    return box;
  }

  const count = card.marks.length;
  box.addChild(
    new Text(
      theme.fg("toolTitle", theme.bold("mark annotations ")) +
        theme.fg("muted", `${count} review note${count === 1 ? "" : "s"}`),
      0,
      0,
    ),
  );

  const displayed = expanded ? card.marks : card.marks.slice(0, 8);
  for (const mark of displayed) {
    const location = safeDisplayText(annotationLocation(mark));
    const body = safeDisplayText(mark.body)
      .split("\n")
      .map((line) => `  ${line}`)
      .join("\n");
    box.addChild(new Text(`\n${theme.fg("accent", location)}\n${theme.fg("text", body)}`, 0, 0));
  }
  if (displayed.length < count) {
    box.addChild(
      new Text(theme.fg("dim", `\n… ${count - displayed.length} more (Ctrl-O to expand)`), 0, 0),
    );
  }
  box.addChild(new Text(theme.fg("dim", "\nAttached to next prompt · /mark send sends now"), 0, 0));
  return box;
}

function annotationLocation(annotation: MarkAnnotation): string {
  if (annotation.new_line !== undefined) {
    return `${annotation.path}:${annotation.new_line}`;
  }
  return `${annotation.path}:${annotation.old_line} (old)`;
}

function safeDisplayText(text: string): string {
  return [...text]
    .filter((character) => {
      const code = character.codePointAt(0) ?? 0;
      return (
        character === "\n" ||
        character === "\t" ||
        (code >= 32 && code !== 127 && (code < 128 || code > 159))
      );
    })
    .join("");
}

function isAnnotationCard(value: unknown): value is AnnotationCard {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.createdAt !== "number") {
    return false;
  }
  try {
    parseMarkAnnotations(value);
    return true;
  } catch {
    return false;
  }
}

function isConsumedAnnotations(value: unknown): value is ConsumedAnnotations {
  return (
    isRecord(value) && Array.isArray(value.ids) && value.ids.every((id) => typeof id === "string")
  );
}

function optionalPositiveInteger(value: unknown): boolean {
  return value === undefined || (Number.isSafeInteger(value) && (value as number) > 0);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error;
}

export function parseCommandLine(input: string): string[] {
  const args: string[] = [];
  let current = "";
  let quote: "'" | '"' | undefined;
  let escaped = false;
  let tokenStarted = false;

  for (const character of input) {
    if (escaped) {
      current += character;
      escaped = false;
      tokenStarted = true;
      continue;
    }

    if (quote === "'") {
      if (character === "'") {
        quote = undefined;
      } else {
        current += character;
      }
      tokenStarted = true;
      continue;
    }

    if (quote === '"') {
      if (character === '"') {
        quote = undefined;
      } else if (character === "\\") {
        escaped = true;
      } else {
        current += character;
      }
      tokenStarted = true;
      continue;
    }

    if (character === "\\") {
      escaped = true;
      tokenStarted = true;
      continue;
    }

    if (character === "'" || character === '"') {
      quote = character;
      tokenStarted = true;
      continue;
    }

    if (/\s/.test(character)) {
      if (tokenStarted) {
        args.push(current);
        current = "";
        tokenStarted = false;
      }
      continue;
    }

    current += character;
    tokenStarted = true;
  }

  if (escaped) {
    current += "\\";
  }

  if (quote) {
    throw new Error(
      `Unterminated ${quote === "'" ? "single" : "double"} quote in slash command arguments.`,
    );
  }

  if (tokenStarted) {
    args.push(current);
  }

  return args;
}

export function markInvocationNeedsGit(command: MarkCommand, argv: string[]): boolean {
  if (argv.some((arg) => ["--help", "-h"].includes(arg) || isVersionFlag(arg))) {
    return false;
  }

  if (command === "patch") {
    return false;
  }

  if (command === "help") {
    return false;
  }

  if (command === "review") {
    const target = targetFromArgs(argv, 0);
    return target ? !isGitHubPullRequestUrl(target) : false;
  }

  return true;
}

function patchTargetFromArgs(argv: string[]): string | undefined {
  return targetFromArgs(argv, 0);
}

function targetFromArgs(argv: string[], startIndex: number): string | undefined {
  for (let index = startIndex; index < argv.length; index++) {
    const arg = argv[index];
    if (arg === "--") {
      return argv[index + 1];
    }
    if (arg === "--stat" || arg === "-s" || arg === "--no-syntax") {
      continue;
    }
    if (arg === "--repo" || arg === "-r") {
      index++;
      continue;
    }
    if (arg.startsWith("--repo=") || (arg.startsWith("-r") && arg !== "-r")) {
      continue;
    }
    return arg;
  }
  return undefined;
}

function markInvocationArgs(invocation: MarkInvocation): string[] {
  const versionFlag = invocation.cliArgs.find(isVersionFlag);
  if (versionFlag) {
    return [versionFlag];
  }

  return invocation.cliArgs;
}

function stdinPatchSource(): string {
  return "/mark patch";
}

function isVersionFlag(arg: string): boolean {
  return arg === "--version" || arg === "-V";
}

function repoPathFromArgs(argv: string[]): RepoArg {
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    if (arg === "--repo" || arg === "-r") {
      return repoPathValue(argv[index + 1]);
    }
    if (arg?.startsWith("--repo=")) {
      return repoPathValue(arg.slice("--repo=".length));
    }
    if (arg?.startsWith("-r")) {
      const value = arg.slice("-r".length);
      return repoPathValue(value.startsWith("=") ? value.slice("=".length) : value);
    }
  }
  return { kind: "absent" };
}

function repoPathValue(value: string | undefined): RepoArg {
  return value ? { kind: "value", path: value } : { kind: "missing-value" };
}

function stdinPatchRequested(command: MarkCommand, argv: string[]): boolean {
  if (command === "patch") {
    return patchTargetFromArgs(argv) === "-";
  }
  return false;
}

function isGitHubPullRequestUrl(target: string): boolean {
  const value = target.trim();
  const withoutScheme = value.startsWith("https://")
    ? value.slice("https://".length)
    : value.startsWith("http://")
      ? value.slice("http://".length)
      : value;

  const path = withoutScheme.startsWith("github.com/")
    ? withoutScheme.slice("github.com/".length).split(/[?#]/, 1)[0]
    : undefined;
  if (!path) {
    return false;
  }

  const [owner, repo, marker, number] = path.split("/");
  return (
    validGitHubPathSegment(owner) &&
    validGitHubPathSegment(repo) &&
    marker === "pull" &&
    typeof number === "string" &&
    /^[0-9]+$/.test(number) &&
    !/^0+$/.test(number)
  );
}

function validGitHubPathSegment(segment: string | undefined): boolean {
  return typeof segment === "string" && /^[A-Za-z0-9._-]+$/.test(segment);
}

function report(ctx: ExtensionCommandContext, message: string, level: NotifyLevel): void {
  if (ctx.hasUI) {
    ctx.ui.notify(message, level);
    return;
  }

  const prefix = level === "error" ? "error" : level === "warning" ? "warning" : "info";
  console.error(`pi-mark ${prefix}: ${message}`);
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}
