import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { discoverAndLoadExtensions } from "@earendil-works/pi-coding-agent";
import extension, {
  markArgumentCompletions,
  markInvocationNeedsGit,
  parseCommandLine,
  parseMarkAnnotations,
} from "../extensions/pi-mark.ts";

const packageRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const testTheme = {
  bg: (_color, text) => text,
  fg: (_color, text) => text,
  bold: (text) => text,
};

function loadExtension(overrides = {}) {
  extension({
    registerCommand() {},
    registerEntryRenderer() {},
    on() {},
    appendEntry() {},
    sendMessage() {},
    ...overrides,
  });
}

test("extension registers mark source commands", () => {
  const registered = [];
  loadExtension({
    registerCommand(name, options) {
      registered.push({
        name,
        description: options.description,
        completions: options.getArgumentCompletions("").map(({ value }) => value),
      });
    },
  });

  assert.deepEqual(registered, [
    {
      name: "mark",
      description: "Open mark diff reviewer (or /mark send|clear)",
      completions: ["diff", "show", "review", "patch", "send", "clear", "help"],
    },
  ]);
});

test("mark command autocompletes every supported action", () => {
  assert.deepEqual(
    markArgumentCompletions("").map(({ value }) => value),
    ["diff", "show", "review", "patch", "send", "clear", "help"],
  );
  assert.deepEqual(
    markArgumentCompletions("  p").map(({ value }) => value),
    ["patch"],
  );
  assert.deepEqual(
    markArgumentCompletions("help r").map(({ value }) => value),
    ["help review"],
  );
  assert.equal(markArgumentCompletions("diff "), null);
  assert.equal(markArgumentCompletions("unknown"), null);
});

test("package manifest loads mark source commands", async () => {
  const agentDir = await mkdtemp(join(tmpdir(), "pi-mark-test-"));

  try {
    const result = await discoverAndLoadExtensions([packageRoot], packageRoot, agentDir);
    assert.deepEqual(result.errors, []);

    const loaded = result.extensions.find((loadedExtension) =>
      loadedExtension.commands.has("mark"),
    );
    assert.ok(loaded, "expected package manifest to load mark source commands");
  } finally {
    await rm(agentDir, { recursive: true, force: true });
  }
});

test("parseCommandLine splits whitespace", () => {
  assert.deepEqual(parseCommandLine("--no-untracked --base main"), [
    "--no-untracked",
    "--base",
    "main",
  ]);
});

test("parseCommandLine preserves quoted arguments", () => {
  assert.deepEqual(parseCommandLine('review "changes with spaces"'), [
    "review",
    "changes with spaces",
  ]);
});

test("parseCommandLine rejects unterminated quotes", () => {
  assert.throws(() => parseCommandLine('review "missing'), /Unterminated double quote/);
});

test("parseMarkAnnotations validates structured mark output", () => {
  assert.deepEqual(
    parseMarkAnnotations({
      version: 1,
      marks: [{ path: "src/app.ts", new_line: 12, body: "Handle this edge case" }],
    }),
    {
      version: 1,
      marks: [{ path: "src/app.ts", new_line: 12, body: "Handle this edge case" }],
    },
  );
  assert.throws(
    () => parseMarkAnnotations({ version: 1, marks: [{ path: "src/app.ts", body: "missing" }] }),
    /invalid annotation/,
  );
});

test("cleared annotation cards stay hidden after session restore", async () => {
  let entryRenderer;
  let sessionStart;
  loadExtension({
    registerEntryRenderer(_customType, renderer) {
      entryRenderer = renderer;
    },
    on(name, handler) {
      if (name === "session_start") sessionStart = handler;
    },
  });

  const card = {
    id: "card-1",
    createdAt: 1,
    version: 1,
    marks: [{ path: "src/app.ts", new_line: 12, body: "Handle this edge case" }],
  };
  await sessionStart(
    {},
    {
      sessionManager: {
        getBranch: () => [
          { type: "custom", customType: "pi-mark-annotations", data: card },
          {
            type: "custom",
            customType: "pi-mark-annotations-consumed",
            data: { ids: [card.id], hideCards: true },
          },
        ],
      },
    },
  );

  assert.equal(entryRenderer({ data: card }, { expanded: false }, testTheme), undefined);
});

test("markInvocationNeedsGit allows patch files", () => {
  assert.equal(markInvocationNeedsGit("patch", ["changes.diff"]), false);
  assert.equal(markInvocationNeedsGit("patch", ["--stat", "changes.diff"]), false);
});

