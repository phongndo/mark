#!/usr/bin/env node
// Rendered browser smoke test. Uses Node 24's WebSocket and Chromium's CDP;
// no npm packages, account access, global settings, or permanent browser profile.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, realpath, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const project = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = resolve(process.argv[2] || join(project, "target/debug/mark"));
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
let directory, viewer, browser, cdp;
let checked = 0;
const errors = [];

function success(description) {
  checked++;
  process.stdout.write(`ok ${checked} - ${description}\n`);
}

function command(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", ...options });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${program} ${args.join(" ")}: ${result.stderr || result.stdout}`);
  return result.stdout;
}

async function until(task, label, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = await task();
    if (result) return result;
    await pause(25);
  }
  throw Error(`Timed out waiting for ${label}`);
}

class DevTools {
  #socket;
  #sequence = 0;
  #pending = new Map();
  session;

  static async connect(url) {
    const client = new DevTools();
    client.#socket = new WebSocket(url);
    await once(client.#socket, "open");
    client.#socket.addEventListener("message", event => {
      const message = JSON.parse(event.data);
      if (message.id !== undefined) {
        const pending = client.#pending.get(message.id);
        if (!pending) return;
        client.#pending.delete(message.id);
        clearTimeout(pending.timeout);
        if (message.error) pending.reject(Error(`${pending.method}: ${message.error.message}`));
        else pending.resolve(message.result);
      } else if (message.method === "Runtime.exceptionThrown") {
        const exception = message.params.exceptionDetails;
        errors.push(exception.exception?.description || exception.text);
      }
    });
    client.#socket.addEventListener("close", () => {
      for (const pending of client.#pending.values()) {
        clearTimeout(pending.timeout);
        pending.reject(Error(`Browser connection closed during ${pending.method}`));
      }
      client.#pending.clear();
    });
    return client;
  }

  send(method, params = {}, session = this.session) {
    const id = ++this.#sequence;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.#pending.delete(id);
        reject(Error(`CDP timed out: ${method}`));
      }, 8000);
      this.#pending.set(id, { resolve, reject, timeout, method });
      this.#socket.send(JSON.stringify({ id, method, params, ...(session ? { sessionId: session } : {}) }));
    });
  }

  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) {
      throw Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
    }
    return result.result.value;
  }

  async wait(expression, label) {
    return until(() => this.evaluate(expression), label);
  }

  async click(selector) {
    const position = await this.evaluate(`(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) throw Error('Missing element: ' + ${JSON.stringify(selector)});
      if (element.disabled) throw Error('Disabled element: ' + ${JSON.stringify(selector)});
      element.scrollIntoView({block:'center', inline:'center', behavior:'instant'});
      const rect = element.getBoundingClientRect();
      return {x:rect.x + rect.width/2, y:rect.y + rect.height/2};
    })()`);
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...position });
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...position });
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...position });
  }

  async text(selector, text) {
    await this.evaluate(`(() => { const element = document.querySelector(${JSON.stringify(selector)}); element.focus(); element.select(); })()`);
    await this.send("Input.insertText", { text });
  }

  async clickText(selector, text) {
    const indexed = await this.evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(selector)})).findIndex(element => element.textContent === ${JSON.stringify(text)})`);
    assert.notEqual(indexed, -1, `Missing ${selector} text ${text}`);
    // Assign a test-local marker to locate the rendered control for mouse input.
    await this.evaluate(`document.querySelectorAll(${JSON.stringify(selector)})[${indexed}].setAttribute('data-browser-smoke','click')`);
    try { await this.click('[data-browser-smoke="click"]'); }
    finally { await this.evaluate(`document.querySelector('[data-browser-smoke="click"]')?.removeAttribute('data-browser-smoke')`); }
  }

  close() { this.#socket.close(); }
}

async function stopped(child, timeout) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return true;
  return Promise.race([once(child, "exit").then(() => true), pause(timeout).then(() => false)]);
}

async function cleanup() {
  if (cdp) {
    await Promise.race([cdp.send("Browser.close", {}, undefined).catch(() => {}), pause(1000)]);
    cdp.close(); cdp = undefined;
  }
  for (const child of [browser, viewer]) {
    if (!child || child.exitCode !== null || child.signalCode !== null) continue;
    child.kill("SIGTERM");
    if (!await stopped(child, 1500)) { child.kill("SIGKILL"); await stopped(child, 1500); }
  }
  browser = viewer = undefined;
  if (directory) { await rm(directory, { recursive: true, force: true }); directory = undefined; }
}

async function main() {
  directory = await realpath(await mkdtemp(join(tmpdir(), "mark-browser-smoke-")));
  const repository = join(directory, "repo"), profile = join(directory, "chromium");
  await mkdir(repository); await mkdir(profile);
  const environment = { ...process.env, XDG_STATE_HOME: join(directory, "state"), XDG_CONFIG_HOME: join(directory, "config"), XDG_CACHE_HOME: join(directory, "cache"), GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" };
  for (const key of ["MARK_HARNESS", "MARK_CONVERSATION_ID", "CODEX_THREAD_ID", "CLAUDE_SESSION_ID", "PI_SESSION_ID", "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"]) delete environment[key];
  const git = args => command("git", args, { cwd: repository, env: environment });
  const bridge = (method, params) => {
    const envelope = JSON.parse(command(binary, ["bridge", "call", method, "--stdin", "--json"], { cwd: repository, env: environment, input: JSON.stringify(params) }));
    assert.equal(envelope.ok, true, JSON.stringify(envelope));
    return envelope.result;
  };
  git(["init", "--quiet"]);
  const special = 'é special\t"file.txt';
  for (const [file, text] of [["a-first.txt", "keep first\nremove first\nlast first\n"], ["b-second.txt", "before second\n"], ["deleted file.txt", "deleted contents\n"], [special, "prior special\n"]]) {
    await writeFile(join(repository, file), text);
  }
  git(["add", "."]);
  git(["-c", "user.name=Mark browser tests", "-c", "user.email=browser-test@example.invalid", "commit", "--quiet", "-m", "base"]);
  await writeFile(join(repository, "a-first.txt"), "keep first\nlast first\nadded first\n");
  await writeFile(join(repository, "b-second.txt"), "after second\n");
  await writeFile(join(repository, special), "next special\n");
  await unlink(join(repository, "deleted file.txt"));
  const review = bridge("review.open", { harness: "codex", conversation: "browser-smoke", repository, source: { kind: "worktree" } });
  const reviewId = review.review_id;
  const get = () => bridge("review.get", { review_id: reviewId });
  const readyFile = join(directory, "viewer-ready.json");
  viewer = spawn(binary, ["bridge", "web-serve", reviewId, "--ready-file", readyFile], { cwd: repository, env: environment, stdio: ["ignore", "ignore", "pipe"] });
  let viewerErrors = "";
  viewer.stderr.on("data", bytes => { viewerErrors = (viewerErrors + bytes).slice(-8000); });
  const ready = await until(async () => {
    assert.equal(viewer.exitCode, null, `Viewer exited: ${viewerErrors}`);
    try { return JSON.parse(await readFile(readyFile, "utf8")); } catch { return false; }
  }, "local review viewer startup");
  assert(!ready.error, ready.error);
  browser = spawn(process.env.MARK_BROWSER || "chromium", [
    "--headless=new", "--no-sandbox", "--disable-dev-shm-usage", "--no-first-run", "--no-default-browser-check",
    "--disable-background-networking", "--disable-component-update", "--disable-sync", "--metrics-recording-only",
    "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--window-size=1440,1000", "about:blank",
  ], { env: environment, stdio: ["ignore", "ignore", "pipe"] });
  let browserErrors = "";
  browser.stderr.on("data", bytes => { browserErrors = (browserErrors + bytes).slice(-8000); });
  browser.on("error", error => errors.push(error.message));
  const debug = await until(async () => {
    assert.equal(browser.exitCode, null, `Chromium exited: ${browserErrors}`);
    try { return (await readFile(join(profile, "DevToolsActivePort"), "utf8")).trim().split("\n"); } catch { return false; }
  }, "Chromium DevTools endpoint");
  cdp = await DevTools.connect(`ws://127.0.0.1:${debug[0]}${debug[1]}`);
  const target = await cdp.send("Target.createTarget", { url: "about:blank" }, undefined);
  const attached = await cdp.send("Target.attachToTarget", { targetId: target.targetId, flatten: true }, undefined);
  cdp.session = attached.sessionId;
  await cdp.send("Page.enable"); await cdp.send("Runtime.enable");
  await cdp.send("Page.navigate", { url: ready.url });
  await cdp.wait("document.querySelectorAll('section.file').length === 4", "four rendered diff files");
  assert.equal(await cdp.evaluate("document.title"), "Mark review");
  assert.equal(await cdp.evaluate("document.querySelector('#send').disabled"), true);
  success("real Chromium renders all diff files with Send disabled before human feedback");

  const selectHeader = async file => {
    await cdp.clickText("section.file h2", file);
    assert.equal(await cdp.evaluate("document.querySelector('#target').textContent"), `${file} · file`);
  };
  for (const file of ["a-first.txt", "b-second.txt", special]) await selectHeader(file);
  success("each file header selects its own anchor, including quoted Unicode paths");
  await cdp.text("#body", "first human note");
  await cdp.click("#save");
  await cdp.wait("document.querySelector('#notice').textContent === 'Note saved. Send feedback when you are ready.'", "saved human note");
  let saved = get();
  assert.equal(saved.threads.length, 1);
  assert.equal(saved.threads[0].anchor.file, special);
  assert.equal(saved.threads[0].messages[0].origin, "human");
  assert.equal(await cdp.evaluate("document.querySelector('#send').disabled"), false);
  success("saving through the rendered form persists a human note on the selected file");

  await cdp.clickText(".thread button", "Edit");
  assert.equal(await cdp.evaluate("document.querySelector('#body').value"), "first human note");
  await cdp.text("#body", "edited human note");
  await cdp.click("#save");
  await cdp.wait("Array.from(document.querySelectorAll('.message p')).some(e => e.textContent === 'edited human note')", "edited note text");
  assert.equal(get().threads[0].messages[0].body, "edited human note");
  success("editing updates the existing human message");
  await cdp.clickText(".thread button", "Remove");
  await cdp.wait("document.querySelectorAll('.thread').length === 0", "removed note");
  assert.equal(get().threads[0].messages[0].deleted, true);
  assert.equal(await cdp.evaluate("document.querySelector('#send').disabled"), true);
  success("removing a note preserves a tombstone and disables Send");

  await cdp.evaluate(`(() => {
    const section = Array.from(document.querySelectorAll('section.file')).find(e => e.querySelector('h2').textContent === 'deleted file.txt');
    section.querySelector('.line.del').setAttribute('data-browser-smoke','deleted-line');
  })()`);
  await cdp.click('[data-browser-smoke="deleted-line"]');
  assert.equal(await cdp.evaluate("document.querySelector('#target').textContent"), "deleted file.txt · old line 1");
  await cdp.text("#body", "Please restore this deleted line");
  await cdp.click("#save");
  await cdp.wait("Array.from(document.querySelectorAll('.message p')).some(e => e.textContent === 'Please restore this deleted line')", "deleted-line note");
  saved = get();
  assert.equal(saved.threads[1].anchor.file, "deleted file.txt");
  assert.equal(saved.threads[1].anchor.old_line, 1);
  assert.equal(saved.threads[1].anchor.new_line, undefined);
  success("a deleted line records an old-side anchor in its own deleted file");

  await selectHeader("b-second.txt");
  await cdp.text("#body", "Keep this note as an unsent draft");
  await cdp.click("#save");
  await cdp.wait("Array.from(document.querySelectorAll('.message p')).some(e => e.textContent === 'Keep this note as an unsent draft')", "second saved draft");
  assert.equal(await cdp.evaluate("document.querySelectorAll('.thread input[type=checkbox]:checked').length"), 2);
  while (await cdp.evaluate("document.querySelectorAll('.thread input[type=checkbox]:checked').length")) {
    await cdp.click('.thread input[type=checkbox]:checked');
  }
  assert.equal(await cdp.evaluate("document.querySelector('#send').disabled"), true);
  await cdp.evaluate("document.querySelector('#send').click()");
  assert.equal(get().submissions.length, 0);
  success("deselecting every saved thread disables Send and submits no feedback");

  await cdp.evaluate(`(() => {
    const thread = Array.from(document.querySelectorAll('.thread')).find(e => e.textContent.includes('Please restore this deleted line'));
    thread.querySelector('input[type=checkbox]').setAttribute('data-browser-smoke','selected-feedback');
  })()`);
  await cdp.click('[data-browser-smoke="selected-feedback"]');
  assert.equal(await cdp.evaluate("document.querySelectorAll('.thread input[type=checkbox]:checked').length"), 1);
  assert.equal(await cdp.evaluate("document.querySelector('#send').disabled"), false);
  await cdp.click("#send");
  await cdp.wait("document.querySelector('#notice').textContent === 'Feedback sent. Return to your conversation to continue.'", "submitted feedback");
  const feedback = bridge("feedback.get", { review_id: reviewId });
  assert.equal(feedback.submissions.length, 1);
  assert.equal(feedback.submissions[0].threads.length, 1);
  assert.equal(feedback.submissions[0].threads[0].messages[0].body, "Please restore this deleted line");
  const draft = get().threads.find(thread => thread.messages.some(message => message.body === "Keep this note as an unsent draft"));
  assert(draft);
  assert(!feedback.submissions[0].thread_ids.includes(draft.thread_id));
  assert.equal(feedback.last_return.outcome, "submitted");
  assert.equal(await cdp.evaluate("document.querySelector('#send').disabled"), true);
  success("Send submits only the chosen thread, preserves the other draft, records the return outcome, and disables itself");

  await cdp.click("#back");
  await cdp.wait("document.querySelector('#notice').textContent.includes('No feedback was submitted.')", "Back return");
  const back = get();
  assert.equal(back.last_return.outcome, "back");
  assert.equal(back.submissions.length, 1);
  success("Back records a distinct return without inventing another submitted batch");

  await writeFile(join(repository, "b-second.txt"), "external agent revision\n");
  const refreshed = bridge("review.refresh", { review_id: reviewId, generation: back.generation, client_mutation_id: "external-refresh" });
  assert.equal(refreshed.generation, back.generation + 1);
  await cdp.click("#refresh");
  await cdp.wait(`document.querySelector('#scope').textContent.endsWith('pass ${refreshed.generation}')`, "recovered generation after external refresh");
  assert(await cdp.evaluate("document.querySelector('#diff').textContent.includes('external agent revision')"));
  assert.equal(await cdp.evaluate("document.querySelector('#notice').textContent"), "Review refreshed. Verify any stale anchors before sending feedback.");
  success("Refresh recovers from an external generation change and renders the new snapshot");

  assert.deepEqual(errors, [], `Unexpected browser exceptions: ${errors.join("\n")}`);
  if (process.env.MARK_BROWSER_SCREENSHOT) {
    const screenshot = await cdp.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    await writeFile(process.env.MARK_BROWSER_SCREENSHOT, Buffer.from(screenshot.data, "base64"));
  }
  process.stdout.write(`Browser review smoke passed (${checked} rendered behavior checks).\n`);
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, async () => { await cleanup(); process.exit(1); });
}
try { await main(); }
catch (error) { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; }
finally { await cleanup(); }
