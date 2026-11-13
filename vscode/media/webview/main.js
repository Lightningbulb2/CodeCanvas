// App shell: top bar, file explorer, global chat, context history, Explain All, host messages.
import { on, send, request } from './bridge.js';
import { createChat } from './chat.js';
import * as C from './canvas.js';
import { h, esc, icons, extColor, basename, timeStr, plural, toast } from './util.js';

const { S } = C;

// ---------------------------------------------------------------- DOM
document.body.append(
  h('div', { id: 'app' },
    h('div', { id: 'topbar' }),
    h('aside', { id: 'explorer' }),
    h('main', { id: 'viewport', tabindex: '0' }),
    h('aside', { id: 'globalChat' }),
  ),
  h('div', { id: 'ctxMenu' }),
);
const topbar = document.getElementById('topbar');
const explorer = document.getElementById('explorer');
const viewport = document.getElementById('viewport');
const gcPanel = document.getElementById('globalChat');

// ---------------------------------------------------------------- top bar
const wsEl = h('span', { class: 'ws' });
const saveAllBtn = h('button', { class: 'tb-btn', title: 'Save all edited files', html: `${icons.save}<span>Save all</span>`, style: { display: 'none' } });
const explainAllBtn = h('button', { class: 'tb-btn', title: 'Open every file, lay them out, and have the LLM break each into explained snippets', html: `${icons.sparkle}<span>Explain All</span>` });
const zoomLabel = h('span', { id: 'zoomLabel', title: 'Ctrl+0 resets' }, '100%');
const fitBtn = h('button', { class: 'tb-btn', title: 'Fit everything (F)', html: icons.fit });
const modelBtn = h('button', { class: 'tb-btn model-chip', title: 'LLM provider & model' }, h('span', { class: 'dot' }), h('span', { class: 'mname' }, '…'));
const chatToggle = h('button', { class: 'tb-btn active', title: 'Toggle the global chat', html: `${icons.chat}<span>Chat</span>` });
const gearBtn = h('button', { class: 'tb-btn', title: 'CodeCanvas settings', html: icons.gear });
const splitRBtn = h('button', { class: 'tb-btn', title: 'Split the active view right (Ctrl+\\)', html: icons.splitR });
const splitDBtn = h('button', { class: 'tb-btn', title: 'Split the active view down (Ctrl+Shift+\\)', html: icons.splitD });
const sharedChk = h('input', { type: 'checkbox', checked: true });
const sharedLbl = h('label', { class: 'tb-check', title: 'Checked: split views show the same canvas (each with its own pan/zoom). Unchecked: each view is a separate canvas.' }, sharedChk, 'Shared canvas');
const helpBtn = h('button', { class: 'tb-btn', title: 'Show / hide shortcuts' }, '?');
topbar.append(
  h('span', { class: 'brand', html: `${icons.logo}<span>CodeCanvas</span>` }), wsEl,
  h('span', { class: 'spacer' }),
  saveAllBtn, explainAllBtn, zoomLabel, fitBtn, splitRBtn, splitDBtn, sharedLbl, modelBtn, chatToggle, helpBtn, gearBtn,
);
saveAllBtn.onclick = () => { C.saveAll(); updateDirty(); };
fitBtn.onclick = () => C.fitAll();
splitRBtn.onclick = () => C.splitPane(undefined, 'row');
splitDBtn.onclick = () => C.splitPane(undefined, 'col');
sharedChk.addEventListener('change', () => C.setShared(sharedChk.checked));
gearBtn.onclick = () => send({ type: 'openSettings' });
chatToggle.onclick = () => {
  gcPanel.classList.toggle('collapsed');
  chatToggle.classList.toggle('active', !gcPanel.classList.contains('collapsed'));
};