test("markInvocationNeedsGit allows full GitHub pull request URLs", () => {
  assert.equal(markInvocationNeedsGit("review", ["https://github.com/owner/repo/pull/123"]), false);
  assert.equal(
    markInvocationNeedsGit("review", ["https://github.com/owner/repo/pull/123/"]),
    false,
  );
  assert.equal(
    markInvocationNeedsGit("review", ["https://github.com/owner/repo/pull/123/files"]),
    false,
  );
  assert.equal(
    markInvocationNeedsGit("review", ["https://github.com/owner/repo/pull/123/files?diff=split"]),
    false,
  );
  assert.equal(
    markInvocationNeedsGit("review", ["--stat", "https://github.com/owner/repo/pull/123"]),
    false,
  );
  assert.equal(
    markInvocationNeedsGit("review", [
      "--repo",
      "/tmp/not-a-repo",
      "https://github.com/owner/repo/pull/123",
    ]),
    false,
  );
});

test("markInvocationNeedsGit requires git for diffs, revisions, and review numbers", () => {
  assert.equal(markInvocationNeedsGit("diff", []), true);
  assert.equal(markInvocationNeedsGit("diff", ["--no-untracked"]), true);
  assert.equal(markInvocationNeedsGit("show", []), true);
  assert.equal(markInvocationNeedsGit("show", ["HEAD~1"]), true);
  assert.equal(markInvocationNeedsGit("review", ["123"]), true);
});

test("mark command rejects stdin patch sources before preflight", async () => {
  let handler;
  loadExtension({
    registerCommand(name, options) {
      if (name === "mark") {
        handler = options.handler;
      }
    },
  });

  const notifications = [];
  let customCalled = false;

  await handler("patch -", {
    mode: "tui",
    cwd: packageRoot,
    hasUI: true,
    ui: {
      notify(message, level) {
        notifications.push({ message, level });
      },
      async custom() {
        customCalled = true;
      },
    },
    async waitForIdle() {
      throw new Error("waitForIdle should not be called");
    },
  });

  assert.equal(customCalled, false);
  assert.deepEqual(notifications, [
    {
      message:
        "/mark patch cannot read a patch from stdin inside Pi. Write the patch to a file and run /mark patch <file>.",
      level: "error",
    },
  ]);
});

