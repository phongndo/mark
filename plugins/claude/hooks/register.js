// Claude's mod API cannot lend its own terminal to a child, so /mark runs the
// real Mark TUI in a split beside Claude (Rex or tmux) and waits for its result.
let active;
const POLL_MS = 500;

async function run($, argv) {
  const response = await $.process.run(argv);
  if (response.exitCode !== 0) throw Error((response.stderr || response.stdout).trim() || `${argv[0]} exited with ${response.exitCode}`);
  return response.stdout.trim();
}

// A failed launch would close the split before anyone reads why; keep it open.
const annotate = (binary, repository, result) => ["/bin/sh", "-c",
  '"$0" "$@" || { status=$?; printf "\\nMark exited with status %s. Press Enter to close." "$status"; read _; }',
  binary, "bridge", "annotate", "--repo", repository, "--result-file", result];

async function split($, command, repository) {
  const rexBlock = await $.env.get("REX_BLOCK");
  if (rexBlock) {
    const created = JSON.parse(await run($, ["rex", "split", "--json", "-b", rexBlock, "-c", repository, "--label", "mark", "--shell", "none", "--", ...command]));
    const block = created.block_ids[0];
    return { alive: async () => (await $.process.run(["rex", "block", "inspect", block])).exitCode === 0 };
  }
  if (await $.env.get("TMUX")) {
    const target = await $.env.get("TMUX_PANE");
    const pane = await run($, ["tmux", "split-window", "-h", ...(target ? ["-t", target] : []), "-c", repository, "-P", "-F", "#{pane_id}", "--", ...command]);
    return { alive: async () => (await $.process.run(["tmux", "display-message", "-p", "-t", pane, "#{pane_id}"])).exitCode === 0 };
  }
  throw Error("/mark opens Mark in a split pane, so run Claude Code inside Rex or tmux. Otherwise run `mark diff` in another terminal.");
}

async function finish($, review) {
  active = undefined;
  review.timer.cancel();
  try {
    const markdown = (await $.fs.exists(review.result)) ? JSON.parse(await $.fs.read(review.result)).markdown : "";
    if (typeof markdown !== "string") throw Error("Mark returned an invalid annotation result.");
    if (!markdown) return;
    if (await $.session.id() !== review.session) throw Error("The conversation changed. Run /mark in the conversation you want to annotate.");
    const draft = await $.prompt.read();
    const filled = await $.prompt.fill({ text: (draft.text.trim() ? "\n\n" : "") + markdown, mode: "append" });
    if (!filled.isFilled) {
      await $.ui.copy({ text: markdown });
      throw Error("The chat input is unavailable; your marks are on the clipboard.");
    }
  } catch (error) { $.ui.toast("Mark: " + error.message); }
  finally { await $.process.run(["rm", "-rf", review.directory]); }
}

export function register(on) {
  on("session.start", async ($, event, next) => {
    await $.command.register({ name: "mark", description: "Annotate the diff in Mark beside this chat and put the notes in the chat input" });
    return next(event);
  });
  on("command.run", { command: "mark" }, async ($) => {
    if (active) return { text: "Mark is already open beside this chat." };
    try {
      const session = await $.session.id(), repository = await $.session.cwd();
      const binary = (await $.env.get("MARK_BIN")) || "mark";
      const directory = await run($, ["mktemp", "-d", "-t", "mark-annotations"]);
      const result = `${directory}/result.json`;
      let pane;
      try { pane = await split($, annotate(binary, repository, result), repository); }
      catch (error) { await $.process.run(["rm", "-rf", directory]); throw error; }
      const review = { session, directory, result, busy: false };
      review.timer = $.clock.every(POLL_MS, async () => {
        if (review.busy || active !== review) return;
        review.busy = true;
        try { if (await $.fs.exists(result) || !await pane.alive()) await finish($, review); }
        finally { review.busy = false; }
      });
      active = review;
      return { text: "Mark opened beside this chat. Q sends all marks here, n picks some, q closes." };
    } catch (error) { return { text: "Mark: " + error.message }; }
  });
}