let cfg = { provider: 'ollama', model: '' };
let dropdown = null;
modelBtn.addEventListener('click', e => {
  e.stopPropagation();
  if (dropdown) { closeDropdown(); return; }
  dropdown = h('div', { class: 'dropdown' }, h('div', { class: 'dd-label' }, 'Loading models…'));
  modelBtn.append(dropdown);
  send({ type: 'listModels' });
});
function closeDropdown() { dropdown?.remove(); dropdown = null; }
document.addEventListener('pointerdown', e => { if (dropdown && !modelBtn.contains(e.target)) closeDropdown(); });
on('models', m => {
  if (!dropdown) return;
  dropdown.replaceChildren();
  const item = (label, fn, onState) => {
    const d = h('div', { class: 'dd-item' + (onState ? ' on' : ''), title: label }, label);
    d.addEventListener('click', ev => { ev.stopPropagation(); fn(); closeDropdown(); });
    dropdown.append(d);
  };
  dropdown.append(h('div', { class: 'dd-label' }, `${cfg.provider} models`));
  if (!m.models.length) dropdown.append(h('div', { class: 'dd-item', style: { color: 'var(--muted)' } }, cfg.provider === 'anthropic' ? 'Set an API key to list models' : 'No server reachable at ' + (cfg.endpoint || 'endpoint')));
  for (const name of m.models) item(name, () => send({ type: 'setModel', model: name }), name === m.current);
  dropdown.append(h('div', { class: 'dd-sep' }), h('div', { class: 'dd-label' }, 'Provider'));
  for (const [p, label] of [['ollama', 'Ollama (local)'], ['llamacpp', 'llama.cpp (llama-server)'], ['openai', 'OpenAI-compatible (LM Studio, vLLM, OpenRouter…)'], ['anthropic', 'Anthropic Claude']]) {
    item(label, () => send({ type: 'setProvider', provider: p }), cfg.provider === p);
  }
  dropdown.append(h('div', { class: 'dd-sep' }), h('div', { class: 'dd-label' }, 'Tools'));
  item(cfg.web ? `Web search: ${cfg.web}` : 'Web search: off — set SearXNG URL…', () => send({ type: 'setSearxng' }), !!cfg.web);
  dropdown.append(h('div', { class: 'dd-sep' }));
  item('Set API key…', () => send({ type: 'setApiKey' }));
  item('All settings…', () => send({ type: 'openSettings' }));
});
on('config', m => {
  cfg = m;
  modelBtn.querySelector('.mname').textContent = `${m.model}`;
  modelBtn.title = `${m.provider} · ${m.endpoint}${m.web ? ` · web search via ${m.web}` : ''}`;
  modelBtn.classList.toggle('local', /localhost|127\.0\.0\.1|\[::1\]/.test(m.endpoint));
});

let zoomRaf = 0;
function onView(v) {
  if (zoomRaf) return;
  zoomRaf = requestAnimationFrame(() => { zoomRaf = 0; zoomLabel.textContent = Math.round(v.z * 100) + '%'; });
}