test("Shift-Q output is rendered as a card and attached to the next prompt", async () => {
  const tempDir = await mkdtemp(join(tmpdir(), "pi-mark-test-"));
  const markPath = join(tempDir, "mark");
  const oldPiMarkBin = process.env.PI_MARK_BIN;

  try {
    await writeFile(
      markPath,
      `#!/usr/bin/env node
import { writeFileSync } from "node:fs";
writeFileSync(process.env.MARK_ANNOTATIONS_PATH, JSON.stringify({
  version: 1,
  marks: [{ path: "src/app.ts", new_line: 12, body: "Handle this edge case" }],
}));
process.exit(0);
`,
    );
    await chmod(markPath, 0o755);
    process.env.PI_MARK_BIN = markPath;

    const handlers = new Map();
    const events = new Map();
    const entries = [];
    const sentMessages = [];
    let entryRenderer;
    loadExtension({
      registerEntryRenderer(_customType, renderer) {
        entryRenderer = renderer;
      },
      registerCommand(name, options) {
        handlers.set(name, options.handler);
      },
      on(name, handler) {
        events.set(name, handler);
      },
      appendEntry(customType, data) {
        entries.push({ customType, data });
      },
      sendMessage(message, options) {
        sentMessages.push({ message, options });
      },
    });

    const notifications = [];
    const transcriptEntries = [];
    let toolsExpanded = false;
    const mountAnnotationEntry = (data) => {
      let expanded = toolsExpanded;
      let content = entryRenderer({ data }, { expanded }, testTheme);
      const entry = {
        setExpanded(nextExpanded) {
          if (expanded !== nextExpanded) {
            expanded = nextExpanded;
            content = entryRenderer({ data }, { expanded }, testTheme);
          }
        },
        render(width) {
          // Pi's CustomEntryComponent owns this leading spacer whenever its
          // renderer returns a component.
          return content ? ["", ...content.render(width)] : [];
        },
      };
      transcriptEntries.push(entry);
      return entry;
    };
    const commandContext = {
      mode: "tui",
      cwd: tempDir,
      hasUI: true,
      isIdle() {
        return true;
      },
      ui: {
        notify(message, level) {
          notifications.push({ message, level });
        },
        getToolsExpanded() {
          return toolsExpanded;
        },
        setToolsExpanded(expanded) {
          toolsExpanded = expanded;
          for (const entry of transcriptEntries) {
            entry.setExpanded(expanded);
          }
        },
        async custom(render) {
          let result;
          await render(
            { stop() {}, start() {}, requestRender() {} },
            undefined,
            undefined,
            (value) => {
              result = value;
            },
          );
          return result;
        },
      },
    };
    await handlers.get("mark")("patch changes.diff", commandContext);

    assert.equal(entries[0].customType, "pi-mark-annotations");
    assert.deepEqual(entries[0].data.marks, [
      { path: "src/app.ts", new_line: 12, body: "Handle this edge case" },
    ]);
    assert.match(notifications[0].message, /ready.*\/mark send to submit now/);

    const injection = await events.get("before_agent_start")({}, {});
    assert.equal(injection.message.customType, "pi-mark-annotations-context");
    assert.equal(injection.message.display, false);
    assert.match(injection.message.content, /Handle this edge case/);
    assert.equal(entries[1].customType, "pi-mark-annotations-consumed");

    await handlers.get("mark")("patch changes.diff", commandContext);
    assert.equal(entries[2].customType, "pi-mark-annotations");
    const submittedCard = mountAnnotationEntry(entries[2].data);
    assert.match(submittedCard.render(120).join("\n"), /Pending/);

    await handlers.get("mark")("send", commandContext);
    assert.equal(entries[3].customType, "pi-mark-annotations-consumed");
    assert.equal(sentMessages[0].message.customType, "pi-mark-annotations-context");
    assert.deepEqual(sentMessages[0].options, { triggerTurn: true });
    assert.match(submittedCard.render(120).join("\n"), /Submitted to agent/);
    assert.match(notifications.at(-1).message, /Submitted 1 mark annotation/);
    assert.equal(await events.get("before_agent_start")({}, {}), undefined);

    await handlers.get("mark")("patch changes.diff", commandContext);
    assert.equal(entries[4].customType, "pi-mark-annotations");
    const clearedCard = mountAnnotationEntry(entries[4].data);
    assert.ok(clearedCard.render(120).length > 0);

    await handlers.get("mark")("clear", commandContext);
    assert.equal(entries[5].customType, "pi-mark-annotations-consumed");
    assert.equal(entries[5].data.hideCards, true);
    assert.deepEqual(clearedCard.render(120), []);
    assert.match(notifications.at(-1).message, /Cleared 1 pending mark annotation/);
    assert.equal(await events.get("before_agent_start")({}, {}), undefined);
  } finally {
    if (oldPiMarkBin === undefined) {
      delete process.env.PI_MARK_BIN;
    } else {
      process.env.PI_MARK_BIN = oldPiMarkBin;
    }
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("top-level help and version flags run mark without git preflight", async () => {
  const tempDir = await mkdtemp(join(tmpdir(), "pi-mark-test-"));
  const markPath = join(tempDir, "mark");
  const argsPath = join(tempDir, "args.json");
  const oldPiMarkBin = process.env.PI_MARK_BIN;
  const oldArgsPath = process.env.PI_MARK_TEST_ARGS;

  try {
    await writeFile(
      markPath,
      `#!/usr/bin/env node
import { writeFileSync } from "node:fs";
writeFileSync(process.env.PI_MARK_TEST_ARGS, JSON.stringify(process.argv.slice(2)));
process.exit(0);
`,
    );
    await chmod(markPath, 0o755);

    process.env.PI_MARK_BIN = markPath;
    process.env.PI_MARK_TEST_ARGS = argsPath;

    const handlers = new Map();
    loadExtension({
      registerCommand(name, options) {
        handlers.set(name, options.handler);
      },
    });

    for (const [args, expected] of [
      ["help", ["help"]],
      ["help diff", ["help", "diff"]],
      ["--version", ["--version"]],
      ["-V", ["-V"]],
      ["diff --version", ["--version"]],
      ["show -V", ["-V"]],
      ["review --version", ["--version"]],
      ["patch --version", ["--version"]],
    ]) {
      const notifications = [];
      let customCalled = false;
      await writeFile(argsPath, "[]");

      await handlers.get("mark")(args, {
        mode: "tui",
        cwd: tempDir,
        hasUI: true,
        ui: {
          notify(message, level) {
            notifications.push({ message, level });
          },
          async custom(render) {
            customCalled = true;
            let result;
            await render(
              {
                stop() {},
                start() {},
                requestRender() {},
              },
              undefined,
              undefined,
              (value) => {
                result = value;
              },
            );
            return result;
          },
        },
        async waitForIdle() {
          throw new Error("waitForIdle should not be called");
        },
      });

      assert.equal(customCalled, true, `expected /mark ${args} to run mark`);
      assert.deepEqual(notifications, []);
      assert.deepEqual(JSON.parse(await readFile(argsPath, "utf8")), expected);
    }
  } finally {
    if (oldPiMarkBin === undefined) {
      delete process.env.PI_MARK_BIN;
    } else {
      process.env.PI_MARK_BIN = oldPiMarkBin;
    }
    if (oldArgsPath === undefined) {
      delete process.env.PI_MARK_TEST_ARGS;
    } else {
      process.env.PI_MARK_TEST_ARGS = oldArgsPath;
    }
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("mark command forwards default diff and explicit subcommands", async () => {
  const tempDir = await mkdtemp(join(tmpdir(), "pi-mark-test-"));
  const repoDir = join(tempDir, "repo");
  const markPath = join(tempDir, "mark");
  const argsPath = join(tempDir, "args.json");
  const oldPiMarkBin = process.env.PI_MARK_BIN;
  const oldArgsPath = process.env.PI_MARK_TEST_ARGS;

  try {
    await mkdir(repoDir);
    await mkdir(join(repoDir, ".git"));
    await writeFile(
      markPath,
      `#!/usr/bin/env node
import { writeFileSync } from "node:fs";
writeFileSync(process.env.PI_MARK_TEST_ARGS, JSON.stringify(process.argv.slice(2)));
process.exit(0);
`,
    );
    await chmod(markPath, 0o755);

    process.env.PI_MARK_BIN = markPath;
    process.env.PI_MARK_TEST_ARGS = argsPath;

    let handler;
    loadExtension({
      registerCommand(name, options) {
        if (name === "mark") {
          handler = options.handler;
        }
      },
    });

    for (const [args, expected] of [
      ["", []],
      ["--no-untracked", ["--no-untracked"]],
      ["diff --base main", ["diff", "--base", "main"]],
      ["show HEAD~1", ["show", "HEAD~1"]],
      ["review 123", ["review", "123"]],
      ["patch changes.diff", ["patch", "changes.diff"]],
    ]) {
      const notifications = [];
      await writeFile(argsPath, "[]");

      await handler(args, {
        mode: "tui",
        cwd: repoDir,
        hasUI: true,
        ui: {
          notify(message, level) {
            notifications.push({ message, level });
          },
          async custom(render) {
            let result;
            await render(
              {
                stop() {},
                start() {},
                requestRender() {},
              },
              undefined,
              undefined,
              (value) => {
                result = value;
              },
            );
            return result;
          },
        },
      });

      assert.deepEqual(notifications, []);
      assert.deepEqual(JSON.parse(await readFile(argsPath, "utf8")), expected);
    }
  } finally {
    if (oldPiMarkBin === undefined) {
      delete process.env.PI_MARK_BIN;
    } else {
      process.env.PI_MARK_BIN = oldPiMarkBin;
    }
    if (oldArgsPath === undefined) {
      delete process.env.PI_MARK_TEST_ARGS;
    } else {
      process.env.PI_MARK_TEST_ARGS = oldArgsPath;
    }
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("mark command preflight honors attached short repo arguments without waiting for idle", async () => {
  const tempDir = await mkdtemp(join(tmpdir(), "pi-mark-test-"));
  const binDir = join(tempDir, "bin");
  const repoDir = join(tempDir, "repo");
  const outsideDir = join(tempDir, "outside");
  const markPath = join(binDir, "mark");
  const gitPath = join(binDir, "git");
  const oldPiMarkBin = process.env.PI_MARK_BIN;
  const oldPath = process.env.PATH;
  const oldExpectedRepo = process.env.PI_MARK_TEST_EXPECTED_REPO;

  try {
    await mkdir(binDir);
    await mkdir(repoDir);
    await mkdir(outsideDir);
    await writeFile(
      markPath,
      `#!/usr/bin/env node
process.exit(0);
`,
    );
    await writeFile(
      gitPath,
      `#!/usr/bin/env node
const args = process.argv.slice(2);
const expectedRepo = process.env.PI_MARK_TEST_EXPECTED_REPO;
if (
  expectedRepo &&
  args.length === 4 &&
  args[0] === "-C" &&
  args[1] === expectedRepo &&
  args[2] === "rev-parse" &&
  args[3] === "--is-inside-work-tree"
) {
  console.log("true");
  process.exit(0);
}
process.exit(1);
`,
    );
    await chmod(markPath, 0o755);
    await chmod(gitPath, 0o755);

    process.env.PI_MARK_BIN = markPath;
    process.env.PATH = `${binDir}${delimiter}${oldPath ?? ""}`;

    for (const { args, expectedRepo } of [
      { args: "-r../repo", expectedRepo: "../repo" },
      { args: `-r=${repoDir}`, expectedRepo: repoDir },
    ]) {
      process.env.PI_MARK_TEST_EXPECTED_REPO = expectedRepo;
      const notifications = [];
      let customCalled = false;
      let waitForIdleCalled = false;
      let handler;

      loadExtension({
        registerCommand(name, options) {
          if (name === "mark") {
            handler = options.handler;
          }
        },
      });

      await handler(`diff ${args}`, {
        mode: "tui",
        cwd: outsideDir,
        hasUI: true,
        ui: {
          notify(message, level) {
            notifications.push({ message, level });
          },
          async custom(render) {
            customCalled = true;
            let result;
            await render(
              {
                stop() {},
                start() {},
                requestRender() {},
              },
              undefined,
              undefined,
              (value) => {
                result = value;
              },
            );
            return result;
          },
        },
        async waitForIdle() {
          waitForIdleCalled = true;
        },
      });

      assert.equal(
        waitForIdleCalled,
        false,
        `expected /mark diff ${args} to open without waiting for idle`,
      );
      assert.equal(customCalled, true, `expected ${args} to run mark`);
      assert.deepEqual(notifications, []);
    }
  } finally {
    if (oldPiMarkBin === undefined) {
      delete process.env.PI_MARK_BIN;
    } else {
      process.env.PI_MARK_BIN = oldPiMarkBin;
    }
    if (oldPath === undefined) {
      delete process.env.PATH;
    } else {
      process.env.PATH = oldPath;
    }
    if (oldExpectedRepo === undefined) {
      delete process.env.PI_MARK_TEST_EXPECTED_REPO;
    } else {
      process.env.PI_MARK_TEST_EXPECTED_REPO = oldExpectedRepo;
    }
    await rm(tempDir, { recursive: true, force: true });
  }
});

test("mark command uses filesystem git marker fast path", async () => {
  const tempDir = await mkdtemp(join(tmpdir(), "pi-mark-test-"));
  const binDir = join(tempDir, "bin");
  const repoDir = join(tempDir, "repo");
  const outsideDir = join(tempDir, "outside");
  const markPath = join(binDir, "mark");
  const gitPath = join(binDir, "git");
  const oldPiMarkBin = process.env.PI_MARK_BIN;
  const oldPath = process.env.PATH;

  try {
    await mkdir(binDir);
    await mkdir(repoDir);
    await mkdir(join(repoDir, ".git"));
    await mkdir(outsideDir);
    await writeFile(
      markPath,
      `#!/usr/bin/env node
process.exit(0);
`,
    );
    await writeFile(
      gitPath,
      `#!/usr/bin/env node
process.exit(1);
`,
    );
    await chmod(markPath, 0o755);
    await chmod(gitPath, 0o755);

    process.env.PI_MARK_BIN = "mark";
    process.env.PATH = `${binDir}${delimiter}${oldPath ?? ""}`;

    const notifications = [];
    let customCalled = false;
    let handler;

    loadExtension({
      registerCommand(name, options) {
        if (name === "mark") {
          handler = options.handler;
        }
      },
    });

    await handler(`diff --repo=${repoDir}`, {
      mode: "tui",
      cwd: outsideDir,
      hasUI: true,
      ui: {
        notify(message, level) {
          notifications.push({ message, level });
        },
        async custom(render) {
          customCalled = true;
          let result;
          await render(
            {
              stop() {},
              start() {},
              requestRender() {},
            },
            undefined,
            undefined,
            (value) => {
              result = value;
            },
          );
          return result;
        },
      },
      async waitForIdle() {
        throw new Error("waitForIdle should not be called");
      },
    });

    assert.equal(customCalled, true, "expected /mark diff to run mark");
    assert.deepEqual(notifications, []);
  } finally {
    if (oldPiMarkBin === undefined) {
      delete process.env.PI_MARK_BIN;
    } else {
      process.env.PI_MARK_BIN = oldPiMarkBin;
    }
    if (oldPath === undefined) {
      delete process.env.PATH;
    } else {
      process.env.PATH = oldPath;
    }
    await rm(tempDir, { recursive: true, force: true });
  }
});
