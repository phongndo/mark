const $ = id => document.getElementById(id);
let record, anchor, editing, submitted, busy = false;
const selected = new Set(), pending = new Map();
const endpoint = new URL('api', location.href), recordURL = new URL('api/review', location.href);

function node(tag, text, cls) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  if (cls) element.className = cls;
  return element;
}
function notice(text) { $('notice').textContent = text; }
async function api(method, params = {}) {
  const key = JSON.stringify({ method, params });
  if (!pending.has(key)) pending.set(key, {
    method, params: { ...params, generation: record.generation, client_mutation_id: crypto.randomUUID() },
  });
  const response = await fetch(endpoint, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(pending.get(key)),
  });
  const data = await response.json();
  if (!data.ok) {
    pending.delete(key);
    throw Error(data.error.message);
  }
  pending.delete(key);
  return data.result;
}
async function read() {
  const response = await fetch(recordURL);
  const data = await response.json();
  if (!data.ok) throw Error(data.error.message);
  return data.result;
}
async function load() {
  record = await read();
  $('scope').textContent = `${record.binding.repository} · pass ${record.generation}`;
  renderDiff(); renderThreads();
}
function resetCompose() {
  anchor = editing = undefined;
  $('body').value = ''; $('target').textContent = 'Select a file or line in the diff.';
  $('save').disabled = true; $('composeTitle').textContent = 'Add a note'; $('cancelEdit').hidden = true;
}
function target(value, message) {
  anchor = value; editing = message;
  $('target').textContent = value.file + (value.old_line ? ' · old line ' + value.old_line : value.new_line ? ' · line ' + value.new_line : ' · file');
  $('save').disabled = false; $('composeTitle').textContent = message ? 'Edit your note' : 'Add a note';
  $('cancelEdit').hidden = !message;
  if (message) $('body').value = message.body;
  document.querySelectorAll('.selected').forEach(element => element.classList.remove('selected'));
  $('body').focus();
}
function chooseLine(element, value) {
  element.dataset.anchor = 'true'; element.tabIndex = 0;
  const select = () => { target(value); element.classList.add('selected'); };
  element.onclick = select;
  element.onkeydown = event => { if (event.key === 'Enter') select(); };
}
function renderDiff() {
  const root = $('diff'), navigation = $('files');
  root.replaceChildren(); navigation.replaceChildren();
  for (const [index, file] of record.files.entries()) {
    const section = node('section', undefined, 'file'); section.id = 'file-' + index;
    const title = node('h2', file.path + (file.binary ? ' · binary' : ''));
    title.tabIndex = 0;
    const fileAnchor = { file: file.new_path || file.old_path || file.path, scope: 'file' };
    title.onclick = () => target(fileAnchor);
    title.onkeydown = event => { if (event.key === 'Enter') target(fileAnchor); };
    const nav = node('button', file.path);
    nav.onclick = () => section.scrollIntoView({ behavior: 'smooth' });
    navigation.append(nav); section.append(title);
    const patch = node('div', undefined, 'patch'); section.append(patch);
    for (const hunk of file.hunks) {
      patch.append(node('div', hunk.header, 'line hunk'));
      for (const item of hunk.lines) {
        const line = node('div', undefined, 'line ' + item.kind);
        line.append(node('span', item.old_line || '', 'gutter'), node('span', item.new_line || '', 'gutter'));
        const prefix = item.kind === 'add' ? '+' : item.kind === 'del' ? '-' : item.kind === 'context' ? ' ' : '';
        line.append(node('span', prefix + item.text, 'code'));
        if (item.new_line) chooseLine(line, { file: file.new_path || file.path, new_line: item.new_line });
        else if (item.old_line) chooseLine(line, { file: file.old_path || file.path, old_line: item.old_line });
        patch.append(line);
      }
    }
    if (!file.hunks.length) patch.append(node('p', file.binary ? 'Binary contents changed.' : file.status, 'empty'));
    root.append(section);
  }
  if (!record.files.length) root.append(node('p', 'No changes in this review.', 'empty'));
}
function unsent(thread) {
  return thread.messages.some(message => message.origin === 'human' && !message.deleted &&
    !record.submissions.some(batch => batch.threads.some(old => old.messages.some(sent =>
      sent.message_id === message.message_id && sent.revision === message.revision))));
}
function selectedThreads() {
  return record.threads.filter(thread => selected.has(thread.thread_id) && unsent(thread)).map(thread => thread.thread_id);
}
function updateSend() {
  $('send').disabled = !submitted && !selectedThreads().length;
}
function renderThreads() {
  const root = $('threads'); root.replaceChildren();
  for (const id of selected) {
    if (!record.threads.some(thread => thread.thread_id === id && unsent(thread))) selected.delete(id);
  }
  for (const thread of record.threads) {
    const messages = thread.messages.filter(message => !message.deleted);
    if (!messages.length) continue;
    const container = node('div', undefined, 'thread'), label = node('label'), check = node('input');
    check.type = 'checkbox'; check.checked = selected.has(thread.thread_id);
    check.disabled = !unsent(thread);
    check.onchange = () => {
      if (check.checked) selected.add(thread.thread_id);
      else selected.delete(thread.thread_id);
      updateSend();
    };
    label.append(check, node('strong', thread.anchor.file)); container.append(label);
    if (thread.stale) container.append(node('small', 'Anchor needs verification against this pass.', 'stale'));
    for (const message of messages) {
      const block = node('div', undefined, 'message');
      block.append(node('p', message.body));
      if (message.origin === 'human') {
        const edit = node('button', 'Edit'); edit.onclick = () => target(thread.anchor, message); block.append(edit);
        const remove = node('button', 'Remove');
        remove.onclick = () => action(async () => { await api('comment.remove', { message_id: message.message_id }); await load(); });
        block.append(remove);
      }
      container.append(block);
    }
    root.append(container);
  }
  updateSend();
}
async function action(task) {
  if (busy) return; busy = true;
  try { await task(); } catch (error) { notice(error.message); } finally { busy = false; }
}
$('compose').onsubmit = event => {
  event.preventDefault(); action(async () => {
    const body = $('body').value.trim(); if (!anchor || !body) return;
    const method = editing ? 'comment.edit' : 'comment.add';
    const params = editing ? { message_id: editing.message_id, body } : { anchor, body };
    const result = await api(method, params);
    const id = result.thread?.thread_id; if (id) selected.add(id);
    resetCompose(); await load(); notice('Note saved. Send feedback when you are ready.');
  });
};
$('cancelEdit').onclick = resetCompose;
$('refresh').onclick = () => action(async () => {
  if ($('body').value.trim()) throw Error('Save your current note before refreshing the review.');
  record = await read();
  await api('review.refresh'); resetCompose(); await load();
  notice('Review refreshed. Verify any stale anchors before sending feedback.');
});
$('send').onclick = () => action(async () => {
  if ($('body').value.trim()) throw Error('Save your current note before sending feedback.');
  if (!submitted) {
    const threadIds = selectedThreads();
    if (!threadIds.length) throw Error('Select at least one saved thread before sending feedback.');
    const result = await api('feedback.submit', { thread_ids: threadIds });
    submitted = result.submission || result;
  }
  await api('review.finish', { outcome: 'submitted', submission_ids: [submitted.submission_id] });
  submitted = undefined; selected.clear(); await load();
  notice('Feedback sent. Return to your conversation to continue.');
});
$('back').onclick = () => action(async () => {
  if ($('body').value.trim()) throw Error('Save your current note before returning.');
  await api('review.finish', { outcome: 'back' }); notice('Review saved. Return to your conversation. No feedback was submitted.');
});
load().catch(error => notice(error.message));
setInterval(async () => {
  if (!record || busy || document.hidden) return;
  try {
    const next = await read();
    if (next.generation !== record.generation) {
      notice('The review snapshot changed. Refresh after saving your current note.'); return;
    }
    if (next.revision !== record.revision) { record = next; renderThreads(); }
  } catch (_) { /* A transient poll failure must not erase a draft. Explicit actions report failures. */ }
}, 1500);
window.addEventListener('beforeunload', event => {
  if ($('body').value.trim()) { event.preventDefault(); event.returnValue = ''; }
});