// ---------------------------------------------------------------- explorer
const exFilter = h('input', { class: 'ex-filter', placeholder: 'Filter files…', spellcheck: 'false' });
const tree = h('div', { id: 'tree' });
const refreshBtn = h('button', { class: 'icon-btn', title: 'Refresh', html: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5"/></svg>' });
explorer.append(h('div', { class: 'ex-head' }, h('span', { style: { flex: '1' } }, 'Explorer'), refreshBtn), exFilter, tree);
refreshBtn.onclick = () => send({ type: 'refreshFiles' });
exFilter.addEventListener('input', renderTree);
exFilter.addEventListener('keydown', e => e.stopPropagation());

let files = [];
const expanded = new Set();

function buildTree(paths) {
  const root = { name: '', dirs: new Map(), files: [] };
  for (const p of paths) {
    const parts = p.split('/');
    let n = root;
    for (let i = 0; i < parts.length - 1; i++) {
      if (!n.dirs.has(parts[i])) n.dirs.set(parts[i], { name: parts[i], path: parts.slice(0, i + 1).join('/'), dirs: new Map(), files: [] });
      n = n.dirs.get(parts[i]);
    }
    n.files.push(p);
  }
  return root;
}

function fileRow(p, depth, label) {
  const open = !!C.fileByPath(p);
  const row = h('div', { class: 'tree-row' + (open ? ' open-on-canvas' : ''), draggable: 'true', title: p, style: { paddingLeft: 6 + depth * 12 + 'px' } },
    h('span', { class: 'chev' }), h('span', { class: 'fi', style: { background: extColor(p) } }), h('span', { class: 'name' }, label));
  row.addEventListener('click', () => C.openFile(p));
  row.addEventListener('dragstart', e => { e.dataTransfer.setData('text/cc-path', p); e.dataTransfer.effectAllowed = 'copy'; });
  row.addEventListener('contextmenu', e => {
    e.preventDefault();
    const menu = document.getElementById('ctxMenu');
    menu.replaceChildren(
      ...[['Open on canvas', () => C.openFile(p)], ['Explain file', () => C.explainFile(p)], [`Open in ${window.CC_EDITOR || 'VS Code'}`, () => send({ type: 'openInEditor', path: p })]]
        .map(([l, fn]) => { const mi = h('div', { class: 'mi' }, l); mi.onclick = () => { menu.style.display = 'none'; fn(); }; return mi; }));
    Object.assign(menu.style, { display: 'block', left: e.clientX + 'px', top: e.clientY + 'px' });
  });
  return row;
}

function renderTree() {
  tree.replaceChildren();
  const q = exFilter.value.trim().toLowerCase();
  if (q) {
    for (const p of files.filter(f => f.toLowerCase().includes(q)).slice(0, 400)) tree.append(fileRow(p, 0, p));
    return;
  }
  const walk = (node, depth) => {
    for (const d of [...node.dirs.values()].sort((a, b) => a.name.localeCompare(b.name))) {
      const isOpen = expanded.has(d.path);
      const row = h('div', { class: 'tree-row' + (isOpen ? ' expanded' : ''), style: { paddingLeft: 6 + depth * 12 + 'px' } },
        h('span', { class: 'chev' }, '›'), h('span', { class: 'name' }, d.name));
      row.addEventListener('click', () => { isOpen ? expanded.delete(d.path) : expanded.add(d.path); renderTree(); });
      tree.append(row);
      if (isOpen) walk(d, depth + 1);
    }
    for (const p of node.files.sort((a, b) => basename(a).localeCompare(basename(b)))) tree.append(fileRow(p, depth, basename(p)));
  };
  walk(buildTree(files), 0);
}

// panel toggles live on the viewport edges so they stay reachable when a panel is collapsed
const exToggle = h('button', { class: 'ex-toggle', title: 'Toggle explorer' }, '◀');
const gcToggle = h('button', { class: 'gc-toggle', title: 'Toggle chat' }, '▶');
exToggle.onclick = () => { explorer.classList.toggle('collapsed'); exToggle.textContent = explorer.classList.contains('collapsed') ? '▶' : '◀'; };
gcToggle.onclick = () => chatToggle.click();
new MutationObserver(() => { gcToggle.textContent = gcPanel.classList.contains('collapsed') ? '◀' : '▶'; }).observe(gcPanel, { attributes: true });

// ---------------------------------------------------------------- viewport overlays
const ctxHistory = h('div', { id: 'ctxHistory' });
const progress = h('div', { id: 'progress' }, h('span', { class: 'pg-t' }), h('button', { class: 'tb-btn' }, 'Stop'), h('div', { class: 'bar' }, h('i')));
const emptyState = h('div', { id: 'emptyState', html: '<div><h2>Your canvas is empty</h2>Click a file in the explorer (or drag it here) to place it.<br>Select lines → <b>◆ Snippet</b> · drag a snippet onto a file to merge it<br>Double-click empty space for a new snippet · try <b>Explain All</b></div>' });
const hint = h('div', { id: 'hint', html: '<kbd>Wheel</kbd> scroll · <kbd>Shift</kbd>+wheel sideways · <kbd>Ctrl</kbd>+wheel zoom · middle-drag pan · <kbd>Space</kbd>+drag moves any card · <kbd>Ctrl</kbd>/<kbd>Shift</kbd>+click multi-select · right-click → Merge · double-click to edit' });
const toasts = h('div', { id: 'toasts' });
exToggle.style.left = '0'; exToggle.style.right = 'auto'; exToggle.style.borderLeft = '0'; exToggle.style.borderRadius = '0 6px 6px 0';
gcToggle.style.right = '0'; gcToggle.style.left = 'auto';
viewport.append(ctxHistory, progress, emptyState, hint, toasts, exToggle, gcToggle);

let historyCollapsed = false;
function renderCtxHistory() {
  const n = S.ctx.length;
  const lines = S.ctx.reduce((a, id) => a + C.ctxLines(id), 0);
  ctxHistory.classList.toggle('collapsed', historyCollapsed);
  ctxHistory.replaceChildren();
  const head = h('div', { class: 'ch-head', title: 'Global context — used by the chat on the right and by snippet chats in “Global” mode' },
    h('span', { class: 't' }, 'Context'), h('span', { class: 'sum' }, n ? `${plural(n, 'item')} · ${plural(lines, 'line')}` : 'empty'), h('span', { style: { color: 'var(--muted)' } }, historyCollapsed ? '▾' : '▴'));
  head.onclick = () => { historyCollapsed = !historyCollapsed; renderCtxHistory(); };
  ctxHistory.append(head);
  const chips = h('div', { class: 'chips' });
  for (const id of S.ctx) {
    const rm = h('button', { title: 'Remove from context' }, '×');
    rm.onclick = e => { e.stopPropagation(); C.toggleCtx(id); };
    const chip = h('span', { class: 'chip', title: `${C.ctxLabel(id)} · ${plural(C.ctxLines(id), 'line')}` }, h('span', {}, C.ctxLabel(id)), rm);
    chip.onclick = () => C.focusCard(id);
    chips.append(chip);
  }
  if (!n) chips.append(h('div', { class: 'noctx' }, 'Use ◎ on a file or snippet to add it.'));
  ctxHistory.append(chips);
  const log = h('div', { class: 'log' });
  for (const ev of S.history.slice().reverse()) {
    const row = h('div', { class: `ev ${ev.kind}`, title: new Date(ev.t).toLocaleString() },
      h('span', { class: 's' }, ev.kind === 'add' ? '+' : ev.kind === 'rem' ? '−' : '•'),
      h('span', { class: 'm' }, ev.text),
      ev.lines ? h('span', { class: 'n' }, `${ev.lines} ln`) : null,
      h('span', { class: 'tm' }, timeStr(ev.t)));
    if (ev.ref) row.onclick = () => C.focusCard(ev.ref);
    log.append(row);
  }
  if (!S.history.length) log.append(h('div', { class: 'noctx' }, 'Context changes will be listed here.'));
  log.addEventListener('wheel', e => e.stopPropagation(), { passive: true });
  ctxHistory.append(log);
  ctxHistory.addEventListener('pointerdown', e => e.stopPropagation());
  gcSub.textContent = n ? `${plural(n, 'item')} in context` : 'no context';
}

let hintPinned = false;
helpBtn.onclick = () => { hintPinned = !hintPinned; helpBtn.classList.toggle('active', hintPinned); updateEmpty(); };
function updateEmpty() {
  const empty = !C.allCards().length;
  emptyState.style.display = empty ? 'flex' : 'none';
  hint.style.display = empty || hintPinned ? '' : 'none';
}
function updateDirty() { saveAllBtn.style.display = C.anyDirty() ? '' : 'none'; }

// ---------------------------------------------------------------- global chat (unfocused)
const gcSub = h('span', { class: 'sub' });
const clearBtn = h('button', { class: 'icon-btn', title: 'Reset this chat', html: icons.reset });
const gchat = createChat({
  messages: () => S.globalChat,
  context: () => ({ items: C.globalContextItems() }),
  onSnippets: snips => C.spawnDrafts(snips, null),
  onCodeToSnippet: (code, info, attrs) => C.spawnDrafts([{ title: attrs.title || 'From chat', code, file: attrs.file, insertAt: +attrs.insert || undefined }], null),
  onFocusSnippet: id => C.focusCard(id),
  onChange: () => C.scheduleSave(),
  placeholder: 'Ask about the project…',
  empty: 'General chat — not tied to any snippet. It sees the <b>global context</b> (top right of the canvas) and can read any project file on its own.<br><br>Ask it to draft code and the snippets will appear on the canvas, ready to drag into files.',
});
clearBtn.onclick = () => { S.globalChat.length = 0; gchat.render(); C.scheduleSave(); };
gcPanel.append(h('div', { class: 'gc-head' }, h('span', { class: 't' }, 'Chat'), gcSub, clearBtn), gchat.el);

// ---------------------------------------------------------------- Explain All
let explainRun = null;
explainAllBtn.onclick = () => (explainRun ? stopExplainAll() : explainAll());
progress.querySelector('button').onclick = () => stopExplainAll();

async function explainAll() {
  if (explainRun) return;
  let res;
  try { res = await request({ type: 'listAll' }, 30000); } catch { toast('Could not list project files', 'error'); return; }
  if (!res.files.length) { toast('No files to explain.'); return; }
  if (res.total > res.files.length) toast(`Explaining the first ${res.files.length} of ${res.total} files (codecanvas.explainAll.maxFiles).`, 'info', 6000);
  explainRun = { stop: false, current: null };
  explainAllBtn.innerHTML = `${icons.stop}<span>Stop explaining</span>`;
  setProgress(`Opening ${plural(res.files.length, 'file')}…`, 0);
  const opened = [];
  for (const p of res.files) {
    const f = await C.openFile(p, { x: 0, y: 0 }, { silent: true });
    if (f) opened.push(f);
  }
  C.arrange(opened);
  C.logHistory('info', `Explain All started (${plural(opened.length, 'file')})`, 0);
  let i = 0;
  for (const f of opened) {
    if (explainRun.stop) break;
    setProgress(`Explaining ${f.path}  (${i + 1}/${opened.length})`, i / opened.length);
    explainRun.current = f.path;
    await C.explainFile(f.path);
    i++;
  }
  setProgress('', 1);
  progress.style.display = 'none';
  toast(explainRun.stop ? `Explain All stopped after ${plural(i, 'file')}.` : `Explained ${plural(i, 'file')}. Click a ▶ tab next to any snippet to read it.`, 'ok', 6000);
  explainRun = null;
  explainAllBtn.innerHTML = `${icons.sparkle}<span>Explain All</span>`;
}
function stopExplainAll() {
  if (!explainRun) { for (const p of S.explaining.keys()) C.cancelExplain(p); progress.style.display = 'none'; return; }
  explainRun.stop = true;
  if (explainRun.current) C.cancelExplain(explainRun.current);
}
function setProgress(text, frac) {
  progress.style.display = 'flex';
  progress.querySelector('.pg-t').textContent = text;
  progress.querySelector('.bar i').style.width = Math.round(frac * 100) + '%';
}
function onExplain() {
  if (explainRun) return;
  const running = [...S.explaining.keys()];
  if (running.length) setProgress(`Explaining ${running.join(', ')}…`, 0.5);
  else progress.style.display = 'none';
}

// ---------------------------------------------------------------- canvas
C.initCanvas(viewport, {
  onView,
  onContext: renderCtxHistory,
  onFilesChanged: () => { renderTree(); updateEmpty(); updateDirty(); },
  onDirty: () => { updateDirty(); updateEmpty(); },
  onSelection: () => {},
  onExplain,
  explainAll,
  onChange: () => updateEmptySoon(),
  onShared: on => { sharedChk.checked = on; },
});
let emptyRaf = 0;
function updateEmptySoon() { if (!emptyRaf) emptyRaf = requestAnimationFrame(() => { emptyRaf = 0; updateEmpty(); }); }

// ---------------------------------------------------------------- host messages
on('init', async m => {
  files = m.files || [];
  wsEl.textContent = m.workspace || '';
  renderTree();
  await C.restoreSaved(m.state);
  gchat.render();
  renderCtxHistory();
  updateEmpty();
  updateDirty();
});
on('files', m => { files = m.files; renderTree(); });
on('fileChanged', m => { C.onDiskChange(m.path, m.text); updateDirty(); });
on('saved', m => toast(`Saved ${m.path}`, 'ok', 1600));
on('toast', m => toast(m.text, m.level || 'info', 6000));
on('command', async m => {
  if (m.cmd === 'openFile' && m.path) C.openFile(m.path);
  if (m.cmd === 'explainFile' && m.path) C.explainFile(m.path);
  if (m.cmd === 'explainAll') explainAll();
});

renderCtxHistory();
send({ type: 'ready' });
