// CodeCanvas core: split panes, file + snippet cards, live drag reflow, references, drafts, edit mode, undo.
import { send, request, stream } from './bridge.js';
import { createChat } from './chat.js';
import { h, esc, uid, clamp, basename, dirname, extColor, nextColor, icons, highlightLines, toast, plural, LANGS, langName } from './util.js';

const EDITOR = window.CC_EDITOR || 'VS Code';
export const LH = 19;          // line height (px) — must match --lh
const HEAD = 35;               // header incl. border
const PAD = 6;                 // body top padding
const BODY_TOP = HEAD + PAD;   // first line offset inside a card
const GP = 10;                 // extra breathing room around a gap while dragging into a file
const FAR_Z = 0.32;            // below this zoom → strategic view
const MIN_Z = 0.04, MAX_Z = 2.5;
const TAB_GAP = 24;            // vertical distance between a snippet's chat tab and its draft tab

export const S = {
  files: new Map(),     // id -> file card
  snippets: new Map(),  // id -> snippet (anchored in a file, or floating card)
  selected: new Set(),  // card ids and anchored-snippet ids
  ctx: [],              // ids in global context
  history: [],          // context history log
  globalChat: [],
  zTop: 10,
  editing: null,
  pick: null,
  undo: [], redo: [],
  explaining: new Map(), // path -> request id
  layout: null,          // split tree: { pane } | { dir: 'row'|'col', children, sizes }
  active: null,          // active pane id
  shared: true,          // split views show the same canvas (true) or each has its own (false)
};

// Shared mode: one real world element W lives in the "host" pane; the other panes show a live mirror (a clone).
let W = null, hostId = null, mirrorObs = null, mirrorTimer = 0;

const panes = new Map(); // id -> pane
let root, menuEl, selTools;
let hooks = {};
let charW = 7.2;

// =====================================================================
// setup
// =====================================================================
export function initCanvas(viewportEl, hk) {
  hooks = hk;
  root = h('div', { id: 'panes' });
  viewportEl.prepend(root);
  for (const el of [root, viewportEl]) el.addEventListener('scroll', () => { el.scrollTop = 0; el.scrollLeft = 0; });
  // Drags capture the pointer on #panes, which retargets the following dblclick there — so listen here, once.
  root.addEventListener('dblclick', e => { const P = paneAt(e.clientX, e.clientY); if (P) onDblClick(e, P); });
  menuEl = document.getElementById('ctxMenu');

  const c = document.createElement('canvas').getContext('2d');
  c.font = '12.5px ' + getComputedStyle(document.body).getPropertyValue('--mono');
  charW = c.measureText('MMMMMMMMMM').width / 10 || 7.2;

  const P = createPane('p1');
  S.layout = { pane: P.id };
  renderLayout();
  setActive(P);

  window.addEventListener('keydown', onKey);
  window.addEventListener('keyup', e => { if (e.code === 'Space') setSpace(false); });
  window.addEventListener('pointerdown', e => { if (!menuEl.contains(e.target)) hideMenu(); }, true);
  window.addEventListener('blur', () => setSpace(false));
  window.addEventListener('pagehide', () => { if (saveTimer) { clearTimeout(saveTimer); send({ type: 'saveState', state: serialize() }); } });
  window.addEventListener('pointerup', e => { lastPointer = e; }, true); // so a replayed press knows the button was released
  requestAnimationFrame(tick);
}

// =====================================================================
// panes (split views)
// =====================================================================
const AP = () => panes.get(S.active) || panes.values().next().value;
const hostPane = () => panes.get(hostId) || AP();
/** The pane whose DOM holds this card (in shared mode that's always the host). */
function paneOf(c) {
  if (S.shared) return hostPane();
  if (!c) return AP();
  if (c.fileId) return paneOf(hostOf(c.fileId));
  return panes.get(c.pane) || AP();
}
/** Is card c visible in pane P? (shared: everything is everywhere) */
const inPane = (c, P) => S.shared || paneOf(c) === P;
function paneAt(cx, cy) {
  for (const P of panes.values()) {
    const r = P.el.getBoundingClientRect();
    if (cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom) return P;
  }
  return null;
}
const mount = c => { const P = paneOf(c); c.pane = P.id; P.world.append(c.el); c._toolsShift = null; c._farBox = null; c._headFixed = null; placeOverlaid(P, c); };

function makeWorld() {
  const world = h('div', { class: 'world' });
  const links = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  links.setAttribute('class', 'links');
  links.setAttribute('width', '1'); links.setAttribute('height', '1');
  world.append(links);
  world._links = links;
  return world;
}

function createPane(id) {
  const el = h('div', { class: 'pane' });
  let world;
  if (S.shared && W) world = W;
  else world = makeWorld();
  const marquee = h('div', { class: 'marquee' });
  const P = { id, el, world, links: world._links, marquee, mirror: null, view: { x: 60, y: 60, z: 1 }, target: { x: 60, y: 60, z: 1 }, anchor: { x: 0, y: 0 }, far: null };
  const bar = h('div', { class: 'pane-bar' },
    paneBtn('splitR', 'Split right (Ctrl+\\)', () => splitPane(P, 'row')),
    paneBtn('splitD', 'Split down', () => splitPane(P, 'col')),
    paneBtn('close', 'Close this view (its cards move to the neighbouring view)', () => closePane(P), 'pane-close'));
  if (S.shared && W) { P.mirror = W.cloneNode(true); el.append(P.mirror, marquee, bar); }
  else {
    el.append(world, marquee, bar);
    if (S.shared) { W = world; hostId = id; observeMirror(); }
  }
  el._pane = P;
  // Shared mode: clicking into a mirrored view moves the real canvas there first, then re-targets the press.
  el.addEventListener('pointerdown', e => {
    if (!S.shared || hostId === P.id || !e.isTrusted) return;
    makeHost(P);
    const t = document.elementFromPoint(e.clientX, e.clientY);
    if (t && t !== e.target && P.el.contains(t)) {
      e.stopImmediatePropagation();
      e.preventDefault();
      t.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true, composed: true, pointerId: e.pointerId, pointerType: e.pointerType, isPrimary: e.isPrimary, button: e.button, buttons: e.buttons, clientX: e.clientX, clientY: e.clientY, screenX: e.screenX, screenY: e.screenY, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: e.metaKey }));
    }
  }, true);
  el.addEventListener('wheel', e => onWheel(e, P), { passive: false });
  el.addEventListener('pointerdown', e => onPointerDown(e, P));
  el.addEventListener('contextmenu', e => onContextMenu(e, P));
  el.addEventListener('pointermove', onHoverMove);
  el.addEventListener('dragover', e => { if (e.dataTransfer.types.includes('text/cc-path')) { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; } });
  el.addEventListener('drop', e => {
    const p = e.dataTransfer.getData('text/cc-path');
    if (!p) return;
    e.preventDefault();
    setActive(P);
    const w = toWorld(e.clientX, e.clientY, P);
    const existing = fileByPath(p);
    if (existing) { moveToPane(existing, P, w.x - 40, w.y - 16); bringToFront(existing); scheduleSave(); }
    else openFile(p, { x: w.x - 40, y: w.y - 16 }, { pane: P.id });
  });
  new ResizeObserver(() => applyView(P)).observe(el);
  // Pan/zoom is done with transforms; the browser must never scroll a view itself (focus() would try to).
  el.addEventListener('scroll', () => { if (el.scrollTop || el.scrollLeft) { el.scrollTop = 0; el.scrollLeft = 0; } });
  panes.set(id, P);
  return P;
}

function paneBtn(icon, title, fn, cls = '') {
  const b = h('button', { class: `icon-btn ${cls}`, title, html: icons[icon] });
  b.addEventListener('pointerdown', e => e.stopPropagation());
  b.addEventListener('click', e => { e.stopPropagation(); fn(); });
  return b;
}

function observeMirror() {
  mirrorObs?.disconnect();
  if (!W) return;
  mirrorObs = new MutationObserver(recs => {
    // the host's own pan/zoom only changes W's style/class; mirrors don't care about that
    if (recs.every(r => r.target === W && r.type === 'attributes')) return;
    if (![...panes.values()].some(P => P.mirror)) return;
    if (!mirrorTimer) mirrorTimer = setTimeout(refreshMirrors, 60);
  });
  mirrorObs.observe(W, { subtree: true, childList: true, attributes: true, characterData: true });
}
function refreshMirrors() {
  mirrorTimer = 0;
  for (const P of panes.values()) {
    if (!P.mirror) continue;
    const c = W.cloneNode(true);
    P.mirror.replaceWith(c);
    P.mirror = c;
    P.far = null;
    applyView(P);
  }
}
/** Shared mode: move the real canvas into pane P; the previous host gets a mirror. */
function makeHost(P) {
  if (!S.shared || !W || hostId === P.id) return;
  const old = panes.get(hostId);
  if (old) {
    old.mirror = W.cloneNode(true);
    old.el.insertBefore(old.mirror, old.marquee);
    old.far = null;
  }
  P.mirror?.remove();
  P.mirror = null;
  P.el.insertBefore(W, P.marquee);
  hostId = P.id;
  P.far = null;
  applyView(P);
  if (old) applyView(old);
}

/** Toggle whether split views share one canvas or each keep their own. */
export function setShared(on) {
  if (S.shared === on) return;
  const host = AP();
  if (on) {
    // merge every view's cards into the active view's canvas, side by side so nothing overlaps
    let right = null;
    const homeCards = allCards().filter(c => paneOf(c) === host);
    if (homeCards.length) right = bounds(homeCards).x1 + 400;
    for (const P of panes.values()) {
      if (P === host) continue;
      const moving = allCards().filter(c => paneOf(c) === P);
      let dx = 0;
      if (moving.length && right != null) { dx = right - bounds(moving).x0; }
      for (const c of moving) { c.x += dx; c.pane = host.id; host.world.append(c.el); positionCard(c); }
      if (moving.length) right = bounds([...homeCards, ...moving]).x1 + 400;
      P.world.remove();
    }
    S.shared = true;
    W = host.world; hostId = host.id;
    for (const P of panes.values()) {
      P.world = W; P.links = W._links;
      if (P !== host) { P.mirror = W.cloneNode(true); P.el.insertBefore(P.mirror, P.marquee); P.far = null; }
    }
    observeMirror();
  } else {
    const h0 = hostPane();
    for (const c of allCards()) c.pane = h0.id;
    S.shared = false;
    mirrorObs?.disconnect();
    for (const P of panes.values()) {
      if (P === h0) continue;
      P.mirror?.remove(); P.mirror = null;
      P.world = makeWorld(); P.links = P.world._links;
      P.el.insertBefore(P.world, P.marquee);
      P.far = null;
    }
    W = null; hostId = null;
  }
  for (const P of panes.values()) applyView(P);
  renderLinks();
  hooks.onShared?.(S.shared);
  scheduleSave();
}

/** Undo any scrolling the browser did on its own (e.g. to reveal a focused text box). */
function resetNativeScroll() {
  for (const el of [root, root?.parentElement, ...[...panes.values()].map(P => P.el)]) {
    if (el && (el.scrollTop || el.scrollLeft)) { el.scrollTop = 0; el.scrollLeft = 0; }
  }
}

function setActive(P) {
  if (!P) return;
  S.active = P.id;
  for (const Q of panes.values()) Q.el.classList.toggle('active', Q === P);
  hooks.onView?.(P.view);
}

function renderLayout() {
  const build = node => {
    if (node.pane) { const el = panes.get(node.pane).el; el.style.flex = ''; return el; }
    const box = h('div', { class: `split ${node.dir}` });
    node.children.forEach((ch, i) => {
      if (i) box.append(makeSash(node, i, box));
      const el = build(ch);
      el.style.flex = `${node.sizes[i]} 1 0`;
      box.append(el);
    });
    return box;
  };
  root.replaceChildren(build(S.layout));
  root.classList.toggle('multi', panes.size > 1);
  for (const P of panes.values()) applyView(P);
}

function makeSash(node, i, box) {
  const sash = h('div', { class: 'sash' });
  sash.addEventListener('pointerdown', e => {
    e.preventDefault(); e.stopPropagation();
    sash.setPointerCapture(e.pointerId);
    const horiz = node.dir === 'row';
    const rect = box.getBoundingClientRect();
    const span = horiz ? rect.width : rect.height;
    const sum = node.sizes.reduce((a, b) => a + b, 0);
    const pair = node.sizes[i - 1] + node.sizes[i];
    const a0 = node.sizes[i - 1], start = horiz ? e.clientX : e.clientY;
    const kids = [...box.children].filter(k => !k.classList.contains('sash'));
    const mv = ev => {
      const a = clamp(a0 + (((horiz ? ev.clientX : ev.clientY) - start) / span) * sum, pair * 0.08, pair * 0.92);
      node.sizes[i - 1] = a; node.sizes[i] = pair - a;
      kids[i - 1].style.flex = `${node.sizes[i - 1]} 1 0`;
      kids[i].style.flex = `${node.sizes[i]} 1 0`;
    };
    const up = () => { sash.removeEventListener('pointermove', mv); sash.removeEventListener('pointerup', up); scheduleSave(); };
    sash.addEventListener('pointermove', mv);
    sash.addEventListener('pointerup', up);
  });
  return sash;
}

function findLeaf(node, id, parent = null, index = 0) {
  if (node.pane) return node.pane === id ? { parent, index } : null;
  for (let i = 0; i < node.children.length; i++) { const r = findLeaf(node.children[i], id, node, i); if (r) return r; }
  return null;
}
const firstLeaf = node => (node.pane ? node.pane : firstLeaf(node.children[0]));
function collapseTree(node) {
  if (node.pane) return node;
  node.children = node.children.map(collapseTree);
  return node.children.length === 1 ? node.children[0] : node;
}

/** Split pane P to the right ('row') or below ('col'). The new view starts looking at the same spot. */
export function splitPane(P = AP(), dir = 'row') {
  const N = createPane(uid('p'));
  N.view = { ...P.view }; N.target = { ...P.target };
  if (N.mirror) N.far = null;
  const leaf = findLeaf(S.layout, P.id);
  if (!leaf.parent) S.layout = { dir, children: [{ pane: P.id }, { pane: N.id }], sizes: [1, 1] };
  else if (leaf.parent.dir === dir) {
    const half = leaf.parent.sizes[leaf.index] / 2;
    leaf.parent.sizes[leaf.index] = half;
    leaf.parent.children.splice(leaf.index + 1, 0, { pane: N.id });
    leaf.parent.sizes.splice(leaf.index + 1, 0, half);
  } else {
    leaf.parent.children[leaf.index] = { dir, children: [{ pane: P.id }, { pane: N.id }], sizes: [1, 1] };
  }
  renderLayout();
  setActive(N);
  scheduleSave();
  return N;
}

export function closePane(P = AP()) {
  if (panes.size <= 1) return;
  const leaf = findLeaf(S.layout, P.id);
  if (S.shared && hostId === P.id) {
    const other = [...panes.values()].find(Q => Q !== P);
    makeHost(other);
  }
  const moving = S.shared ? [] : allCards().filter(c => paneOf(c) === P);
  leaf.parent.children.splice(leaf.index, 1);
  leaf.parent.sizes.splice(leaf.index, 1);
  const T = panes.get(firstLeaf(leaf.parent.children[Math.max(0, leaf.index - 1)]));
  S.layout = collapseTree(S.layout);
  panes.delete(P.id);
  // Place the incoming cards to the right of what the neighbouring view already shows, so nothing overlaps.
  const here = S.shared ? [] : allCards().filter(c => !moving.includes(c) && paneOf(c) === T);
  let dx = 0;
  if (here.length && moving.length) dx = bounds(here).x1 + 400 - bounds(moving).x0;
  for (const c of moving) moveToPane(c, T, c.x + dx, c.y);
  P.el.remove();
  if (S.active === P.id) setActive(T);
  renderLayout();
  renderLinks();
  scheduleSave();
}

function moveToPane(c, P, x, y) {
  c.pane = P.id;
  c.x = x; c.y = y;
  if (!S.shared) P.world.append(c.el);
  positionCard(c);
}

// =====================================================================
// viewport: transform, smooth zoom ("strategic" style), panning — per pane
// =====================================================================
export function toWorld(cx, cy, P = AP()) {
  const r = P.el.getBoundingClientRect();
  return { x: (cx - r.left - P.view.x) / P.view.z, y: (cy - r.top - P.view.y) / P.view.z };
}

function applyView(P) {
  const { x, y, z } = P.view;
  const el = P.mirror || P.world; // shared mode: non-host panes draw their mirror
  el.style.transform = `translate(${x}px, ${y}px) scale(${z})`;
  if (S.editing && paneOf(S.editing) === P) stickHeader(S.editing);
  placeOverlaid(P);
  let g = 24 * z;
  while (g < 14) g *= 4;
  P.el.style.backgroundSize = `${g}px ${g}px`;
  P.el.style.backgroundPosition = `${x}px ${y}px`;
  const far = z < FAR_Z;
  if (far !== P.far) { el.classList.toggle('far', far); P.far = far; }
  // The far labels keep a constant on-screen size. The variable goes on the labels themselves: on the
  // world it would restyle every line of every card on each frame of a zoom.
  if (far) for (const l of el.getElementsByClassName('far-label')) l.style.setProperty('--inv-z', String(1 / z));
  if (P.id === S.active) hooks.onView?.(P.view);
}

function stepView(P) {
  const v = P.view, t = P.target, a = P.anchor;
  let moved = false;
  if (Math.abs(t.z - v.z) > 1e-4) {
    const zN = Math.abs(t.z / v.z - 1) < 0.002 ? t.z : v.z * Math.pow(t.z / v.z, 0.22);
    const r = zN / v.z;
    v.x = a.x - (a.x - v.x) * r; v.y = a.y - (a.y - v.y) * r; v.z = zN;
    moved = true;
  }
  // Pan toward where the target would place the anchor at the *current* zoom (keeps zoom-to-cursor exact).
  const dx = a.x - ((a.x - t.x) / t.z) * v.z, dy = a.y - ((a.y - t.y) / t.z) * v.z;
  if (Math.abs(dx - v.x) > 0.3 || Math.abs(dy - v.y) > 0.3) { v.x += (dx - v.x) * 0.3; v.y += (dy - v.y) * 0.3; moved = true; }
  else if (v.x !== dx || v.y !== dy) { v.x = dx; v.y = dy; moved = true; }
  if (moved) applyView(P);
  return moved;
}

function tick() {
  let any = false;
  for (const P of panes.values()) any = stepView(P) || any;
  if (any) { if (S.pick) onHoverMove(lastPointer); drag?.refresh?.(); scheduleSave(); }
  requestAnimationFrame(tick);
}

function onWheel(e, P) {
  if (e.target.closest('.dropdown')) return;
  e.preventDefault();
  hideMenu(); hideSelTools();
  setActive(P);
  const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? P.el.clientHeight : 1;
  const r = P.el.getBoundingClientRect();
  if (e.ctrlKey || e.metaKey) {
    P.anchor = { x: e.clientX - r.left, y: e.clientY - r.top };
    const t = P.target, a = P.anchor;
    const nz = clamp(t.z * Math.exp(-e.deltaY * k * 0.0016), MIN_Z, MAX_Z);
    t.x = a.x - (a.x - t.x) * (nz / t.z); t.y = a.y - (a.y - t.y) * (nz / t.z); t.z = nz;
    return;
  }
  const dy = e.deltaY * k, dx = e.deltaX * k;
  if (drag?.kind === 'shiftlines') { P.target.y -= (dy || dx); return; }
  if (e.shiftKey) P.target.x -= (dy || dx);
  else { P.target.y -= dy; P.target.x -= dx; }
}

export function zoomTo(z, cx, cy, P = AP()) {
  const r = P.el.getBoundingClientRect();
  P.anchor = { x: cx ?? r.width / 2, y: cy ?? r.height / 2 };
  const t = P.target, a = P.anchor, nz = clamp(z, MIN_Z, MAX_Z);
  t.x = a.x - (a.x - t.x) * (nz / t.z); t.y = a.y - (a.y - t.y) * (nz / t.z); t.z = nz;
}

/** Smoothly frame a world rectangle in pane P. */
export function frameRect(x0, y0, x1, y1, maxZ = 1, P = AP()) {
  const r = P.el.getBoundingClientRect();
  const pad = 60;
  const z = clamp(Math.min((r.width - pad * 2) / Math.max(1, x1 - x0), (r.height - pad * 2) / Math.max(1, y1 - y0)), MIN_Z, maxZ);
  P.target = { z, x: r.width / 2 - ((x0 + x1) / 2) * z, y: r.height / 2 - ((y0 + y1) / 2) * z };
  P.anchor = { x: r.width / 2, y: r.height / 2 };
}

export function fitAll(P = AP()) {
  const cs = allCards().filter(c => inPane(c, P));
  if (!cs.length) { frameRect(0, 0, 800, 600, 1, P); return; }
  const b = bounds(cs);
  frameRect(b.x0, b.y0, b.x1 + 260, b.y1, 1, P);
}

export function focusCard(id, line) {
  const c = S.files.get(id) || S.snippets.get(id);
  if (!c) return;
  const P = S.shared ? AP() : paneOf(c);
  setActive(P);
  if (c.fileId) { // anchored snippet
    const f = hostOf(c.fileId);
    const y = f.y + BODY_TOP + c.start * LH;
    frameRect(f.x, y - 60, f.x + f.w + 360, y + Math.max(200, (c.end - c.start + 1) * LH) + 60, 1, P);
    flashRegion(c);
    return;
  }
  const y = line != null ? c.y + BODY_TOP + line * LH : c.y;
  frameRect(c.x - 40, y - 40, c.x + c.w + 40, line != null ? y + 300 : c.y + Math.min(cardH(c), 900) + 40, 1, P);
  bringToFront(c);
}

function bounds(cs) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const c of cs) { x0 = Math.min(x0, c.x); y0 = Math.min(y0, c.y); x1 = Math.max(x1, c.x + c.w); y1 = Math.max(y1, c.y + cardH(c)); }
  return { x0, y0, x1, y1 };
}

// =====================================================================
// model helpers
// =====================================================================
export const allCards = () => [...S.files.values(), ...[...S.snippets.values()].filter(s => !s.fileId)];
export const fileByPath = p => [...S.files.values()].find(f => f.path === p);
const regionsOf = f => [...S.snippets.values()].filter(s => s.fileId === f.id).sort((a, b) => a.start - b.start || b.end - a.end);
const snippetLines = s => s.fileId ? hostOf(s.fileId).lines.slice(s.start, s.end + 1) : refSource(s) ? snippetLines(refSource(s)) : s.lines;
/** What a marked snippet lives in: a file, or a floating snippet (snippets can be marked inside snippets too). */
const hostOf = id => S.files.get(id) || S.snippets.get(id);
/** A path that names a card's language: the file's own, or the snippet's. */
const hostPath = c => (!c ? '' : c.syntax ? 'x.' + c.syntax : c.type === 'file' ? c.path : snippetPath(c));
/** Redraws a card after its lines or its marked snippets changed. */
function renderHost(c) { if (!c) return; if (c.type === 'file') renderFile(c); else if (c.el) renderSnippetCard(c); }
/** A floating snippet whose lines are its own (not a reference), so it can hold marked snippets. */
const ownLines = c => c?.type === 'snippet' && !c.fileId && !c.ref;
const gutterW = n => Math.max(36, String(n).length * charW + 22);

// ---------- lines wider than their card ----------
const visLen = l => l.replace(/\t/g, '    ').length;
const codeLeft = c => (c.type === 'file' ? gutterW(c.lines.length) : 44);
/** Columns of code that fit across card c; longer lines are cut off at its right edge. */
const fitCols = c => Math.max(1, Math.floor((c.w - 2 - codeLeft(c)) / charW));
const anyWide = c => { const k = fitCols(c); return c.lines.some(l => visLen(l) > k); };
/** Marks the rows that run past the card's edge (CSS draws the fade and the bright edge). */
function markWide(c) {
  if (!c.linesEl) return;
  const k = fitCols(c), kids = c.linesEl.children;
  c.lines.forEach((l, i) => kids[i]?.classList.toggle('wide', visLen(l) > k));
}
let peekCard = null;
/**
 * The hidden part of wide lines, as pillars sticking out of the card's right side, lined up with the
 * visible part: every wide line while the card is edited or its overflow toggle is on, otherwise only
 * the line the pointer rests on. While editing, a click in a pillar places the caret there.
 */
function paintPillars(c) {
  c._pillars?.forEach(p => p.remove());
  c._pillars = [];
  c.linesEl?.querySelectorAll('.ln.open').forEach(l => l.classList.remove('open'));
  if (!c.el?.isConnected || !c.linesEl) return;
  const ed = S.editing === c ? c._editor : null;
  const lines = ed ? ed.ta.value.split('\n') : c.lines;
  const k = fitCols(c);
  const which = ed || c.wide ? lines.map((_, i) => i).filter(i => visLen(lines[i]) > k)
    : c._peek != null && visLen(lines[c._peek] || '') > k ? [c._peek] : [];
  if (!which.length) return;
  const z = paneOf(c)?.view.z || 1;
  // positions inside the card are measured from inside its border
  const cr = c.el.getBoundingClientRect(), bx = c.el.clientLeft, by = c.el.clientTop;
  const left = ed ? (ed.ta.getBoundingClientRect().left - cr.left) / z - bx : codeLeft(c);
  const top0 = ((ed ? ed.ta : c.linesEl).getBoundingClientRect().top - cr.top) / z - by;
  const headBottom = (c.el.querySelector('.card-head')?.getBoundingClientRect().bottom - cr.top) / z || HEAD;
  let caret = null;
  if (ed) { const before = ed.ta.value.slice(0, ed.ta.selectionStart).split('\n'); caret = { line: before.length - 1, col: visLen(before[before.length - 1]) }; }
  for (const i of which) {
    const top = top0 + i * LH;
    if (ed && top < headBottom - 0.5) continue; // nothing sticks out from under the header
    const html = ed ? (ed.hlEl.children[i]?.innerHTML ?? esc(lines[i])) : (c.linesEl.children[i]?.querySelector('.code')?.innerHTML ?? '');
    const cols = Math.min(visLen(lines[i]) - k, 600);
    const inner = h('span', { html });
    inner.style.marginLeft = -k * charW + 'px';
    const p = h('div', { class: 'pillar' }, inner);
    Object.assign(p.style, { left: left + k * charW + 'px', top: top + 'px', width: cols * charW + 8 + 'px' });
    if (caret && caret.line === i && caret.col >= k) p.append(h('div', { class: 'pcaret', style: { left: (caret.col - k) * charW + 'px' } }));
    p.addEventListener('pointerdown', e => {
      e.stopPropagation();
      if (S.editing !== c || !c._editor) return;
      e.preventDefault();
      const col = k + Math.max(0, Math.round((e.clientX - p.getBoundingClientRect().left) / (charW * z)));
      const ta = c._editor.ta, ls = ta.value.split('\n');
      let ch = 0, v = 0;
      for (const x of ls[i]) { if (v >= col) break; v += x === '\t' ? 4 : 1; ch++; }
      const off = ls.slice(0, i).reduce((n, l) => n + l.length + 1, 0) + ch;
      ta.focus({ preventScroll: true });
      ta.setSelectionRange(off, off);
      paintPillars(c);
    });
    c.linesEl.children[i]?.classList.add('open');
    c.el.append(p);
    c._pillars.push(p);
  }
}
/** Hovering a wide line (or its pillar) shows that line's hidden part. */
function peekAt(e) {
  const t = e.target;
  if (t?.closest?.('.pillar')) return;
  const c = t?.closest && cardFromEl(t);
  let i = null;
  if (c && c.linesEl && S.editing !== c && !c.wide && !paneOf(c)?.far && !drag && c.bodyEl?.contains(t)) {
    const n = lineAt(c, e.clientY);
    if (visLen(c.lines[n] || '') > fitCols(c)) i = n;
  }
  const target = i == null ? null : c;
  if (peekCard && (peekCard !== target || peekCard._peek !== i)) { const p = peekCard; peekCard = null; p._peek = null; paintPillars(p); }
  if (target && target._peek !== i) { target._peek = i; peekCard = target; paintPillars(target); }
}
/**
 * Every frame the view moves: each card's header buttons move toward the middle of the header by as much as
 * the context panel or the view's right edge would hide them, and when zoomed out to names, each name stays
 * in the middle of the part of its card that is on screen. Reads come first, then only changed writes.
 */
function placeOverlaid(P, only) {
  if (!P) return;
  const { x, y, z } = P.view;
  const pr = P.el.getBoundingClientRect();
  const panelEl = document.getElementById('ctxHistory');
  if (panelEl && !panelEl._watched) { panelEl._watched = true; new ResizeObserver(() => { for (const Q of panes.values()) placeOverlaid(Q); }).observe(panelEl); }
  const panel = panelEl?.getBoundingClientRect();
  const cards = only ? [only] : allCards().filter(c => c.el && inPane(c, P));
  const hs = P.far ? cards.map(c => cardH(c)) : null;
  cards.forEach((c, k) => {
    const sx = c.x * z + x, sy = c.y * z + y, sw = c.w * z;
    if (P.far) {
      const sh = hs[k] * z, vt = Math.max(sy, 0), vb = Math.min(sy + sh, pr.height), vl = Math.max(sx, 0), vr = Math.min(sx + sw, pr.width);
      const box = vb > vt && vr > vl ? `${(vt - sy) / z}|${(vl - sx) / z}|${(vr - vl) / z}|${(vb - vt) / z}` : '';
      if (box !== c._farBox) {
        c._farBox = box;
        const l = c.el.querySelector('.far-label');
        if (l) { const [t, le, w, hh] = box ? box.split('|') : ['', '', '', '']; Object.assign(l.style, box ? { top: t + 'px', left: le + 'px', width: w + 'px', height: hh + 'px', right: 'auto', bottom: 'auto' } : { top: '', left: '', width: '', height: '', right: '', bottom: '' }); }
      }
      return;
    }
    let limit = pr.left + pr.width;
    const headTop = pr.top + sy + (c._stuck || 0) * z, headBot = headTop + HEAD * z, right = pr.left + sx + sw;
    if (panel && panel.height && headTop < panel.bottom && headBot > panel.top && panel.left < right) limit = Math.min(limit, panel.left);
    // as far as the title can give way: everything else in the header keeps its width (measured once per render)
    if (c._headFixed == null) { const hd = c.el.querySelector('.card-head'); const t = hd?.querySelector('.title'); c._headFixed = hd ? [...hd.children].filter(k => k !== t).reduce((a, k) => a + k.offsetWidth, 0) - (c._toolsShift || 0) + 30 : 0; }
    const shift = Math.round(clamp(right - limit + 8, 0, Math.max(0, sw - c._headFixed * z)) / z);
    if (shift !== c._toolsShift) { c._toolsShift = shift; c.el.querySelector('.card-head')?.style.setProperty('--tools-shift', shift + 'px'); }
  });
}

/** While a card is edited, its header and find bar stay at the top of the view when its top scrolls above it. */
function stickHeader(c) {
  const head = c?.el?.querySelector('.card-head');
  if (!head) return;
  const P = paneOf(c);
  const top = P ? c.y * P.view.z + P.view.y : 0;
  const off = S.editing === c && P && top < 0 ? Math.max(0, Math.min(-top / P.view.z, cardH(c) - HEAD - 8)) : 0;
  head.style.transform = off ? `translateY(${off}px)` : '';
  head.classList.toggle('stuck', off > 0);
  const fb = finds.get(c.id)?.el;
  if (fb) fb.style.transform = off ? `translateY(${off}px)` : '';
  // pillars that would stick out beside the header are left out (see paintPillars)
  if (S.editing === c && (c._stuck || 0) !== off) { c._stuck = off; paintPillars(c); }
}
/** A path-like string that tells the highlighter what language a snippet is in. */
function snippetPath(s) {
  if (s.syntax) return 'x.' + s.syntax;
  if (s.fileId) return hostPath(hostOf(s.fileId));
  const r = refSource(s);
  if (r) return hostPath(hostOf(r.fileId));
  if (s.target?.file) return s.target.file;
  if (s.lang) return /[./]/.test(s.lang) ? s.lang : 'x.' + s.lang;
  return guessLangPath(s.lines || []);
}
/** Fallback: sniff the language from the code itself. */
function guessLangPath(lines) {
  const t = lines.slice(0, 80).join('\n');
  if (/^\s*local\s+\w|\bthen\b|^\s*end\b|^\s*--/m.test(t) && !/[{};]\s*$/m.test(t)) return 'x.lua';
  if (/^\s*(def|class)\s+\w+.*:\s*$|^\s*(import|from)\s+[\w.]+(\s+import\b)?/m.test(t) && !/[{};]\s*$/m.test(t)) return 'x.py';
  if (/^\s*#\s*(include|define|pragma)\b/m.test(t)) return 'x.cpp';
  if (/^#!.*\b(sh|bash)\b|^\s*(echo|export)\s/m.test(t)) return 'x.sh';
  if (/^\s*[\w-]+:\s*\S/m.test(t) && !/[{};()]/.test(t)) return 'x.yaml';
  return 'x.ts';
}
export function cardH(c) {
  if (c.el?.isConnected && c.el.offsetHeight) return c.el.offsetHeight;
  return BODY_TOP + c.lines.length * LH + 10 + (c.type === 'snippet' && (c.target?.file || c.ref) ? 20 : 0);
}
function autoWidth(lines, gutter) {
  let m = 0;
  for (let i = 0; i < Math.min(lines.length, 3000); i++) m = Math.max(m, lines[i].replace(/\t/g, '    ').length);
  return clamp(Math.round(gutter + m * charW + 34), 340, 980);
}
function isDirty(f) { return f.lines.join('\n') !== f.saved; }

function bringToFront(c) { c.el && (c.el.style.zIndex = String(++S.zTop)); }

function positionCard(c) {
  c.el.style.left = c.x + 'px';
  c.el.style.top = c.y + 'px';
  c.el.style.width = c.w + 'px';
  // its header was just built or it moved: place its buttons (and far label) again
  c._toolsShift = null; c._farBox = null; c._headFixed = null;
  if (c.el.isConnected) placeOverlaid(paneOf(c), c);
}

/** Finds a free spot for a w×h card near (px,py) in pane P. */
function findSpot(w, hgt, px, py, P = AP()) {
  const rects = allCards().filter(c => c.el && inPane(c, P)).map(c => ({ x: c.x - 30, y: c.y - 30, x1: c.x + c.w + 300, y1: c.y + cardH(c) + 30 }));
  const free = (x, y) => !rects.some(r => x < r.x1 && x + w > r.x && y < r.y1 && y + Math.min(hgt, 1200) > r.y);
  for (let ring = 0; ring < 40; ring++) {
    for (let k = 0; k <= ring; k++) {
      for (const [dx, dy] of [[ring, k], [k, ring], [-ring, k], [k, -ring], [ring, -k], [-k, ring], [-ring, -k], [-k, -ring]]) {
        const x = px + dx * 120, y = py + dy * 120;
        if (free(x, y)) return { x, y };
      }
    }
  }
  return { x: px, y: py };
}

function viewCenterWorld(P = AP()) {
  const r = P.el.getBoundingClientRect();
  return toWorld(r.left + r.width / 2, r.top + r.height / 2, P);
}

/**
 * Line-precise splice that keeps anchored snippet regions attached to the right lines.
 */
export function spliceFile(f, at, del, ins) {
  const d = ins.length - del;
  const delEnd = at + del - 1;
  for (const s of regionsOf(f)) {
    if (del === 0) {
      if (at <= s.start) { s.start += d; s.end += d; }
      else if (at <= s.end) s.end += d;
      continue;
    }
    if (s.end < at) continue;
    if (s.start > delEnd) { s.start += d; s.end += d; continue; }
    if (at >= s.start && delEnd <= s.end) { // change entirely inside the region
      s.end += d;
      if (s.end < s.start) removeSnippet(s, true);
      continue;
    }
    // partial overlap: keep surviving lines
    const map = i => (i < at ? i : i > delEnd ? i + d : null);
    let a = null, b = null;
    for (let i = s.start; i <= s.end; i++) { const m = map(i); if (m != null) { if (a == null) a = m; b = m; } }
    if (a == null) removeSnippet(s, true);
    else { s.start = a; s.end = b; }
  }
  f.lines.splice(at, del, ...ins);
  if (!f.lines.length) f.lines.push('');
  f.sel = null;
}

/** Replace a file's lines via common prefix/suffix diff, so regions survive edits. */
function setFileLines(f, next) {
  const old = f.lines;
  let p = 0;
  while (p < old.length && p < next.length && old[p] === next[p]) p++;
  let s = 0;
  while (s < old.length - p && s < next.length - p && old[old.length - 1 - s] === next[next.length - 1 - s]) s++;
  if (p === old.length && p === next.length) return false;
  spliceFile(f, p, old.length - s - p, next.slice(p, next.length - s));
  return true;
}

// =====================================================================
// files
// =====================================================================
export async function openFile(path, pos, opts = {}) {
  const existing = fileByPath(path);
  if (existing) { if (!opts.silent) focusCard(existing.id); return existing; }
  let res;
  try { res = await request({ type: 'readFile', path }); } catch { toast(`Couldn't open ${path}`, 'error'); return null; }
  if (res.text == null) return null;
  if (fileByPath(path)) return fileByPath(path);
  const eol = res.text.includes('\r\n') ? '\r\n' : '\n';
  const lines = res.text.split(/\r?\n/);
  if (lines.length > 6000) toast(`${basename(path)} has ${lines.length} lines — rendering may be slow.`, 'info');
  const P = panes.get(opts.pane) || AP();
  const w = opts.w || autoWidth(lines, gutterW(lines.length));
  const hgt = BODY_TOP + lines.length * LH;
  const p = pos || (() => { const c = viewCenterWorld(P); return findSpot(w, hgt, c.x - w / 2, c.y - 200, P); })();
  const f = {
    id: opts.id || uid('f'), type: 'file', path, pane: P.id, x: p.x, y: p.y, w, lines, saved: lines.join('\n'), eol, sel: null,
    chat: opts.chat || [], mode: opts.mode || 'snippet', drawerOpen: !!opts.drawerOpen,
  };
  if (opts.lines) f.lines = opts.lines;
  S.files.set(f.id, f);
  renderHost(f);
  mount(f);
  bringToFront(f);
  if (!opts.silent && !pos) focusCard(f.id);
  hooks.onFilesChanged?.();
  scheduleSave();
  return f;
}

export async function closeFile(f, force) {
  if (!force && isDirty(f)) {
    // confirm()/prompt() are blocked inside VS Code webviews, so ask the host for a native dialog.
    let ok = false;
    try { ok = (await request({ type: 'confirm', message: `${f.path} has unsaved canvas edits. Close it anyway?`, action: 'Close anyway' }, 600000)).ok; } catch { /* */ }
    if (!ok) return;
  }
  for (const s of regionsOf(f)) removeSnippet(s, true);
  f.el.remove();
  f._drawer?.el.remove();
  S.files.delete(f.id);
  S.selected.delete(f.id);
  removeFromCtx(f.id);
  hooks.onFilesChanged?.();
  renderLinks();
  scheduleSave();
}

export function saveFile(f) {
  const text = f.lines.join(f.eol || '\n');
  send({ type: 'saveFile', path: f.path, text });
  f.saved = f.lines.join('\n');
  f.el.classList.toggle('is-dirty', false);
  hooks.onDirty?.();
}
export function saveAll() { for (const f of S.files.values()) if (isDirty(f)) saveFile(f); }
export const anyDirty = () => [...S.files.values()].some(isDirty);

export function onDiskChange(path, text) {
  const f = fileByPath(path);
  if (!f) return;
  const next = text.split(/\r?\n/);
  if (isDirty(f)) { toast(`${path} changed on disk (you have unsaved canvas edits — keeping yours).`, 'info'); f.saved = next.join('\n'); return; }
  if (setFileLines(f, next)) { f.saved = next.join('\n'); renderHost(f); renderLinks(); }
}

function renderFile(f) {
  const el = f.el || (f.el = h('div', { class: 'card file-card', 'data-id': f.id }));
  el.classList.toggle('is-dirty', isDirty(f));
  el.classList.toggle('selected', S.selected.has(f.id));
  el.classList.toggle('in-context', S.ctx.includes(f.id));
  el.style.setProperty('--gutter', gutterW(f.lines.length) + 'px');
  el.replaceChildren();

  const dir = dirname(f.path), base = basename(f.path);
  const tools = h('div', { class: 'tools' },
    iconBtn('explain', 'Explain this file (LLM breaks it into explained snippets)', () => explainFile(f.path)),
    iconBtn('search', 'Find and replace in this file (Ctrl+F)', () => toggleFind(f)),
    iconBtn('notes', 'Toggle all snippet chats/notes', () => toggleAllDrawers(f)),
    iconBtn('chat', 'Chat about this whole file', () => toggleDrawer(f)),
    iconBtn('ctx', 'Add / remove file from global context', () => toggleCtx(f.id), 'ctx-btn'),
    iconBtn('save', 'Save (Ctrl+S)', () => saveFile(f)),
    iconBtn('editor', `Open in ${EDITOR}`, () => send({ type: 'openInEditor', path: f.path })),
    anyWide(f) ? iconBtn('wide', 'Show lines wider than the card in full', () => { f.wide = !f.wide; renderHost(f); scheduleSave(); }, f.wide ? 'on' : '') : null,
    iconBtn('close', 'Close', () => closeFile(f)),
  );
  if (S.explaining.has(f.path)) tools.firstChild.classList.add('spin');
  const head = h('div', { class: 'card-head' },
    h('span', { class: 'ico', style: { background: extColor(f.path) } }),
    h('span', { class: 'title', title: f.path, html: `<span class="dir">${esc(dir)}</span>${esc(base)}` }),
    langChip(f),
    h('span', { class: 'dirty', title: 'Unsaved changes' }),
    tools);

  const hl = highlightLines(f.lines, hostPath(f));
  const linesEl = h('div', { class: 'lines' });
  linesEl.innerHTML = hl.map((x, i) => `<div class="ln"><span class="no">${i + 1}</span><span class="code">${x}</span></div>`).join('');
  f.linesEl = linesEl;
  f.regionsEl = h('div', { class: 'regions' });
  f.selEl = h('div', { class: 'sel-overlay' });
  f.dropEl = h('div', { class: 'drop-indicator' }, h('span', { class: 'lbl' }));
  f.replEl = h('div', { class: 'replace-indicator' });
  f.bodyEl = h('div', { class: 'card-body' }, linesEl, f.regionsEl, f.selEl, f.dropEl, f.replEl);
  f.marginEl = h('div', { class: 'margin' });
  el.append(head, f.bodyEl, f.marginEl, h('div', { class: 'resize' }), h('div', { class: 'edge-l' }), h('div', { class: 'edge-r' }),
    h('div', { class: 'far-label', html: `<span>${esc(base)}<small>${esc(dir)} · ${f.lines.length} lines</small></span>` }));
  renderRegions(f);
  paintSel(f);
  positionCard(f);
  refreshRefs(f);
  sizeFarLabel(f);
  mountFind(f);
  markWide(f);
  requestAnimationFrame(() => paintPillars(f));
}

/** A card drawn while its view is zoomed far out gets its label sized straight away (see applyView). */
function sizeFarLabel(c) {
  const P = paneOf(c);
  if (P?.far) c.el?.querySelector('.far-label')?.style.setProperty('--inv-z', String(1 / P.view.z));
}

/** The language a card is highlighted as; a click picks another (or back to automatic). */
function langChip(c) {
  const chip = h('button', { class: 'lang-chip', title: 'Language used for colouring; click to change' }, langName(hostPath(c)));
  chip.addEventListener('pointerdown', e => e.stopPropagation());
  chip.addEventListener('click', e => {
    e.stopPropagation();
    const set = ext => { if (ext) c.syntax = ext; else delete c.syntax; renderHost(c); for (const x of S.snippets.values()) if (x.ref && refSource(x)?.fileId === c.id && x.el) renderSnippetCard(x); scheduleSave(); };
    showMenu(e, [{ header: 'Language' }, { label: c.type === 'file' ? 'Automatic (from the file name)' : 'Automatic (guess from the code)', fn: () => set(null) },
      ...LANGS.map(([name, ext]) => ({ label: name, fn: () => set(ext) }))]);
  });
  return chip;
}

function iconBtn(icon, title, fn, cls = '') {
  const b = h('button', { class: `icon-btn ${cls}`, title, html: icons[icon] });
  b.addEventListener('pointerdown', e => e.stopPropagation());
  b.addEventListener('click', e => { e.stopPropagation(); fn(e); });
  return b;
}

/** Region overlays inside a file + the margin column: the file's own chat tab, then a chat tab and a draft tab per snippet. */
function renderRegions(f) {
  if (!f?.regionsEl) return;
  f.regionsEl.replaceChildren();
  let lastTabBottom = 30;
  if (f.type === 'file') {
    f.marginEl.querySelectorAll('.mtab').forEach(t => t.remove());
    // the file's own chat, and its draft while that is open or holds changes
    const ftab = buildTab(f);
    ftab.style.top = '6px';
    f.marginEl.append(ftab);
    placeDrawer(ensureDrawer(f).el, f.marginEl, 6, ftab);
    const st = draftStats(f);
    if (f.draftOpen || (st && (st.add || st.del))) {
      const dtab = buildDraftTab(f);
      dtab.style.top = 6 + TAB_GAP + 'px';
      f.marginEl.append(dtab);
      placeDrawer(ensureDraftUI(f).el, f.marginEl, 6 + TAB_GAP, dtab);
      if (f.draftOpen) paintDraft(f);
      lastTabBottom = 30 + TAB_GAP;
    } else f._draftUI?.el.remove();
  } else {
    // a snippet keeps its own chat and draft tabs; those of snippets marked inside it go below
    f.marginEl.querySelectorAll('.mtab').forEach(t => { if (t.dataset.sid !== f.id) t.remove(); });
    lastTabBottom = 8 + TAB_GAP + 22;
  }

  const regs = regionsOf(f);
  const tabTops = [];
  for (const s of regs) {
    const top = PAD + s.start * LH;
    const r = h('div', { class: 'region' + (S.selected.has(s.id) ? ' selected' : '') + (S.hl === s.id && S.selected.has(s.id) ? ' picked' : '') + (isReferenced(s) ? ' referenced' : ''), 'data-sid': s.id, style: { top: top + 'px', height: (s.end - s.start + 1) * LH + 'px' } },
      h('div', { class: 'rbar', title: `${s.title} — click to select · drag to move · drag its top/bottom edge to grow or shrink · Alt+drag for a reference · right-click for options` },
        h('div', { class: 'rh top' }), h('div', { class: 'rh bot' })));
    r.style.setProperty('--rc', s.color);
    f.regionsEl.append(r);
    const tabTop = Math.max(HEAD + top, lastTabBottom + 2);
    lastTabBottom = tabTop + TAB_GAP + 22;
    tabTops.push(tabTop);
  }
  regs.forEach((s, i) => {
    const tabTop = tabTops[i];
    const tab = buildTab(s);
    tab.style.top = tabTop + 'px';
    const dtab = buildDraftTab(s);
    dtab.style.top = tabTop + TAB_GAP + 'px';
    f.marginEl.append(tab, dtab);
    const d = ensureDrawer(s);
    const next = tabTops.slice(i + 1).find(t => t > tabTop + 60);
    d.el.style.maxHeight = (S.notesMode?.has(f.id) && next ? Math.max(120, next - tabTop - 6) : 440) + 'px';
    placeDrawer(d.el, f.marginEl, tabTop, tab);
    placeDrawer(ensureDraftUI(s).el, f.marginEl, tabTop + TAB_GAP, dtab);
    if (s.draftOpen) paintDraft(s);
    if (s.drawerOpen) requestAnimationFrame(() => layoutDraftTab(s));
  });
}

function placeDrawer(el, margin, top, tab) {
  el.style.top = top + 'px';
  el.style.left = '0px';
  el.style.transform = ''; el.style.opacity = '';
  if (el.parentElement !== margin) margin.append(el);
  const fit = () => { el.style.marginLeft = (tab.offsetWidth || 0) + 'px'; };
  fit();
  requestAnimationFrame(fit);
}

function buildTab(s) {
  const isFile = s.type === 'file';
  const label = isFile ? 'Chat' : s.title;
  const tab = h('div', { class: 'mtab chat-tab' + (isFile ? ' file-tab' : '') + (s.drawerOpen ? ' open' : '') + (s.chat.some(m => m.role !== 'note') ? ' has-chat' : ''), 'data-sid': s.id,
    title: isFile ? 'Chat about this whole file' : 'Click: open/close chat · drag: move the snippet (Alt: reference)' },
  h('span', { class: 'arrow' }, '▶'), h('span', { class: 't' }, label));
  tab.style.setProperty('--rc', isFile ? 'var(--accent)' : s.color);
  tab.addEventListener('pointerdown', e => {
    if (spaceDown) return; // let Space+drag move the whole card
    e.stopPropagation();
    hideMenu();
    if (e.button !== 0 || e.target.closest('.title-input')) return;
    if (S.editing) exitEdit();
    if (s.fileId) startRegionDrag(e, s, 'tab');
    else capture(e, () => {}, () => toggleDrawer(s));
  });
  if (!isFile) {
    tab.addEventListener('mouseenter', () => highlightRegion(s, true));
    tab.addEventListener('mouseleave', () => highlightRegion(s, false));
  }
  tab.addEventListener('contextmenu', e => { e.preventDefault(); e.stopPropagation(); if (s.fileId) showMenu(e, regionMenu(s)); });
  return tab;
}

function buildDraftTab(s) {
  const st = draftStats(draftOwner(s));
  const tab = h('div', { class: 'mtab draft-tab' + (s.draftOpen ? ' open' : '') + (st && (st.add || st.del) ? ' has-draft' : ''), 'data-sid': s.id, title: 'Draft: an editable copy shown as a diff — LLM suggestions land here' },
    h('span', { class: 'arrow' }, '▶'), h('span', { class: 't' }, '✎ Draft'),
    st && (st.add || st.del) ? h('span', { class: 'dstat', html: `<b class="a">+${st.add}</b> <b class="d">−${st.del}</b>` }) : null);
  tab.style.setProperty('--rc', s.color || 'var(--accent)');
  tab.addEventListener('pointerdown', e => {
    if (spaceDown) return;
    e.stopPropagation();
    hideMenu();
    if (e.button !== 0) return;
    capture(e, () => {}, () => toggleDraft(s));
  });
  return tab;
}

function highlightRegion(s, on) {
  const f = hostOf(s.fileId);
  f?.regionsEl.querySelector(`[data-sid="${s.id}"]`)?.classList.toggle('hl', on);
}
function flashRegion(s) {
  highlightRegion(s, true);
  setTimeout(() => highlightRegion(s, false), 1400);
}

// ---------- line selection inside a file ----------
function paintSel(f) {
  if (!f.selEl) return;
  if (!f.sel) { f.selEl.style.display = 'none'; return; }
  const a = Math.min(f.sel.a, f.sel.b), b = Math.max(f.sel.a, f.sel.b);
  Object.assign(f.selEl.style, { display: 'block', top: PAD + a * LH + 'px', height: (b - a + 1) * LH + 'px' });
}
const selRange = f => f.sel && { a: Math.min(f.sel.a, f.sel.b), b: Math.max(f.sel.a, f.sel.b) };

function lineAt(c, clientY, boundary) {
  const r = c.linesEl.getBoundingClientRect();
  const local = (clientY - r.top) / paneOf(c).view.z;
  const n = c.lines.length;
  return boundary ? clamp(Math.round(local / LH), 0, n) : clamp(Math.floor(local / LH), 0, n - 1);
}

function clearAllSel(except) {
  for (const f of allCards()) if (f !== except && f.sel) { f.sel = null; paintSel(f); }
  hideSelTools();
}

function showSelTools(f) {
  hideSelTools();
  const r = selRange(f);
  if (!r) return;
  const ln = f.linesEl.children[r.b];
  if (!ln) return;
  const P = paneOf(f);
  const rect = ln.getBoundingClientRect(), vr = P.el.getBoundingClientRect();
  const n = r.b - r.a + 1;
  const mk = (label, title, fn) => { const b = h('button', { title }, label); b.addEventListener('click', e => { e.stopPropagation(); hideSelTools(); fn(); }); return b; };
  selTools = h('div', { class: 'sel-tools' },
    mk('◆ Snippet', 'Mark these lines as a snippet (stays in the file)', () => makeSnippetFromSel(f)),
    mk('✂ Detach', 'Cut these lines out into a floating snippet', () => { const s = makeSnippetFromSel(f); detachRegion(s, f.x + f.w + 80, f.y + BODY_TOP + r.a * LH); }),
    mk('↗ Reference', 'Pop out a reference card: the lines stay here (grayed) and the card mirrors them', () => { const s = makeSnippetFromSel(f); const ref = makeReference(s, f.x + f.w + 80, f.y + BODY_TOP + r.a * LH - 40); flyLines(s.start, s.end - s.start + 1, f, ref); }),
    mk('⧉ Copy out', 'Copy these lines into an independent floating snippet', () => copyOut(f, r)),
    mk('? Ask', 'Make a snippet and ask the LLM about it', () => { const s = makeSnippetFromSel(f); openDrawer(s, true); }),
    mk('◎ Context', 'Add to global context', () => { const s = makeSnippetFromSel(f); toggleCtx(s.id); }),
  );
  selTools.addEventListener('pointerdown', e => e.stopPropagation());
  selTools.style.left = clamp(rect.left - vr.left + 40, 4, vr.width - 420) + 'px';
  selTools.style.top = clamp(rect.bottom - vr.top + 6, 4, vr.height - 40) + 'px';
  selTools.title = `${plural(n, 'line')} selected`;
  P.el.append(selTools);
}
function hideSelTools() { selTools?.remove(); selTools = null; }

function makeSnippetFromSel(f) {
  const r = selRange(f);
  pushUndo();
  const s = createAnchored(f, r.a, r.b, {});
  f.sel = null; paintSel(f);
  return s;
}

function copyOut(f, r) {
  pushUndo();
  const lines = f.lines.slice(r.a, r.b + 1);
  const s = createFloating({ title: guessTitle(lines, hostPath(f), r.a), lines, x: f.x + f.w + 80, y: f.y + BODY_TOP + r.a * LH - 40, pane: paneOf(f).id, lang: hostPath(f) });
  f.sel = null; paintSel(f);
  flyLines(r.a, lines.length, f, s);
}

function guessTitle(lines, path, start) {
  for (const l of lines) {
    const m = /(?:function|def|fn|class|struct|interface|enum|type|const|let|var|func|impl|pub fn|async function)\s+([A-Za-z_$][\w$]*)/.exec(l)
      || /^\s*(?:public|private|protected|static|async|\s)*([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*[{:]/.exec(l);
    if (m) return m[1];
  }
  return `${basename(path)}:${start + 1}-${start + lines.length}`;
}

// =====================================================================
// snippets
// =====================================================================
export function createAnchored(f, a, b, props) {
  const lines = f.lines.slice(a, b + 1);
  const s = {
    id: uid('s'), type: 'snippet', title: props.title || guessTitle(lines, hostPath(f), a), color: props.color || nextColor(),
    chat: props.chat || [], mode: props.mode || 'snippet', drawerOpen: !!props.drawerOpen, explained: !!props.explained,
    fileId: f.id, start: a, end: b,
  };
  S.snippets.set(s.id, s);
  if (!props.noRender) renderRegions(f);
  scheduleSave();
  return s;
}

export function createFloating(props) {
  const s = {
    id: props.id || uid('s'), type: 'snippet', title: props.title || 'Snippet', color: props.color || nextColor(),
    chat: props.chat || [], mode: props.mode || 'snippet', drawerOpen: !!props.drawerOpen,
    pane: (panes.get(props.pane) || AP()).id,
    x: props.x ?? 0, y: props.y ?? 0, w: props.w || autoWidth(props.lines, 44), lines: props.lines.slice(),
    target: props.target || null,
  };
  if (props.ref) s.ref = props.ref;
  if (props.lang) s.lang = props.lang;
  if (props.x == null) { const P = paneOf(s); const c = viewCenterWorld(P); Object.assign(s, findSpot(s.w, cardH(s), c.x, c.y, P)); }
  S.snippets.set(s.id, s);
  renderSnippetCard(s);
  mount(s);
  bringToFront(s);
  renderLinks();
  scheduleSave();
  return s;
}

function renderSnippetCard(s) {
  const src = refSource(s);
  if (s.ref && !src) delete s.ref;
  if (src) s.lines = snippetLines(src).slice();
  const el = s.el || (s.el = h('div', { class: 'card snippet-card', 'data-id': s.id }));
  el.style.setProperty('--rc', s.color);
  el.classList.toggle('ref-card', !!src);
  el.classList.toggle('selected', S.selected.has(s.id));
  el.classList.toggle('in-context', S.ctx.includes(s.id));
  el.style.setProperty('--gutter', '44px');
  el.replaceChildren();
  const tools = h('div', { class: 'tools' });
  if (src) tools.append(iconBtn('editor', 'Jump to the source lines', () => focusCard(src.id)));
  if (s.target?.file) tools.append(iconBtn('merge', 'Apply to its suggested location', () => applyToTarget(s)));
  if (anyWide(s)) tools.append(iconBtn('wide', 'Show lines wider than the card in full', () => { s.wide = !s.wide; renderSnippetCard(s); scheduleSave(); }, s.wide ? 'on' : ''));
  tools.append(
    iconBtn('search', 'Find and replace in this snippet (Ctrl+F)', () => toggleFind(s)),
    iconBtn('ctx', 'Add / remove from global context', () => toggleCtx(s.id), 'ctx-btn'),
    iconBtn('chat', 'Toggle chat', () => toggleDrawer(s)),
    src ? iconBtn('close', 'Remove this reference (the source lines stay)', () => { pushUndo(); removeSnippet(s); })
        : iconBtn('trash', 'Delete snippet', () => { pushUndo(); removeSnippet(s); }),
  );
  const titleEl = h('span', { class: 'title', title: 'Double-click to rename' }, s.title);
  titleEl.addEventListener('dblclick', e => { e.stopPropagation(); renameInline(s, titleEl); });
  const head = h('div', { class: 'card-head' }, h('span', { class: 'ico' }), titleEl, langChip(s), tools);
  const kids = [head];
  if (src) {
    const sf = hostOf(src.fileId);
    const row = h('div', { class: 'target ref-row', title: 'Reference: click to jump to the source. Edits go to the source file.', html: `↗ <b>${esc(sf.type === 'file' ? sf.path : sf.title)}</b> lines ${src.start + 1}–${src.end + 1}` });
    row.addEventListener('pointerdown', e => { if (!spaceDown) e.stopPropagation(); });
    row.addEventListener('click', e => { e.stopPropagation(); focusCard(src.id); });
    kids.push(row);
  } else if (s.target?.file) {
    const where = s.target.replaceStart ? `replace ${s.target.replaceStart}–${s.target.replaceEnd}` : s.target.insertAt ? `near line ${s.target.insertAt}` : '';
    kids.push(h('div', { class: 'target', html: `suggested for <b>${esc(s.target.file)}</b> ${esc(where)}` }));
  }
  const start = src ? src.start + 1 : 1;
  const linesEl = h('div', { class: 'lines' });
  const sfPath = src ? hostPath(hostOf(src.fileId)) : null;
  linesEl.innerHTML = highlightLines(s.lines, s.syntax ? snippetPath(s) : sfPath || snippetPath(s))
    .map((x, i) => `<div class="ln"><span class="no">${start + i}</span><span class="code">${x}</span></div>`).join('');
  s.linesEl = linesEl;
  s.regionsEl = h('div', { class: 'regions' });
  s.selEl = h('div', { class: 'sel-overlay' });
  s.dropEl = h('div', { class: 'drop-indicator' }, h('span', { class: 'lbl' }));
  s.replEl = h('div', { class: 'replace-indicator' });
  s.bodyEl = h('div', { class: 'card-body' }, linesEl, s.regionsEl, s.selEl, s.dropEl, s.replEl);
  const margin = h('div', { class: 'margin' });
  s.marginEl = margin;
  const tab = buildTab(s);
  tab.style.top = '8px';
  const dtab = buildDraftTab(s);
  dtab.style.top = 8 + TAB_GAP + 'px';
  margin.append(tab, dtab);
  kids.push(s.bodyEl, margin, h('div', { class: 'resize' }), h('div', { class: 'edge-l' }), h('div', { class: 'edge-r' }), h('div', { class: 'far-label', html: `<span>${src ? '↗ ' : ''}${esc(s.title)}<small>${plural(s.lines.length, 'line')}</small></span>` }));
  el.append(...kids);
  placeDrawer(ensureDrawer(s).el, margin, 8, tab);
  placeDrawer(ensureDraftUI(s).el, margin, 8 + TAB_GAP, dtab);
  if (s.draftOpen) paintDraft(s);
  if (s.drawerOpen) requestAnimationFrame(() => layoutDraftTab(s));
  if (!src) { renderRegions(s); paintSel(s); }
  positionCard(s);
  sizeFarLabel(s);
  mountFind(s);
  markWide(s);
  requestAnimationFrame(() => paintPillars(s));
}

function renameInline(s, titleEl) {
  const inp = h('input', { class: 'title-input', value: s.title });
  titleEl.replaceWith(inp);
  inp.focus(); inp.select();
  const done = ok => {
    if (ok && inp.value.trim()) s.title = inp.value.trim();
    refreshSnippet(s);
    scheduleSave();
  };
  inp.addEventListener('keydown', e => { e.stopPropagation(); if (e.key === 'Enter') done(true); if (e.key === 'Escape') done(false); });
  inp.addEventListener('blur', () => done(true));
  inp.addEventListener('pointerdown', e => e.stopPropagation());
  inp.addEventListener('click', e => e.stopPropagation());
}

export function renameSnippet(s) {
  const f = s.fileId && hostOf(s.fileId);
  const el = f ? f.marginEl.querySelector(`.chat-tab[data-sid="${s.id}"] .t`) : s.el?.querySelector('.card-head .title');
  if (el) renameInline(s, el);
}

function refreshSnippet(s) {
  if (s.fileId) renderRegions(hostOf(s.fileId)); else renderSnippetCard(s);
  if (s._drawer) s._drawer.titleEl.textContent = s.title;
}

/** Turn every reference to anchored snippet s into a plain copy. */
function unlinkRefs(s) {
  for (const x of S.snippets.values()) {
    if (x.ref !== s.id) continue;
    x.lines = snippetLines(s).slice();
    delete x.ref;
    if (x.el) renderSnippetCard(x);
  }
}

export function removeSnippet(s, silent) {
  for (const x of regionsOf(s)) removeSnippet(x, true); // the snippets marked inside it go with it
  if (s.fileId && hostOf(s.fileId)) unlinkRefs(s);
  const src = refSource(s);
  if (s.el) { s.el.remove(); s.el = null; }
  s._drawer?.el.remove();
  s._draftUI?.el.remove();
  S.snippets.delete(s.id);
  S.selected.delete(s.id);
  removeFromCtx(s.id);
  if (!silent && s.fileId && hostOf(s.fileId)) renderRegions(hostOf(s.fileId));
  if (src && hostOf(src.fileId)) renderRegions(hostOf(src.fileId));
  renderLinks();
  scheduleSave();
}

// ---------- chat drawers (snippets and whole files) ----------
function ensureDrawer(s) {
  if (s._drawer) {
    s._drawer.el.classList.toggle('closed', !s.drawerOpen);
    s._drawer.el.style.setProperty('--rc', s.type === 'file' ? 'var(--accent)' : s.color);
    return s._drawer;
  }
  const isFile = s.type === 'file';
  const titleEl = h('span', { class: 't' }, isFile ? basename(s.path) : s.title);
  const segSnip = h('button', { class: s.mode === 'snippet' ? 'on' : '', title: isFile ? 'Only this file is sent as context' : 'Only this snippet is sent as context' }, isFile ? 'File' : 'Snippet');
  const segGlob = h('button', { class: s.mode === 'global' ? 'on' : '', title: `This ${isFile ? 'file' : 'snippet'} + everything in global context` }, 'Global');
  const setMode = m => { s.mode = m; segSnip.classList.toggle('on', m === 'snippet'); segGlob.classList.toggle('on', m === 'global'); scheduleSave(); };
  segSnip.onclick = () => setMode('snippet');
  segGlob.onclick = () => setMode('global');
  const reset = h('button', { class: 'icon-btn', html: icons.reset, title: 'Reset this chat (explanation notes are kept)' });
  const close = h('button', { class: 'icon-btn', html: icons.close, title: 'Hide chat' });
  close.onclick = () => toggleDrawer(s, false);
  const chat = createChat({
    messages: () => s.chat,
    context: () => (isFile ? fileContext(s) : snippetContext(s)),
    onSnippets: snips => (isFile ? routeFileDrafts(s, snips) : routeDrafts(s, snips)),
    onCodeToSnippet: (code, info, attrs) => spawnDrafts([{ title: attrs.title || 'From chat', code, file: attrs.file, insertAt: +attrs.insert || undefined, lang: (info || '').split(/\s/)[0] || undefined }], s),
    onCodeToDraft: isFile ? null : code => setDraft(s, code),
    onFocusSnippet: id => focusSnippetOrDraft(id),
    onChange: () => { scheduleSave(); renderTabsSoon(isFile ? s.id : s.fileId || s.id); },
    placeholder: isFile ? `Ask about ${basename(s.path)}…` : `Ask about “${s.title}”…`,
    empty: isFile
      ? 'Ask anything about this whole file. <b>File</b> sends only this file; <b>Global</b> also sends everything in the context list (top right). Replies stay in this thread.'
      : 'Ask anything about this snippet. <b>Snippet</b> sends only this code; <b>Global</b> also sends everything in the context list (top right). Suggested changes land in the <b>✎ Draft</b> tab as a diff.',
  });
  reset.onclick = () => { s.chat.splice(0, s.chat.length, ...s.chat.filter(m => m.role === 'note')); chat.render(); scheduleSave(); renderTabsSoon(isFile ? s.id : s.fileId || s.id); };
  const el = h('div', { class: 'drawer' + (s.drawerOpen ? '' : ' closed'), 'data-sid': s.id },
    h('div', { class: 'dr-head' }, titleEl, h('div', { class: 'seg' }, segSnip, segGlob), reset, close), chat.el);
  el.style.setProperty('--rc', isFile ? 'var(--accent)' : s.color);
  el.addEventListener('pointerdown', e => { e.stopPropagation(); el.style.zIndex = String(++S.zTop); });
  el.addEventListener('wheel', e => { if (!e.ctrlKey && e.target.closest('.msgs')) e.stopPropagation(); }, { passive: true });
  s._drawer = { el, chat, titleEl };
  if (!isFile) new ResizeObserver(() => layoutDraftTab(s)).observe(el);
  return s._drawer;
}

const pendingTabs = new Set();
function renderTabsSoon(cardId) {
  if (pendingTabs.has(cardId)) return;
  pendingTabs.add(cardId);
  requestAnimationFrame(() => {
    pendingTabs.delete(cardId);
    const c = S.files.get(cardId) || S.snippets.get(cardId);
    if (!c?.el || S.editing) return;
    const list = c.type === 'file' ? [c, ...regionsOf(c)] : [c];
    for (const s of list) c.el.querySelector(`.chat-tab[data-sid="${s.id}"]`)?.classList.toggle('has-chat', s.chat.some(m => m.role !== 'note'));
  });
}

export function toggleDrawer(s, force) {
  s.drawerOpen = force ?? !s.drawerOpen;
  const d = ensureDrawer(s);
  d.el.classList.toggle('closed', !s.drawerOpen);
  const host = s.fileId ? hostOf(s.fileId) : s;
  host?.el?.querySelector(`.chat-tab[data-sid="${s.id}"]`)?.classList.toggle('open', s.drawerOpen);
  if (s.drawerOpen) { d.el.style.zIndex = String(++S.zTop); bringToFront(host); d.chat.render(); }
  layoutDraftTab(s);
  scheduleSave();
}

/** The draft tab (and its drawer) live right under the chat tab — or under the chat window when it's open. */
function layoutDraftTab(s) {
  if (s.type === 'file') return;
  const host = s.fileId ? hostOf(s.fileId) : s;
  const tab = host?.el?.querySelector(`.chat-tab[data-sid="${s.id}"]`);
  const dtab = host?.el?.querySelector(`.draft-tab[data-sid="${s.id}"]`);
  if (!tab || !dtab) return;
  const base = parseFloat(tab.style.top) || 0;
  const chatH = s.drawerOpen && s._drawer ? s._drawer.el.offsetHeight : 0;
  const top = s.drawerOpen && chatH ? base + chatH + 6 : base + TAB_GAP;
  dtab.style.top = top + 'px';
  if (s._draftUI) s._draftUI.el.style.top = top + 'px';
}
export function openDrawer(s, focus) { toggleDrawer(s, true); if (focus) setTimeout(() => s._drawer.chat.focus(), 50); }

function toggleAllDrawers(f) {
  const regs = regionsOf(f);
  if (!S.notesMode) S.notesMode = new Set();
  const open = !regs.some(s => s.drawerOpen);
  if (open) S.notesMode.add(f.id); else S.notesMode.delete(f.id);
  renderRegions(f);
  regs.forEach(s => toggleDrawer(s, open));
}

function snippetContext(s) {
  const items = [ctxItem(s.id)];
  if (s.mode === 'global') for (const id of S.ctx) if (id !== s.id) { const it = ctxItem(id); it && items.push(it); }
  const f = s.fileId && hostOf(s.fileId);
  const rs = refSource(s), rf = rs && hostOf(rs.fileId);
  const where = f ? `${f.type === 'file' ? f.path : `the snippet "${f.title}"`} lines ${s.start + 1}-${s.end + 1}` : rf ? `a live reference to ${hostPath(rf)} lines ${rs.start + 1}-${rs.end + 1}` : s.target?.file ? `a draft for ${s.target.file}` : 'a free-floating snippet';
  return {
    items,
    focus: `the snippet "${s.title}" (${where}). If you propose changes to this snippet, reply with the COMPLETE revised snippet in exactly one \`\`\`snippet block without file/insert/replace attributes — it is shown to the user as a diff draft`,
  };
}
function fileContext(f) {
  const items = [ctxItem(f.id)];
  if (f.mode === 'global') for (const id of S.ctx) if (id !== f.id) { const it = ctxItem(id); it && items.push(it); }
  return { items, focus: `the file ${f.path}. If you propose changes to this file, give each change as a \`\`\`snippet block with file="${f.path}" and replace="A-B" (or insert="N"), using the file's line numbers as they are now; the changes are combined into one diff draft that the user reviews and applies` };
}

// =====================================================================
// drafts: an editable copy of a snippet, shown as a git-style diff
// =====================================================================
const draftOwner = s => refSource(s) || s;
function draftStats(o) {
  if (!o?.draft) return null;
  let add = 0, del = 0;
  for (const op of diffLines(snippetLines(o), o.draft)) { if (op.t === '+') add++; else if (op.t === '-') del++; }
  return { add, del };
}

/** Line diff (LCS with common prefix/suffix trimmed). ops: {t:' '|'-'|'+', a, b}. */
export function diffLines(a, b) {
  let p = 0;
  while (p < a.length && p < b.length && a[p] === b[p]) p++;
  let s = 0;
  while (s < a.length - p && s < b.length - p && a[a.length - 1 - s] === b[b.length - 1 - s]) s++;
  const A = a.slice(p, a.length - s), B = b.slice(p, b.length - s);
  const ops = [];
  for (let i = 0; i < p; i++) ops.push({ t: ' ', a: i, b: i });
  const n = A.length, m = B.length;
  if (n * m > 250000) {
    for (let i = 0; i < n; i++) ops.push({ t: '-', a: p + i, b: null });
    for (let j = 0; j < m; j++) ops.push({ t: '+', a: null, b: p + j });
  } else {
    const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    let i = 0, j = 0;
    while (i < n && j < m) {
      if (A[i] === B[j]) { ops.push({ t: ' ', a: p + i, b: p + j }); i++; j++; }
      else if (dp[i + 1][j] >= dp[i][j + 1]) { ops.push({ t: '-', a: p + i, b: null }); i++; }
      else { ops.push({ t: '+', a: null, b: p + j }); j++; }
    }
    while (i < n) { ops.push({ t: '-', a: p + i, b: null }); i++; }
    while (j < m) { ops.push({ t: '+', a: null, b: p + j }); j++; }
  }
  for (let k = 0; k < s; k++) ops.push({ t: ' ', a: a.length - s + k, b: b.length - s + k });
  return ops;
}

function ensureDraftUI(s) {
  if (s._draftUI) { s._draftUI.el.classList.toggle('closed', !s.draftOpen); s._draftUI.el.style.setProperty('--rc', s.color || 'var(--accent)'); return s._draftUI; }
  const stats = h('span', { class: 'dstats' });
  const editBtn = h('button', { class: 'dbtn', title: 'Edit the draft directly' }, 'Edit');
  const reset = h('button', { class: 'icon-btn', html: icons.reset, title: 'Reset the draft to the current snippet' });
  const apply = h('button', { class: 'dbtn primary', title: 'Replace the snippet with this draft' }, 'Apply');
  const close = h('button', { class: 'icon-btn', html: icons.close, title: 'Hide draft' });
  const body = h('div', { class: 'dbody' });
  const ask = h('textarea', { rows: 1, placeholder: 'Ask the LLM for a change… (Enter)' });
  const el = h('div', { class: 'drawer draft-drawer' + (s.draftOpen ? '' : ' closed'), 'data-sid': s.id },
    h('div', { class: 'dr-head' }, h('span', { class: 't' }, '✎ Draft'), stats, editBtn, reset, apply, close),
    body, h('div', { class: 'dask' }, ask));
  el.style.setProperty('--rc', s.color || 'var(--accent)');
  const ui = { el, body, stats, editBtn, ask, editing: false };
  s._draftUI = ui;
  el.addEventListener('pointerdown', e => { e.stopPropagation(); el.style.zIndex = String(++S.zTop); });
  el.addEventListener('dblclick', e => e.stopPropagation());
  el.addEventListener('contextmenu', e => e.stopPropagation());
  el.addEventListener('wheel', e => { if (!e.ctrlKey && body.scrollHeight > body.clientHeight) e.stopPropagation(); }, { passive: true });
  close.onclick = () => toggleDraft(s, false);
  reset.onclick = () => { const o = draftOwner(s); o.draft = snippetLines(o).slice(); ui.editing = false; paintDraft(s); scheduleSave(); };
  apply.onclick = () => applyDraft(s);
  editBtn.onclick = () => { ui.editing = !ui.editing; paintDraft(s); if (ui.editing) body.querySelector('textarea')?.focus(); };
  ask.addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key !== 'Enter' || e.shiftKey) return;
    e.preventDefault();
    const text = ask.value.trim();
    if (!text) return;
    ask.value = '';
    const d = ensureDrawer(s);
    if (d.chat.busy) { toast('This snippet\'s chat is still answering.'); return; }
    d.chat.submit(s.type === 'file' ? `Change this file: ${text}` : `Revise this snippet: ${text}`);
    ui.pending = true;
    paintDraft(s);
  });
  return ui;
}

function paintDraft(s) {
  const ui = ensureDraftUI(s);
  const o = draftOwner(s);
  if (!o.draft) o.draft = snippetLines(o).slice();
  const base = snippetLines(o);
  const st = draftStats(o);
  ui.stats.innerHTML = st.add || st.del ? `<b class="a">+${st.add}</b> <b class="d">−${st.del}</b>` : '<span class="same">no changes</span>';
  ui.editBtn.textContent = ui.editing ? 'Done' : 'Edit';
  ui.body.replaceChildren();
  if (ui.editing) {
    const ta = h('textarea', { class: 'dedit', spellcheck: 'false', wrap: 'off' });
    ta.value = o.draft.join('\n');
    ta.style.height = Math.min(360, (o.draft.length + 1) * LH) + 'px';
    ta.addEventListener('input', () => { o.draft = ta.value.split('\n'); const t = draftStats(o); ui.stats.innerHTML = t.add || t.del ? `<b class="a">+${t.add}</b> <b class="d">−${t.del}</b>` : '<span class="same">no changes</span>'; scheduleSave(); });
    ta.addEventListener('keydown', e => {
      e.stopPropagation();
      if (e.key === 'Tab') { e.preventDefault(); ta.setRangeText('    ', ta.selectionStart, ta.selectionEnd, 'end'); ta.dispatchEvent(new Event('input')); }
      if (e.key === 'Escape') { ui.editing = false; paintDraft(s); refreshDraftTab(s); }
    });
    ta.addEventListener('blur', () => refreshDraftTab(s));
    ui.body.append(ta);
  } else {
    const ops = diffLines(base, o.draft);
    const path = hostPath(o);
    const hA = highlightLines(base, path), hB = highlightLines(o.draft, path);
    const startNo = o.fileId ? o.start + 1 : 1;
    const changed = ops.map(op => op.t !== ' ');
    const keep = i => (st.add || st.del) ? changed.slice(Math.max(0, i - 3), i + 4).some(Boolean) : true;
    const out = [];
    let skipped = 0;
    const fold = () => { if (skipped) out.push(`<div class="drow fold">⋯ ${plural(skipped, 'unchanged line')}</div>`); skipped = 0; };
    ops.forEach((op, i) => {
      if (!keep(i)) { skipped++; return; }
      fold();
      const cls = op.t === '+' ? 'add' : op.t === '-' ? 'del' : 'ctx';
      const code = op.t === '+' ? hB[op.b] : hA[op.a];
      out.push(`<div class="drow ${cls}"><span class="dn">${op.a != null ? startNo + op.a : ''}</span><span class="dn">${op.b != null ? startNo + op.b : ''}</span><span class="dm">${op.t === ' ' ? '' : op.t === '-' ? '−' : '+'}</span><span class="dc">${code || ' '}</span></div>`);
    });
    fold();
    const rows = out.join('');
    const wrap = h('div', { class: 'diff', html: rows });
    if (!st.add && !st.del) wrap.prepend(h('div', { class: 'dempty' }, ui.pending ? 'Asked the LLM — its revision will appear here as a diff…' : 'No changes yet. Click Edit, or ask the LLM below — suggested changes show up here as a diff you can Apply.'));
    ui.body.append(wrap);
  }
}

function refreshDraftTab(s) {
  const host = s.fileId ? hostOf(s.fileId) : s;
  const old = host?.el?.querySelector(`.draft-tab[data-sid="${s.id}"]`);
  if (!old) return;
  const nt = buildDraftTab(s);
  nt.style.top = old.style.top;
  nt.style.transform = old.style.transform;
  old.replaceWith(nt);
  if (s._draftUI) s._draftUI.el.style.marginLeft = nt.offsetWidth + 'px';
}

export function toggleDraft(s, force) {
  s.draftOpen = force ?? !s.draftOpen;
  const ui = ensureDraftUI(s);
  ui.el.classList.toggle('closed', !s.draftOpen);
  if (s.draftOpen) { paintDraft(s); ui.el.style.zIndex = String(++S.zTop); bringToFront(s.fileId ? hostOf(s.fileId) : s); }
  if (s.type === 'file') renderRegions(s); // a file's draft tab only shows while it is open or holds changes
  else refreshDraftTab(s);
  scheduleSave();
}

function setDraft(s, code) {
  const o = draftOwner(s);
  o.draft = code.replace(/\n$/, '').split('\n');
  if (s._draftUI) { s._draftUI.pending = false; s._draftUI.editing = false; }
  toggleDraft(s, true);
  s._draftUI.el.animate([{ boxShadow: '0 0 0 2px var(--ok)' }, { boxShadow: '0 0 0 0 transparent' }], { duration: 900 });
}

/** A snippet chat's reply: the block that revises this snippet becomes its draft, everything else becomes cards. */
function routeDrafts(s, snips) {
  const o = draftOwner(s);
  const own = o.fileId ? hostOf(o.fileId).path || null : null;
  const idx = snips.findIndex(d => !d.file || (own && d.file === own && (d.replaceStart || !d.insertAt)));
  const rest = snips.filter((_, i) => i !== idx);
  if (idx >= 0) setDraft(s, snips[idx].code);
  else if (s._draftUI) { s._draftUI.pending = false; if (s.draftOpen) paintDraft(s); }
  const spawned = spawnDrafts(rest, s);
  let k = 0;
  return snips.map((_, i) => (i === idx ? 'draft:' + s.id : spawned[k++]));
}

/**
 * A file's lines with the changes suggested for it applied. Each replace="A-B" / insert="N" is in the
 * file's numbering before any change, so they go in bottom-up; a block with neither is the whole new file.
 */
export function fileDraft(base, changes) {
  const split = c => c.replace(/\n$/, '').split('\n');
  const whole = [...changes].reverse().find(d => !d.replaceStart && !d.insertAt);
  if (whole) return split(whole.code);
  const n = base.length;
  const at = changes.map(d => {
    if (d.replaceStart) { const a = clamp(d.replaceStart - 1, 0, n); return { a, del: Math.max(Math.min(d.replaceEnd || d.replaceStart, n), a) - a, ins: split(d.code) }; }
    return { a: clamp(d.insertAt - 1, 0, n), del: 0, ins: split(d.code) };
  }).sort((x, y) => y.a - x.a);
  const out = base.slice();
  let floor = Infinity;
  for (const { a, del, ins } of at) { out.splice(a, Math.min(del, Math.max(0, floor - a)), ...ins); floor = a; }
  return out;
}

/** A file chat's reply: changes to the file's own lines become its draft, everything else becomes cards. */
function routeFileDrafts(f, snips) {
  const mine = snips.filter(d => d.file === f.path);
  const rest = snips.filter(d => d.file !== f.path);
  if (mine.length) {
    f.draft = fileDraft(f.lines, mine);
    if (f._draftUI) { f._draftUI.pending = false; f._draftUI.editing = false; }
    toggleDraft(f, true);
    f._draftUI?.el.animate([{ boxShadow: '0 0 0 2px var(--ok)' }, { boxShadow: '0 0 0 0 transparent' }], { duration: 900 });
  }
  const spawned = spawnDrafts(rest, f);
  let k = 0;
  return snips.map(d => (d.file === f.path ? 'draft:' + f.id : spawned[k++]));
}

function focusSnippetOrDraft(id) {
  if (id.startsWith('draft:')) { const s = hostOf(id.slice(6)); if (s) { toggleDraft(s, true); focusCard(s.id); } return; }
  focusCard(id);
}

function applyDraft(s) {
  const o = draftOwner(s);
  if (!o.draft) return;
  const base = snippetLines(o);
  if (base.join('\n') === o.draft.join('\n')) { toast('The draft has no changes.'); return; }
  pushUndo();
  const ops = diffLines(base, o.draft);
  const next = o.draft.slice();
  if (o.fileId) {
    const f = hostOf(o.fileId);
    const at = o.start, n = o.end - o.start + 1;
    if (!next.length) { removeSnippet(o, true); spliceFile(f, at, n, []); }
    else spliceFile(f, at, n, next);
    o.draft = null;
    renderHost(f);
    for (const op of ops) if (op.t === '+') f.linesEl.children[at + op.b]?.classList.add('flash');
    hooks.onDirty?.();
  } else if (o.type === 'file') {
    setFileLines(o, next);
    o.draft = null;
    renderHost(o);
    for (const op of ops) if (op.t === '+') o.linesEl.children[op.b]?.classList.add('flash');
    hooks.onDirty?.();
  } else {
    setFileLines(o, next);
    o.draft = null;
    renderSnippetCard(o);
  }
  if (s !== o && s.el) renderSnippetCard(s);
  toast('Draft applied', 'ok', 1500);
  scheduleSave();
}

// =====================================================================
// context (global)
// =====================================================================
export function ctxItem(id) {
  const f = S.files.get(id);
  if (f) return { kind: 'file', label: f.path, path: f.path, text: f.lines.join('\n') };
  const s = S.snippets.get(id);
  if (!s) return null;
  if (s.fileId) { const hf = hostOf(s.fileId); return { kind: 'snippet', label: s.title, path: hf.path || hf.target?.file, startLine: s.start + 1, text: snippetLines(s).join('\n') }; }
  const rs = refSource(s);
  if (rs) return { kind: 'snippet', label: s.title, path: hostOf(rs.fileId).path || undefined, startLine: rs.start + 1, text: snippetLines(rs).join('\n') };
  return { kind: 'snippet', label: s.title, path: s.target?.file, startLine: s.target?.replaceStart || s.target?.insertAt, text: s.lines.join('\n') };
}
export const ctxLines = id => (S.files.get(id)?.lines.length) ?? (S.snippets.has(id) ? snippetLines(S.snippets.get(id)).length : 0);
export const ctxLabel = id => S.files.get(id)?.path ?? S.snippets.get(id)?.title ?? '?';

export function logHistory(kind, text, lines, ref) {
  S.history.push({ t: Date.now(), kind, text, lines, ref });
  if (S.history.length > 300) S.history.splice(0, S.history.length - 300);
  hooks.onContext?.();
  scheduleSave();
}

export function toggleCtx(id) {
  const i = S.ctx.indexOf(id);
  const isFile = S.files.has(id);
  if (i >= 0) {
    S.ctx.splice(i, 1);
    logHistory('rem', `${isFile ? 'file' : 'snippet'} ${ctxLabel(id)}`, ctxLines(id), id);
  } else {
    S.ctx.push(id);
    logHistory('add', `${isFile ? 'file' : 'snippet'} ${ctxLabel(id)}`, ctxLines(id), id);
  }
  markCtx(id);
}
function removeFromCtx(id) {
  const i = S.ctx.indexOf(id);
  if (i < 0) return;
  S.ctx.splice(i, 1);
  logHistory('rem', `${ctxLabel(id) === '?' ? 'item' : ctxLabel(id)} (removed from canvas)`, 0, id);
}
function markCtx(id) {
  const c = S.files.get(id) || S.snippets.get(id);
  c?.el?.classList.toggle('in-context', S.ctx.includes(id));
  if (c?.fileId) renderRegions(hostOf(c.fileId));
}
export const globalContextItems = () => S.ctx.map(ctxItem).filter(Boolean);

// =====================================================================
// animations: lines flying between cards
// =====================================================================
const wait = ms => new Promise(r => setTimeout(r, ms));

function codePoints(c, from, count) {
  const P = paneOf(c), out = [];
  const kids = c.linesEl.children;
  for (let i = 0; i < count; i++) {
    const code = kids[from + i]?.querySelector('.code');
    if (!code) break;
    const r = code.getBoundingClientRect();
    out.push(toWorld(r.left, r.top, P));
  }
  return out;
}

/** Destination points for lines that will end up at `index` in card c (measured before the animation). */
function destPoints(c, index, count) {
  const P = paneOf(c);
  const lr = c.linesEl.getBoundingClientRect();
  const first = c.linesEl.querySelector('.code');
  const left = first ? toWorld(first.getBoundingClientRect().left, 0, P).x : toWorld(lr.left, 0, P).x + 44;
  const top = toWorld(0, lr.top, P).y;
  return Array.from({ length: count }, (_, k) => ({ x: left, y: top + (index + k) * LH }));
}

function fly(htmls, from, to, P) {
  const n = Math.min(htmls.length, from.length, to.length, 140);
  if (!n) return Promise.resolve();
  const stagger = Math.min(16, 260 / n);
  const els = [];
  for (let k = 0; k < n; k++) {
    const d = h('div', { class: 'fly', html: htmls[k] || ' ' });
    d.style.left = from[k].x + 'px'; d.style.top = from[k].y + 'px';
    P.world.append(d);
    els.push(d);
  }
  requestAnimationFrame(() => requestAnimationFrame(() => els.forEach((d, k) => {
    d.style.transitionDelay = k * stagger + 'ms';
    d.style.transform = `translate(${to[k].x - from[k].x}px, ${to[k].y - from[k].y}px)`;
  })));
  return wait(440 + n * stagger).then(() => els.forEach(d => d.remove()));
}

function openGap(c, at, count) {
  const gap = h('div', { class: 'ln gap' });
  c.linesEl.insertBefore(gap, c.linesEl.children[at] || null);
  c.regionsEl && (c.regionsEl.style.opacity = '0');
  requestAnimationFrame(() => requestAnimationFrame(() => { gap.style.height = count * LH + 'px'; }));
  return gap;
}

function collapseLines(c, from, count, doomed) {
  const kids = [...c.linesEl.children].filter(k => !k.classList.contains('gap'));
  for (let i = from; i < from + count; i++) {
    const k = kids[i];
    if (!k) continue;
    if (doomed) { k.classList.add('doomed'); setTimeout(() => k.classList.add('collapse'), 170); }
    else { k.style.opacity = '.25'; k.classList.add('collapse'); }
  }
  c.regionsEl && (c.regionsEl.style.opacity = '0');
}

function flashLines(f, from, count) {
  const kids = f.linesEl.children;
  for (let i = from; i < from + count; i++) kids[i]?.classList.add('flash');
}

/** Copy animation for "copy out" / references (file unchanged). */
function flyLines(from, count, f, s) {
  if (paneOf(f) !== paneOf(s)) return;
  const htmls = [...f.linesEl.children].slice(from, from + count).map(k => k.querySelector('.code').innerHTML);
  const src = codePoints(f, from, count);
  s.el.style.opacity = '0';
  requestAnimationFrame(() => {
    const dst = destPoints(s, 0, count);
    fly(htmls, src, dst, paneOf(f)).then(() => { s.el.style.transition = 'opacity .2s'; s.el.style.opacity = ''; });
  });
}

/** After a snippet lands in a file, select it so it can be grabbed again straight away. */
function selectLanded(s) {
  S.hl = null;
  S.selected = new Set([s.id]);
  paintSelection();
}

/**
 * Merge a floating snippet into file f, inserting before line `at` (0-based) and replacing `del` lines.
 * Used by right-click → Merge and pick mode; dragging uses the live slot instead.
 */
export async function mergeSnippet(s, f, at, del = 0) {
  if (!s || s.fileId || !f || S.animating) return;
  S.animating = true;
  pushUndo();
  if (s.ref) { materializeRef(s); renderSnippetCard(s); }
  hideIndicators(f);
  const n = s.lines.length;
  const same = paneOf(s) === paneOf(f);
  const htmls = same ? [...s.linesEl.children].map(k => k.querySelector('.code').innerHTML) : [];
  const src = same ? codePoints(s, 0, n) : [];
  const dst = same ? destPoints(f, at, n) : [];
  if (del) collapseLines(f, at, del, true);
  openGap(f, at + del, n);
  s.el.classList.add('fading');
  await wait(del ? 120 : 0);
  await (same ? fly(htmls, src, dst, paneOf(f)) : wait(300));
  s.el.remove(); s.el = null;
  s._drawer?.el.remove(); s._draftUI?.el.remove();
  const lines = s.lines;
  // the snippets marked inside it land on the same lines in the file
  const riders = regionsOf(s);
  riders.forEach(r => { r.fileId = null; });
  spliceFile(f, at, del, lines);
  riders.forEach(r => Object.assign(r, { fileId: f.id, start: r.start + at, end: r.end + at }));
  Object.assign(s, { fileId: f.id, start: at, end: at + n - 1 });
  delete s.lines; delete s.x; delete s.y; delete s.w; delete s.pane;
  s.target = null;
  renderHost(f);
  selectLanded(s);
  fadeInTabs(s);
  renderLinks();
  S.animating = false;
  hooks.onDirty?.();
  scheduleSave();
}

/** Lift an anchored region out of its file into a floating card at (x,y). copy=true leaves the file alone. */
export async function detachRegion(s, x, y, copy = false) {
  const f = hostOf(s.fileId);
  if (!f || S.animating) return;
  S.animating = true;
  if (!copy) pushUndo();
  const P = paneOf(f);
  const n = s.end - s.start + 1;
  const lines = f.lines.slice(s.start, s.end + 1);
  const htmls = [...f.linesEl.children].slice(s.start, s.end + 1).map(k => k.querySelector('.code').innerHTML);
  const src = codePoints(f, s.start, n);
  const dst = Array.from({ length: n }, (_, k) => ({ x: x + 44, y: y + BODY_TOP + k * LH }));
  if (!copy) collapseLines(f, s.start, n, false);
  await fly(htmls, src, dst, P);
  let fl;
  if (copy) {
    fl = createFloating({ title: s.title + ' (copy)', lines, x, y, color: s.color, chat: s.chat.map(m => ({ ...m })), mode: s.mode, pane: P.id, lang: hostPath(f) });
    renderHost(f);
  } else {
    const start = s.start;
    unlinkRefs(s);
    s.fileId = null;
    s._drawer?.el.remove(); s._draftUI?.el.remove();
    spliceFile(f, start, n, []);
    Object.assign(s, { lines, x, y, pane: P.id, w: Math.max(340, Math.min(f.w, autoWidth(lines, 44))), target: null, lang: hostPath(f) });
    delete s.start; delete s.end;
    renderSnippetCard(s);
    mount(s);
    bringToFront(s);
    renderHost(f);
    fl = s;
  }
  fl.el.animate([{ opacity: 0.4, transform: 'scale(.98)' }, { opacity: 1, transform: 'none' }], { duration: 200, easing: 'ease-out' });
  renderLinks();
  S.animating = false;
  hooks.onDirty?.();
  scheduleSave();
  return fl;
}

// =====================================================================
// live drag: lines part for the dragged block in real time and close up behind it.
// The dragged block slides *under* the target file, whose lines become opaque strips
// that glide over it, with a little breathing room above and below the gap.
// =====================================================================
let live = null;

function prepLive(f) {
  if (live.files.has(f)) return;
  live.files.add(f);
  const r = f.linesEl.getBoundingClientRect();
  live.tops.set(f, toWorld(r.left, r.top, paneOf(f)).y);
  f.sel = null; paintSel(f); hideIndicators(f);
  f.linesEl.style.height = f.lines.length * LH + 'px';
  void f.linesEl.offsetHeight;
  f.el.classList.add('live');
}
const liftedIn = f => (live.src && live.src.f === f && !live.refMode ? live.src : null);
/** {px, lines}: how far original line i is displaced right now, and by how many whole lines in the final result. */
function liveOffset(f, i) {
  const src = liftedIn(f);
  let px = 0, lines = 0, rank = i;
  if (src && i > src.end) { px -= live.n * LH; lines -= live.n; rank -= live.n; }
  if (live.over === f && rank >= live.at) { px += live.n * LH + (live.pad ? 2 * GP : 0); lines += live.n; }
  return { px, lines };
}
function layoutLive(f) {
  const src = liftedIn(f);
  const kids = f.linesEl.children;
  for (let i = 0; i < kids.length; i++) {
    const hide = src && i >= src.start && i <= src.end;
    kids[i].style.opacity = hide ? '0' : '';
    const o = hide ? { px: 0, lines: 0 } : liveOffset(f, i);
    kids[i].style.transform = o.px ? `translateY(${o.px}px)` : '';
    // renumber so the gutter always shows where each line will end up
    if (!hide) kids[i].firstChild.textContent = String(i + 1 + o.lines);
  }
  const over = live.over === f;
  const total = f.lines.length - (src ? live.n : 0) + (over ? live.n : 0);
  f.linesEl.style.height = Math.max(1, total) * LH + (over && live.pad ? 2 * GP : 0) + 'px';
  f.el.classList.toggle('drop-target', over);
  for (const s of regionsOf(f)) {
    const gone = src && (live.region ? live.region === s : s.start >= src.start && s.end <= src.end);
    const o = gone ? { px: 0 } : liveOffset(f, s.start);
    f.el.querySelectorAll(`[data-sid="${s.id}"]`).forEach(e => {
      e.style.transform = o.px ? `translateY(${o.px}px)` : '';
      e.style.opacity = gone ? '0' : '';
      if (e.classList.contains('region')) e.classList.toggle('referenced', (live.refMode && live.region === s) || isReferenced(s));
    });
  }
}
/**
 * Re-evaluate where the dragged block would land. linesTop = world y of the dragged block's first line,
 * elTop = world y of the dragged element. While docked, the block snaps (with a short glide) into the
 * padded slot so it always has the same breathing room above and below.
 */
function liveHover(pt, linesTop, alt, P, elTop) {
  const f = live.lock || (live.src || !alt ? fileUnder(pt.x, pt.y, P, null, !!live.src) : null);
  let at = -1;
  if (f) {
    prepLive(f);
    const remaining = f.lines.length - (liftedIn(f) ? live.n : 0);
    at = clamp(Math.round((linesTop - live.tops.get(f)) / LH), 0, remaining);
  }
  if (f !== live.over || at !== live.at) {
    const prev = live.over;
    live.over = f; live.at = at;
    if (prev && prev !== f) layoutLive(prev);
    if (f) layoutLive(f);
    if (live.src && live.src.f !== f && live.src.f !== prev) layoutLive(live.src.f);
    if (f !== prev) {
      live.ghost?.classList.toggle('docked', !!f);
      if (f) { S.zTop += 2; live.ghost.style.zIndex = String(S.zTop - 1); f.el.style.zIndex = String(S.zTop); }
      else live.ghost.style.zIndex = String(++S.zTop + 90000);
    }
    // the dragged block's own gutter shows the line numbers it would get
    const nos = live.ghost.querySelectorAll('.lines .ln .no');
    nos.forEach((n, k) => { n.textContent = String((f ? at : 0) + k + 1); });
  }
  // snap vertically into the slot (top margin GP), keep following the pointer horizontally
  if (f) {
    const slotTop = live.tops.get(f) + at * LH + (live.pad ? GP : 0);
    live.ghost.style.transform = `translateY(${slotTop - linesTop}px)`;
  } else live.ghost.style.transform = '';
  live.elTop = elTop;
}
function setRefMode(on) {
  if (!live.src || live.refMode === on) return;
  live.refMode = on;
  live.over = null; live.at = -1; // lifting changes the meaning of positions in the source file
  layoutLive(live.src.f);
  live.ghost?.classList.toggle('ref-ghost', on);
}
function finishLive() {
  if (!live) return;
  for (const f of live.files) {
    f.el.classList.remove('live', 'drop-target');
    f.linesEl.style.height = '';
    if (hostOf(f.id)) renderHost(f);
  }
  if (live.ghost) { live.ghost.classList.remove('docked', 'docking', 'ref-ghost'); live.ghost.style.transform = ''; }
  live = null;
}
/** Close the breathing room and slide an element (a ghost or a snippet card) into the exact slot. */
function slideInto(el, f, at, linesOff, gutterFrom) {
  // bake the snap offset into `top` so the glide starts exactly where the block is drawn
  const curTop = el.getBoundingClientRect().top;
  const P = paneOf(f);
  el.style.transition = 'none';
  el.style.transform = '';
  el.style.top = toWorld(0, curTop, P).y + 'px';
  void el.offsetHeight;
  live.pad = false;
  layoutLive(f);
  const y = live.tops.get(f) + at * LH - linesOff;
  const x = f.x + (gutterW(f.lines.length) - gutterFrom);
  el.style.transition = 'left .2s cubic-bezier(.2,.8,.3,1), top .2s cubic-bezier(.2,.8,.3,1)';
  el.style.left = x + 'px'; el.style.top = y + 'px';
  el.classList.add('docking');
  return wait(210).then(() => { el.style.transition = ''; });
}

/** After a drop: no flash, the snippet's tabs and region marker just fade in. */
function fadeInTabs(s) {
  const f = hostOf(s.fileId);
  f?.el.querySelectorAll(`.mtab[data-sid="${s.id}"], .region[data-sid="${s.id}"]`).forEach(e => e.classList.add('fade-in'));
}

function makeGhost(s, f) {
  const P = paneOf(f);
  const el = h('div', { class: 'card snippet-card ghost docked' });
  el.style.setProperty('--rc', s.color);
  const gut = gutterW(f.lines.length);
  el.style.setProperty('--gutter', gut + 'px');
  el.style.width = f.w + 'px';
  const rows = [...f.linesEl.children].slice(s.start, s.end + 1).map(k => { const c = k.cloneNode(true); c.className = 'ln'; c.removeAttribute('style'); return c.outerHTML; }).join('');
  el.innerHTML = `<div class="card-head"><span class="ico"></span><span class="title">${esc(s.title)}</span></div><div class="card-body"><div class="lines">${rows}</div></div>`;
  P.world.append(el);
  const linesOff = el.querySelector('.card-body').offsetTop + el.querySelector('.lines').offsetTop;
  const srcTop = toWorld(0, f.linesEl.children[s.start].getBoundingClientRect().top, P).y;
  const g = { el, P, x: f.x, y: srcTop - linesOff, linesOff, gutter: gut };
  el.style.left = g.x + 'px'; el.style.top = g.y + 'px';
  return g;
}

/**
 * Drag an anchored snippet: up/down inside its file, into another file (any pane), or out onto the canvas.
 * Alt = reference (on canvas) / copy (into a file). `via`: 'tab' | 'bar' | 'lines' | 'shift'.
 * 'shift' (Shift+drag on the snippet's lines) grabs the snippet without selecting it first; it then moves
 * like any other snippet drag, inside the file or out of it. Released without dragging, it is a plain Shift+click.
 */
function startRegionDrag(e, s, via) {
  const f = hostOf(s.fileId);
  if (!f) return;
  const P0 = paneOf(f);
  const p0 = toWorld(e.clientX, e.clientY, P0);
  let g = null;
  if (via === 'bar') { S.hl = null; select([s.id]); }
  drag = { kind: 'region', cancel: () => { if (!g) return; g.el.remove(); finishLive(); if (S.shared) makeHost(P0); setActive(P0); } };
  drag.refresh = () => g && lastPointer && regionMove(lastPointer);
  const regionMove = ev => {
    const P = paneAt(ev.clientX, ev.clientY) || g?.P || P0;
    const p = toWorld(ev.clientX, ev.clientY, P);
    if (!g) {
      if (Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) < 5 || S.animating || S.editing || live) return;
      hideSelTools();
      clearAllSel();
      g = makeGhost(s, f);
      live = { n: s.end - s.start + 1, src: { f, start: s.start, end: s.end }, region: s, ghost: g.el, refMode: false, over: f, at: s.start, pad: true, files: new Set(), tops: new Map() };
      prepLive(f);
      layoutLive(f);
      f.el.style.zIndex = String(S.zTop += 2); g.el.style.zIndex = String(S.zTop - 1);
      g.dx = p0.x - g.x; g.dy = p0.y - g.y; // anchor to where the mouse went down (world units)
    }
    if (P !== g.P) { g.P = P; if (S.shared) makeHost(P); else P.world.append(g.el); setActive(P); }
    g.x = p.x - g.dx; g.y = p.y - g.dy;
    g.el.style.left = g.x + 'px'; g.el.style.top = g.y + 'px';
    setRefMode(ev.altKey);
    // Judge 'over a file' by the dragged block's body, not the pointer (the tab handle sits outside the file).
    liveHover({ x: g.x + Math.min(f.w * 0.3, 160), y: p.y }, g.y + g.linesOff, ev.altKey, g.P, g.y);
  };
  drag.abort = capture(e, regionMove, async () => {
    if (!g) {
      if (via === 'tab') toggleDrawer(s);
      else if (via === 'shift') toggleSelect(f.id); // Shift+click: the selection changes on release, never on a drag
      // a plain click on a selected snippet keeps it selected and highlights it
      else if (via === 'lines' || via === 'bar') { S.hl = s.id; paintSelection(); }
      return;
    }
    await dropRegion(s, f, g);
  });
}

async function dropRegion(s, f, g) {
  const L = live, n = L.n;
  S.animating = true;
  try {
    if (L.over) {
      const f2 = L.over, at = L.at;
      await slideInto(g.el, f2, at, g.linesOff, g.gutter);
      const lines = f.lines.slice(s.start, s.end + 1);
      if (L.refMode) {                       // Alt-drop into a file: insert a copy
        pushUndo();
        spliceFile(f2, at, 0, lines);
        const c = createAnchored(f2, at, at + n - 1, { title: s.title, noRender: true });
        finishLive(); selectLanded(c); fadeInTabs(c);
      } else if (f2 === f && at === s.start) {
        finishLive(); selectLanded(s);
      } else {                               // move the lines
        pushUndo();
        const start = s.start;
        s.fileId = null;
        spliceFile(f, start, n, []);
        spliceFile(f2, at, 0, lines);
        Object.assign(s, { fileId: f2.id, start: at, end: at + n - 1 });
        finishLive(); selectLanded(s); fadeInTabs(s);
      }
    } else if (L.refMode) {                  // Alt-drop on canvas: a reference card
      finishLive();
      makeReference(s, g.x, g.y, g.P);
    } else {                                 // drop on canvas: slide it out, the file closes up behind it
      pushUndo();
      const lines = f.lines.slice(s.start, s.end + 1);
      unlinkRefs(s);
      const start = s.start;
      s.fileId = null;
      s._drawer?.el.remove(); s._draftUI?.el.remove();
      spliceFile(f, start, n, []);
      Object.assign(s, { lines, x: g.x, y: g.y, w: f.w, target: null, pane: g.P.id, lang: hostPath(f) });
      delete s.start; delete s.end; delete s.origin;
      renderSnippetCard(s); mount(s); bringToFront(s);
      finishLive();
      selectLanded(s);
    }
  } finally {
    g.el.remove();
    S.animating = false;
  }
  renderLinks(); hooks.onDirty?.(); scheduleSave();
  replayPendingPress();
}

/**
 * Drag a snippet's colour bar to grow or shrink it line by line. The edge nearest the grab point moves
 * (for a one-line snippet, the direction of the drag decides). Other snippets the new range runs into
 * are shown as doomed while dragging and dissolved (their code stays) on release. A click just selects it.
 */
function startRegionResize(e, s, edge) {
  const f = hostOf(s.fileId);
  if (!f || S.animating || live) return;
  const orig = { start: s.start, end: s.end };
  let moved = false, victims = [];
  hideSelTools();
  const paintVictims = () => {
    for (const o of regionsOf(f)) f.regionsEl.querySelector(`.region[data-sid="${o.id}"]`)?.classList.toggle('doomed-region', victims.includes(o));
  };
  drag = {
    kind: 'resize-region',
    cancel: () => { if (!moved) return; Object.assign(s, orig); S.undo.pop(); renderRegions(f); refreshRefs(f); },
  };
  drag.abort = capture(e, ev => {
    if (!moved && Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) < 4) return;
    if (!moved) { moved = true; pushUndo(); select([s.id]); S.hl = null; }
    const line = lineAt(f, ev.clientY);
    if (!edge) edge = line < s.start ? 'start' : 'end';
    if (edge === 'start') s.start = Math.min(line, s.end); else s.end = Math.max(line, s.start);
    // anything the snippet now overlaps — except a snippet that fully contains it — gets dissolved on release
    victims = regionsOf(f).filter(o => o !== s && o.start <= s.end && o.end >= s.start && !(o.start <= s.start && o.end >= s.end));
    renderRegions(f);
    paintVictims();
  }, () => {
    if (!moved) { select([s.id]); S.hl = s.id; paintSelection(); return; }
    if (s.start === orig.start && s.end === orig.end) { S.undo.pop(); renderRegions(f); return; }
    for (const v of victims) removeSnippet(v, true);
    renderRegions(f);
    refreshRefs(f);
    if (victims.length) toast(`Dissolved ${plural(victims.length, 'overlapping snippet')} (the code stays)`, 'info', 2500);
    scheduleSave();
  });
}

/** Remove the snippet markings (not the code) from one file, or from every file on the canvas. */
export function dissolveAll(f) {
  const list = f ? regionsOf(f) : [...S.snippets.values()].filter(x => x.fileId);
  if (!list.length) { toast(f ? `${basename(f.path)} has no snippets.` : 'No snippets in any file.'); return; }
  pushUndo();
  for (const x of list) removeSnippet(x, true);
  for (const ff of f ? [f] : S.files.values()) renderRegions(ff);
  toast(`Dissolved ${plural(list.length, 'snippet')} — the code stays. Ctrl+Z to undo.`, 'ok', 3000);
  scheduleSave();
}

/**
 * Shift+drag a plain line selection up or down inside its own file. The lines part around it live,
 * nothing becomes a snippet, and the wheel scrolls vertically while it's going on.
 */
function startShiftLineDrag(e, f, sr) {
  if (S.animating || live) return;
  const P = paneOf(f);
  const p0 = toWorld(e.clientX, e.clientY, P);
  const n = sr.b - sr.a + 1;
  const pseudo = { id: '__sel', title: plural(n, 'line'), color: 'var(--accent)', start: sr.a, end: sr.b };
  let g = null;
  hideSelTools();
  const step = ev => {
    const p = toWorld(ev.clientX, ev.clientY, P);
    if (!g) {
      if (Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) < 4) return;
      g = makeGhost(pseudo, f);
      g.el.classList.add('sel-ghost');
      live = { n, src: { f, start: sr.a, end: sr.b }, region: null, ghost: g.el, refMode: false, over: f, at: sr.a, pad: true, lock: f, files: new Set(), tops: new Map() };
      f.sel = null;
      prepLive(f);
      layoutLive(f);
      f.el.style.zIndex = String(S.zTop += 2); g.el.style.zIndex = String(S.zTop - 1);
      g.dy = p0.y - g.y;
    }
    g.y = p.y - g.dy;            // vertical only — the block stays in its column
    g.el.style.top = g.y + 'px';
    liveHover({ x: f.x + 10, y: p.y }, g.y + g.linesOff, false, P, g.y);
  };
  drag = {
    kind: 'shiftlines',
    refresh: () => lastPointer && step(lastPointer),
    cancel: () => { if (!g) return; g.el.remove(); finishLive(); f.sel = { a: sr.a, b: sr.b }; paintSel(f); },
  };
  drag.abort = capture(e, step, async () => {
    if (!g) { f.sel = { a: sr.a, b: sr.b }; paintSel(f); showSelTools(f); return; }
    const at = live.at;
    S.animating = true;
    try {
      await slideInto(g.el, f, at, g.linesOff, g.gutter);
      if (at !== sr.a) {
        pushUndo();
        const lines = f.lines.slice(sr.a, sr.b + 1);
        // snippets wholly inside the moved block travel with it
        const riders = regionsOf(f).filter(s => s.start >= sr.a && s.end <= sr.b).map(s => ({ s, d: s.start - sr.a, len: s.end - s.start }));
        riders.forEach(r => { r.s.fileId = null; });
        spliceFile(f, sr.a, n, []);
        spliceFile(f, at, 0, lines);
        riders.forEach(r => Object.assign(r.s, { fileId: f.id, start: at + r.d, end: at + r.d + r.len }));
      }
      finishLive();
      f.sel = { a: at, b: at + n - 1 };
      paintSel(f);
    } finally {
      g.el.remove();
      S.animating = false;
    }
    hooks.onDirty?.(); scheduleSave();
  });
}

/** A floating snippet card was dropped into the open slot of file f. */
async function dropSnippet(s, f, at, linesOff, origPos) {
  S.animating = true;
  await slideInto(s.el, f, at, linesOff, 44);
  const cur = { x: s.x, y: s.y, pane: s.pane };
  Object.assign(s, origPos); pushUndo(); Object.assign(s, cur);
  materializeRef(s);
  const n = s.lines.length, lines = s.lines;
  s.el.remove(); s.el = null;
  s._drawer?.el.remove(); s._draftUI?.el.remove();
  spliceFile(f, at, 0, lines);
  Object.assign(s, { fileId: f.id, start: at, end: at + n - 1, target: null });
  delete s.lines; delete s.x; delete s.y; delete s.w; delete s.origin; delete s.pane;
  finishLive();
  selectLanded(s);
  fadeInTabs(s);
  S.animating = false;
  renderLinks(); hooks.onDirty?.(); scheduleSave();
  replayPendingPress();
}

// ---------- references: a card that mirrors lines still living in a file ----------
function refSource(s) { const r = s?.ref && S.snippets.get(s.ref); return r && r.fileId && !!hostOf(r.fileId) ? r : null; }
function isReferenced(s) { for (const x of S.snippets.values()) if (x.ref === s.id) return true; return false; }
function refreshRefs(f) { for (const x of S.snippets.values()) { const r = refSource(x); if (r && r.fileId === f.id && x.el) renderSnippetCard(x); } }
function materializeRef(s) {
  const r = refSource(s);
  if (!r) return;
  s.lines = snippetLines(r).slice();
  delete s.ref;
  renderRegions(hostOf(r.fileId));
}
export function makeReference(s, x, y, P) {
  const f = hostOf(s.fileId);
  const r = createFloating({ title: s.title, lines: snippetLines(s), ref: s.id, x, y, w: f.w, color: s.color, pane: (P || paneOf(f)).id });
  renderRegions(f);
  r.el.animate([{ opacity: 0.3 }, { opacity: 1 }], { duration: 200 });
  return r;
}

export async function applyToTarget(s) {
  const t = s.target;
  if (!t?.file) return;
  const f = fileByPath(t.file) || await openFile(t.file, null, { silent: true, pane: paneOf(s).id });
  if (!f) { toast(`Target file ${t.file} isn't in the project.`, 'error'); return; }
  if (t.replaceStart) {
    const a = clamp(t.replaceStart - 1, 0, f.lines.length - 1), b = clamp(t.replaceEnd - 1, a, f.lines.length - 1);
    await mergeSnippet(s, f, a, b - a + 1);
  } else {
    await mergeSnippet(s, f, clamp((t.insertAt || f.lines.length + 1) - 1, 0, f.lines.length));
  }
}

// =====================================================================
// drafts coming back from the LLM as new cards
// =====================================================================
export function spawnDrafts(drafts, near) {
  const ids = [];
  const nearCard = near && (near.fileId ? hostOf(near.fileId) : near);
  let k = 0;
  for (const d of drafts) {
    const lines = d.code.replace(/\n$/, '').split('\n');
    const target = d.file ? { file: d.file, insertAt: d.insertAt, replaceStart: d.replaceStart, replaceEnd: d.replaceEnd } : null;
    let pos, P = nearCard ? paneOf(nearCard) : AP();
    const tf = d.file && fileByPath(d.file);
    if (tf) { P = paneOf(tf); pos = { x: tf.x + tf.w + 480, y: tf.y + BODY_TOP + ((d.replaceStart || d.insertAt || 1) - 1) * LH - 40 }; }
    else if (nearCard) pos = { x: nearCard.x + nearCard.w + 480, y: nearCard.y + (near.fileId ? BODY_TOP + near.start * LH : 0) + k * 40 };
    const lang = d.lang || (nearCard && (nearCard.type === 'file' ? nearCard.path : snippetPath(near))) || undefined;
    const s = createFloating({ title: d.title, lines, target, lang, pane: P.id, ...(pos || {}) });
    if (pos) { const p = nudgeFree(s); s.x = p.x; s.y = p.y; positionCard(s); }
    s.el.animate([{ opacity: 0, transform: 'translateY(10px) scale(.97)' }, { opacity: 1, transform: 'none' }], { duration: 260, easing: 'ease-out' });
    ids.push(s.id);
    k++;
    if (d.file && !tf) {
      openFile(d.file, null, { silent: true, pane: P.id }).then(f => {
        if (!f || !S.snippets.has(s.id) || (!S.shared && paneOf(f) !== paneOf(s))) return;
        s.x = f.x + f.w + 480; s.y = f.y + BODY_TOP + ((d.replaceStart || d.insertAt || 1) - 1) * LH - 40;
        const p = nudgeFree(s); s.x = p.x; s.y = p.y;
        positionCard(s); renderLinks();
      });
    }
  }
  renderLinks();
  if (ids.length) toast(`${plural(ids.length, 'snippet')} drafted on the canvas`, 'ok', 2500);
  return ids;
}

function nudgeFree(s) {
  const P = paneOf(s);
  const others = allCards().filter(c => c !== s && inPane(c, P));
  let { x, y } = s;
  const hgt = cardH(s);
  for (let i = 0; i < 60; i++) {
    const hit = others.find(c => x < c.x + c.w && x + s.w > c.x && y < c.y + cardH(c) && y + hgt > c.y);
    if (!hit) break;
    y = hit.y + cardH(hit) + 24;
  }
  return { x, y };
}

// connector lines: drafted snippet → its target line (only when both are in the same pane)
export function renderLinks() {
  const byPane = new Map([...panes.keys()].map(id => [id, []]));
  for (const s of S.snippets.values()) {
    if (s.fileId || !s.target?.file || !s.el) continue;
    const f = fileByPath(s.target.file);
    if (!f || paneOf(f) !== paneOf(s)) continue;
    const line = (s.target.replaceStart || s.target.insertAt || 1) - 1;
    const x0 = f.x + f.w, y0 = f.y + BODY_TOP + line * LH + LH / 2;
    const toLeft = s.x > x0;
    const x1 = toLeft ? s.x : s.x + s.w, y1 = s.y + 17;
    const cx = Math.max(60, Math.abs(x1 - x0) / 2);
    byPane.get(paneOf(s).id)?.push(`<path d="M${x0},${y0} C${x0 + cx},${y0} ${x1 - (toLeft ? cx : -cx)},${y1} ${x1},${y1}" style="stroke:${s.color}"/>`);
  }
  if (S.shared) { if (W) W._links.innerHTML = [...byPane.values()].flat().join(''); return; }
  for (const [id, paths] of byPane) panes.get(id).links.innerHTML = paths.join('');
}

// =====================================================================
// explain
// =====================================================================
export function explainFile(path) {
  return new Promise(async resolve => {
    if (S.explaining.has(path)) { toast(`Already explaining ${path}`); return resolve(false); }
    const f = fileByPath(path) || await openFile(path, null, {});
    if (!f) return resolve(false);
    pushUndo();
    for (const s of regionsOf(f)) if (s.explained) removeSnippet(s, true);
    renderRegions(f);
    const id = uid('ex');
    S.explaining.set(path, id);
    f.el.querySelector('.tools .icon-btn')?.classList.add('spin');
    hooks.onExplain?.();
    let made = 0;
    const apply = segs => {
      const ff = fileByPath(path);
      if (!ff) return;
      for (; made < segs.length; made++) {
        const g = segs[made];
        const a = clamp(g.start - 1, 0, ff.lines.length - 1), b = clamp(g.end - 1, a, ff.lines.length - 1);
        createAnchored(ff, a, b, { title: g.title, explained: true, noRender: true, chat: [{ role: 'note', title: g.title, content: g.explanation }] });
      }
      renderRegions(ff);
    };
    const off = stream(id, m => {
      if (m.type === 'explainProgress') apply(m.segments);
      if (m.type === 'explainDone' || m.type === 'explainError') {
        off();
        if (m.type === 'explainDone') apply(m.segments);
        else toast(`Explain ${path}: ${m.error}`, 'error', 7000);
        S.explaining.delete(path);
        fileByPath(path)?.el.querySelector('.tools .icon-btn')?.classList.remove('spin');
        hooks.onExplain?.();
        scheduleSave();
        resolve(m.type === 'explainDone');
      }
    });
    send({ type: 'explain', id, path, text: f.lines.join('\n') });
  });
}
export function cancelExplain(path) { const id = S.explaining.get(path); if (id) send({ type: 'cancel', id }); }

// =====================================================================
// edit mode (double click)
// =====================================================================
function enterEdit(c, line = 0) {
  if (S.editing) exitEdit();
  const rsrc = refSource(c);
  if (rsrc) c = rsrc;
  if (c.fileId) { const f = hostOf(c.fileId); line = c.start; c = f; }
  S.editing = c;
  hideSelTools();
  c.el.classList.add('editing');
  const lines = c.lines;
  const gut = h('div', { class: 'gut' });
  const ta = h('textarea', { spellcheck: 'false', wrap: 'off' });
  ta.value = lines.join('\n');
  // Syntax colours while editing: a highlighted copy sits under a textarea whose own text is transparent.
  const path = hostPath(c);
  const hlEl = h('div', { class: 'ed-hl', 'aria-hidden': 'true' });
  let hlRaf = 0;
  const paintHl = () => {
    hlRaf = 0;
    hlEl.innerHTML = highlightLines(ta.value.split('\n'), path).map(x => `<div class="ed-ln">${x || ' '}</div>`).join('');
  };
  let pillRaf = 0;
  const pills = () => { if (!pillRaf) pillRaf = requestAnimationFrame(() => { pillRaf = 0; if (S.editing === c) paintPillars(c); }); };
  const sync = () => {
    const ls = ta.value.split('\n');
    const n = ls.length;
    gut.textContent = Array.from({ length: n }, (_, i) => i + 1).join('\n');
    ta.style.height = n * LH + 'px';
    // as wide as the widest line, so the caret never scrolls the text sideways; the card clips it
    ta.style.minWidth = ls.reduce((m, l) => Math.max(m, visLen(l)), 0) * charW + 24 + 'px';
    pills();
    if (n <= 3000) paintHl(); // colours keep up keystroke-for-keystroke
    else if (!hlRaf) hlRaf = requestAnimationFrame(paintHl);
  };
  sync();
  paintHl();
  ta.addEventListener('scroll', () => { hlEl.style.transform = `translate(${-ta.scrollLeft}px, ${-ta.scrollTop}px)`; });
  const stack = h('div', { class: 'ed-stack' }, hlEl, ta);
  const wrap = h('div', { class: 'editor-wrap' }, gut, stack);
  c.linesEl.style.display = 'none';
  if (c.regionsEl) c.regionsEl.style.display = 'none';
  c.bodyEl.prepend(wrap);
  c._editor = { wrap, ta, hlEl };
  ta.addEventListener('input', sync);
  for (const ev of ['keyup', 'pointerup', 'select']) ta.addEventListener(ev, pills);
  ta.addEventListener('pointerdown', e => e.stopPropagation());
  ta.addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key === 'Escape') { e.preventDefault(); exitEdit(); }
    else if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key.toLowerCase() === 'f' || e.key.toLowerCase() === 'h')) {
      // find works on the card's committed text, so the edit is finished first; a one-line selection seeds the query
      e.preventDefault();
      const seed = ta.value.slice(ta.selectionStart, ta.selectionEnd);
      const card = c;
      exitEdit();
      openFind(card, e.key.toLowerCase() === 'h', seed && !seed.includes('\n') ? seed : '');
    }
    else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); const f = c; exitEdit(); if (f.type === 'file') saveFile(f); }
    else if (e.key === 'Tab') {
      e.preventDefault();
      const a = ta.selectionStart, b = ta.selectionEnd;
      ta.setRangeText('    ', a, b, 'end'); sync();
    }
  });
  ta.focus({ preventScroll: true });
  const off = lines.slice(0, line).reduce((n, l) => n + l.length + 1, 0);
  ta.setSelectionRange(off, off);
  resetNativeScroll();
  stickHeader(c);
  pills();
}

export function exitEdit() {
  const c = S.editing;
  if (!c) return;
  S.editing = null;
  stickHeader(c);
  const next = c._editor.ta.value.split('\n');
  c._editor.wrap.remove();
  c._editor = null;
  c.el.classList.remove('editing');
  c.linesEl.style.display = '';
  if (c.regionsEl) c.regionsEl.style.display = '';
  if (c.type === 'file') {
    const before = snapshot();
    if (setFileLines(c, next)) { S.undo.push(before); S.redo = []; }
    renderHost(c);
    hooks.onDirty?.();
  } else {
    if (next.join('\n') !== c.lines.join('\n')) { pushUndo(); if (c.ref) c.lines = next; else setFileLines(c, next); }
    renderSnippetCard(c);
  }
  renderLinks();
  scheduleSave();
}

// =====================================================================
// pointer interaction
// =====================================================================
let drag = null;
let spaceDown = false;
let lastPointer = null;

function setSpace(on) {
  spaceDown = on;
  document.body.classList.toggle('space-held', on);
}

function cardFromEl(el) {
  const ce = el.closest?.('.card');
  if (!ce || ce.classList.contains('ghost')) return null;
  const id = ce.dataset.id;
  return S.files.get(id) || S.snippets.get(id);
}

function fileUnder(wx, wy, P, exclude, snippets) {
  let best = null, bz = -1;
  for (const f of snippets ? allCards().filter(c => c.type === 'file' || (ownLines(c) && c.el)) : S.files.values()) {
    if (f === exclude || !inPane(f, P)) continue;
    if (wx >= f.x && wx <= f.x + f.w && wy >= f.y && wy <= f.y + cardH(f)) {
      const z = +f.el.style.zIndex || 0;
      if (z > bz) { bz = z; best = f; }
    }
  }
  return best;
}

function select(ids, additive) {
  if (!additive) S.selected.clear();
  for (const id of ids) S.selected.add(id);
  paintSelection();
}
function toggleSelect(id) { S.selected.has(id) ? S.selected.delete(id) : S.selected.add(id); paintSelection(); }
export function clearSelection() { S.selected.clear(); S.hl = null; paintSelection(); }
function paintSelection() {
  for (const c of allCards()) c.el?.classList.toggle('selected', S.selected.has(c.id));
  for (const f of S.files.values()) f.regionsEl?.querySelectorAll('.region').forEach(r => {
    r.classList.toggle('selected', S.selected.has(r.dataset.sid));
    // the colour highlight only follows a deliberate click (S.hl), never drags or drops
    r.classList.toggle('picked', S.hl === r.dataset.sid && S.selected.has(r.dataset.sid));
  });
  hooks.onSelection?.();
}

/** A selected snippet region under the pointer can be grabbed from anywhere on its lines. */
function selectedRegionAt(f, clientY) {
  const line = lineAt(f, clientY);
  return regionsOf(f).filter(s => S.selected.has(s.id) && line >= s.start && line <= s.end).sort((a, b) => (a.end - a.start) - (b.end - b.start))[0];
}

/** The innermost snippet region under the pointer, selected or not (Shift+drag grabs it). */
function regionAt(f, clientY) {
  const line = lineAt(f, clientY);
  return regionsOf(f).filter(s => line >= s.start && line <= s.end).sort((a, b) => (a.end - a.start) - (b.end - b.start))[0];
}

/** A press that arrives while a drop is still sliding into place is replayed once the drop settles. */
let pendingPress = null;
function replayPendingPress() {
  const pp = pendingPress;
  pendingPress = null;
  if (!pp || !(lastPointer?.buttons & 1)) return;
  const t = document.elementFromPoint(pp.e.clientX, pp.e.clientY);
  if (!t) return;
  onPointerDown({
    target: t, button: 0, buttons: 1, pointerId: pp.e.pointerId, clientX: pp.e.clientX, clientY: pp.e.clientY,
    ctrlKey: pp.e.ctrlKey, shiftKey: pp.e.shiftKey, altKey: pp.e.altKey, metaKey: pp.e.metaKey, preventDefault() {},
  }, pp.P);
}

function onPointerDown(e, P) {
  hideMenu();
  lastPointer = e;
  if (S.animating && e.button === 0) { pendingPress = { e, P }; return; }
  setActive(P);
  if (S.pick) { if (e.button === 0) finishPick(e); else cancelPick(); return; }
  const t = e.target;
  if (S.editing && !S.editing.el.contains(t)) exitEdit();
  if (t.closest('.sel-tools') || t.closest('.drawer') || t.closest('.title-input') || t.closest('.pane-bar')) return;

  const card = cardFromEl(t);
  // Space + drag: move whatever card is under the pointer, wherever you grabbed it; on empty canvas it pans.
  if (e.button === 0 && spaceDown && !(card && S.editing === card)) {
    if (card) {
      const c = card;
      bringToFront(c); clearAllSel();
      if (!S.selected.has(c.id)) select([c.id]);
      startMove(e, c);
    } else startPan(e, P);
    return;
  }
  if (e.button === 1) { startPan(e, P); return; }
  if (e.button !== 0) return;

  if (!card) {
    clearAllSel();
    if (!e.ctrlKey && !e.shiftKey) clearSelection();
    startMarquee(e, P);
    return;
  }
  bringToFront(card);
  if (S.editing === card) return;

  if (t.closest('.resize') || t.closest('.edge-r')) { startResize(e, card); return; }
  if (t.closest('.edge-l')) { startResize(e, card, true); return; }
  const rbar = t.closest('.rbar');
  if (rbar) {
    const s = S.snippets.get(rbar.closest('.region').dataset.sid);
    const handle = t.closest('.rh');
    if (handle) startRegionResize(e, s, handle.classList.contains('top') ? 'start' : 'end');
    else startRegionDrag(e, s, 'bar');
    return;
  }

  const mod = e.ctrlKey || e.metaKey || e.shiftKey;
  const inHead = t.closest('.card-head');
  // files and snippets alike: drag selects lines, Shift+drag moves them; the header (or Space) moves the card
  if ((card.type === 'file' || ownLines(card)) && !inHead && t.closest('.lines')) {
    const sr = selRange(card);
    if (e.shiftKey && sr) {
      const line = lineAt(card, e.clientY);
      if (line >= sr.a && line <= sr.b) { startShiftLineDrag(e, card, sr); return; }
    }
    if (e.shiftKey) {
      const held = regionAt(card, e.clientY);
      if (held) { startRegionDrag(e, held, 'shift'); return; }
    }
    if (mod) { toggleSelect(card.id); return; }
    const reg = selectedRegionAt(card, e.clientY);
    if (reg) { startRegionDrag(e, reg, 'lines'); return; }
    startLineSelect(e, card);
    return;
  }
  // header (or snippet body) → select + move
  if (mod) toggleSelect(card.id);
  else if (!S.selected.has(card.id)) select([card.id]);
  clearAllSel();
  startMove(e, card);
}

/**
 * Route all pointer events of a drag to move/up. Returns an abort() that ends the drag without calling up
 * (used by Escape); the caller puts its own undo logic in drag.cancel.
 */
function capture(e, move, up) {
  const el = root; // capture on the pane container so drags can cross between split views
  try { el.setPointerCapture(e.pointerId); } catch { /* synthetic events */ }
  const mv = ev => { lastPointer = ev; move(ev); };
  const off = () => {
    el.removeEventListener('pointermove', mv);
    el.removeEventListener('pointerup', done);
    el.removeEventListener('pointercancel', done);
    try { el.releasePointerCapture(e.pointerId); } catch { /* */ }
  };
  const done = ev => { off(); drag = null; up(ev); };
  el.addEventListener('pointermove', mv);
  el.addEventListener('pointerup', done);
  el.addEventListener('pointercancel', done);
  return off;
}

/** Escape during a drag puts everything back the way it was. */
function cancelDrag() {
  if (!drag) return false;
  const d = drag;
  drag = null;
  d.abort?.();
  d.cancel?.();
  return true;
}

function startPan(e, P) {
  e.preventDefault();
  P.el.classList.add('panning');
  const sx = e.clientX, sy = e.clientY, ox = P.view.x, oy = P.view.y;
  drag = { kind: 'pan', cancel: () => { P.view.x = P.target.x = ox; P.view.y = P.target.y = oy; applyView(P); P.el.classList.remove('panning'); } };
  drag.abort = capture(e, ev => {
    P.view.x = P.target.x = ox + ev.clientX - sx;
    P.view.y = P.target.y = oy + ev.clientY - sy;
    applyView(P);
  }, () => { P.el.classList.remove('panning'); scheduleSave(); });
}

function startMarquee(e, P) {
  const r = P.el.getBoundingClientRect();
  const sx = e.clientX - r.left, sy = e.clientY - r.top;
  const base = new Set(e.ctrlKey || e.shiftKey ? S.selected : []);
  let moved = false;
  drag = { kind: 'marquee', cancel: () => { P.marquee.style.display = 'none'; S.selected = new Set(base); paintSelection(); } };
  drag.abort = capture(e, ev => {
    const x = clamp(ev.clientX - r.left, 0, r.width), y = clamp(ev.clientY - r.top, 0, r.height);
    if (!moved && Math.hypot(x - sx, y - sy) < 4) return;
    moved = true;
    Object.assign(P.marquee.style, { display: 'block', left: Math.min(x, sx) + 'px', top: Math.min(y, sy) + 'px', width: Math.abs(x - sx) + 'px', height: Math.abs(y - sy) + 'px' });
    const a = toWorld(Math.min(x, sx) + r.left, Math.min(y, sy) + r.top, P), b = toWorld(Math.max(x, sx) + r.left, Math.max(y, sy) + r.top, P);
    S.selected = new Set(base);
    for (const c of allCards()) if (inPane(c, P) && c.x < b.x && c.x + c.w > a.x && c.y < b.y && c.y + cardH(c) > a.y) S.selected.add(c.id);
    paintSelection();
  }, () => { P.marquee.style.display = 'none'; });
}

/** Width from the right edge or the corner; from the left edge (`left`) the card's left side moves instead. */
function startResize(e, c, left) {
  e.preventDefault();
  const sx = e.clientX, ow = c.w, ox = c.x, z = paneOf(c).view.z;
  drag = { kind: 'resize', cancel: () => { c.w = ow; c.x = ox; positionCard(c); renderLinks(); renderHost(c); } };
  drag.abort = capture(e, ev => {
    const d = (ev.clientX - sx) / z;
    if (left) { c.w = Math.max(240, ow - d); c.x = ox + ow - c.w; } else c.w = Math.max(240, ow + d);
    positionCard(c); markWide(c); renderLinks();
  }, () => { renderHost(c); scheduleSave(); });
}

function startLineSelect(e, f) {
  clearAllSel(f);
  clearSelection();
  const i = lineAt(f, e.clientY);
  if (e.shiftKey && f.sel) f.sel.b = i; else f.sel = { a: i, b: i };
  paintSel(f);
  drag = { kind: 'lines', cancel: () => { f.sel = null; paintSel(f); } };
  drag.abort = capture(e, ev => { f.sel.b = lineAt(f, ev.clientY); paintSel(f); }, () => showSelTools(f));
}

/** Move a card (or the selected group). Cards follow the pointer into other split views. */
function startMove(e, card) {
  let P = paneOf(card);
  const start = toWorld(e.clientX, e.clientY, P);
  const group = (S.selected.has(card.id) ? allCards().filter(c => S.selected.has(c.id)) : [card]);
  const grab = group.map(c => ({ c, dx: start.x - c.x, dy: start.y - c.y, x: c.x, y: c.y, pane: c.pane }));
  const single = group.length === 1 && card.type === 'snippet' ? card : null;
  let moved = false, linesOff = 0, useLive = false;
  const P0 = P;
  drag = {
    kind: 'move',
    cancel: () => {
      group.forEach(c => c.el.classList.remove('dragging'));
      if (useLive) finishLive();
      if (S.shared) makeHost(P0);
      for (const g of grab) {
        const home = panes.get(g.pane) || P0;
        if (!S.shared && g.c.el.parentElement !== home.world) home.world.append(g.c.el);
        g.c.pane = home.id; g.c.x = g.x; g.c.y = g.y; positionCard(g.c);
      }
      setActive(P0);
      renderLinks();
    },
  };
  drag.abort = capture(e, ev => {
    if (!moved && Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) < 3) return;
    const Pn = paneAt(ev.clientX, ev.clientY) || P;
    if (Pn !== P) {
      P = Pn;
      if (S.shared) makeHost(P);
      else for (const g of grab) { g.c.pane = P.id; P.world.append(g.c.el); }
      setActive(P);
    }
    const p = toWorld(ev.clientX, ev.clientY, P);
    if (!moved) {
      group.forEach(c => c.el.classList.add('dragging'));
      if (single && !S.animating && !live) {
        useLive = true;
        live = { n: single.lines.length, src: null, ghost: single.el, over: null, at: -1, pad: true, files: new Set(), tops: new Map() };
        linesOff = single.bodyEl.offsetTop + single.linesEl.offsetTop;
      }
    }
    moved = true;
    for (const g of grab) { g.c.x = p.x - g.dx; g.c.y = p.y - g.dy; positionCard(g.c); }
    renderLinks();
    if (useLive) liveHover({ x: single.x + Math.min(single.w * 0.3, 160), y: p.y }, single.y + linesOff, ev.altKey, P, single.y);
  }, async ev => {
    group.forEach(c => c.el.classList.remove('dragging'));
    if (useLive) {
      if (live?.over) { await dropSnippet(single, live.over, live.at, linesOff, { x: grab[0].x, y: grab[0].y, pane: grab[0].pane }); return; }
      finishLive();
    }
    if (!moved && card.type === 'snippet' && !(ev.ctrlKey || ev.shiftKey)) select([card.id]);
    renderLinks();
    scheduleSave();
  });
}

/** Shows where a picked snippet would land. Returns {at, del}. */
function showDropIndicator(f, clientY, s) {
  const at = lineAt(f, clientY, true);
  const line = lineAt(f, clientY, false);
  const sel = selRange(f);
  let rep = null;
  if (sel && line >= sel.a && line <= sel.b) rep = sel;
  else if (s?.target?.file === f.path && s.target.replaceStart) {
    const a = s.target.replaceStart - 1, b = s.target.replaceEnd - 1;
    if (line >= a - 1 && line <= b + 1) rep = { a, b };
  }
  if (rep) {
    f.dropEl.style.display = 'none';
    Object.assign(f.replEl.style, { display: 'block', top: PAD + rep.a * LH + 'px', height: (rep.b - rep.a + 1) * LH + 'px' });
    return { at: rep.a, del: rep.b - rep.a + 1 };
  }
  f.replEl.style.display = 'none';
  Object.assign(f.dropEl.style, { display: 'block', top: PAD + at * LH + 'px' });
  f.dropEl.firstChild.textContent = `insert at line ${at + 1}`;
  return { at, del: 0 };
}
function hideIndicators(f) { if (!f?.dropEl) return; f.dropEl.style.display = 'none'; f.replEl.style.display = 'none'; }

function onHoverMove(e) {
  if (!e) return;
  lastPointer = e;
  peekAt(e);
  if (!S.pick) return;
  const P = paneAt(e.clientX, e.clientY);
  if (!P) return;
  const p = toWorld(e.clientX, e.clientY, P);
  const f = fileUnder(p.x, p.y, P);
  if (S.pick.over && S.pick.over !== f) hideIndicators(S.pick.over);
  S.pick.over = f;
  if (f && (!S.pick.file || S.pick.file === f)) {
    const r = showDropIndicator(f, e.clientY, S.pick.snips.length === 1 ? S.pick.snips[0] : null);
    f.dropEl.firstChild.textContent = `merge ${plural(S.pick.snips.length, 'snippet')} at line ${r.at + 1}`;
    S.pick.at = r.at; S.pick.del = r.del;
  }
}

function onDblClick(e, P) {
  // Pointer capture retargets dblclick, so look at what is really under the cursor.
  const t = document.elementFromPoint(e.clientX, e.clientY) || e.target;
  if (!P.el.contains(t) || live || S.animating || spaceDown) return;
  if (t.closest('.drawer, .card-head, .mtab, .margin, .icon-btn, .rbar, .sel-tools, .ghost, .resize, .target, .pane-bar, button, input, textarea')) return;
  const card = cardFromEl(t);
  if (!card) {
    if (t !== P.el && t !== P.world && t !== P.links) return;
    const w = toWorld(e.clientX, e.clientY, P);
    const s = createFloating({ title: 'New snippet', lines: [''], x: w.x, y: w.y, pane: P.id });
    enterEdit(s);
    return;
  }
  if (S.editing === card) return;
  if (card.type === 'file') { clearAllSel(); enterEdit(card, lineAt(card, e.clientY)); }
  else enterEdit(card);
}

// ---------- pick mode (merge target chooser) ----------
function startPick(snips, file) {
  cancelPick();
  S.pick = { snips, file, over: null, at: 0, del: 0 };
  for (const P of panes.values()) P.el.classList.add('picking');
  toast(`Click a line${file ? ` in ${basename(file.path)}` : ' in a file'} to merge ${plural(snips.length, 'snippet')} · Esc to cancel`, 'info', 3500);
}
function cancelPick() {
  if (!S.pick) return;
  hideIndicators(S.pick.over);
  S.pick = null;
  for (const P of panes.values()) P.el.classList.remove('picking');
}
async function finishPick(e) {
  const P = paneAt(e.clientX, e.clientY);
  const p = P && toWorld(e.clientX, e.clientY, P);
  const f = P && fileUnder(p.x, p.y, P);
  const pick = S.pick;
  if (!f || (pick.file && pick.file !== f)) { cancelPick(); return; }
  onHoverMove(e);
  const { at, del } = pick;
  cancelPick();
  const snips = pick.snips.slice().sort((a, b) => b.y - a.y); // insert bottom-most first at the same point → top-to-bottom order
  for (let i = 0; i < snips.length; i++) await mergeSnippet(snips[i], f, at, i === 0 ? del : 0);
}

/** Merge a selection of floating snippets into a file (right-click → Merge). */
export async function mergeSelection(f) {
  const snips = [...S.selected].map(id => S.snippets.get(id)).filter(s => s && !s.fileId);
  if (!snips.length) return;
  const placed = [], unplaced = [];
  for (const s of snips) {
    if (f && s.target?.file === f.path) placed.push({ s, line: s.target.replaceStart || s.target.insertAt || f.lines.length + 1, del: s.target.replaceStart ? s.target.replaceEnd - s.target.replaceStart + 1 : 0 });
    else unplaced.push(s);
  }
  placed.sort((a, b) => b.line - a.line);
  for (const p of placed) await mergeSnippet(p.s, f, clamp(p.line - 1, 0, f.lines.length), p.del);
  if (unplaced.length) startPick(unplaced, f);
}

function combineSelection() {
  const snips = [...S.selected].map(id => S.snippets.get(id)).filter(s => s && !s.fileId && !s.ref).sort((a, b) => a.y - b.y || a.x - b.x);
  if (snips.length < 2) return;
  pushUndo();
  const [first, ...rest] = snips;
  for (const r of rest) { first.lines.push(...r.lines); first.chat.push(...r.chat); removeSnippet(r); }
  first.title = `${first.title} +${rest.length}`;
  renderSnippetCard(first);
  select([first.id]);
  scheduleSave();
}

// =====================================================================
// context menu
// =====================================================================
function showMenu(e, items) {
  menuEl.replaceChildren();
  for (const it of items) {
    if (it === '-') { menuEl.append(h('div', { class: 'sep' })); continue; }
    if (it.header) { menuEl.append(h('div', { class: 'hdr' }, it.header)); continue; }
    const mi = h('div', { class: 'mi' + (it.disabled ? ' disabled' : '') }, h('span', {}, it.label), it.key ? h('span', { class: 'k' }, it.key) : null);
    mi.addEventListener('click', () => { hideMenu(); it.fn(); });
    menuEl.append(mi);
  }
  menuEl.style.display = 'block';
  const w = menuEl.offsetWidth, hh = menuEl.offsetHeight;
  menuEl.style.left = Math.min(e.clientX, innerWidth - w - 6) + 'px';
  menuEl.style.top = Math.min(e.clientY, innerHeight - hh - 6) + 'px';
}
function hideMenu() { if (menuEl) menuEl.style.display = 'none'; }

function regionMenu(s) {
  const f = hostOf(s.fileId);
  return [
    { header: s.title },
    { label: s.drawerOpen ? 'Hide chat' : 'Open chat', fn: () => toggleDrawer(s) },
    { label: s.draftOpen ? 'Hide draft' : 'Open draft', fn: () => toggleDraft(s) },
    { label: S.ctx.includes(s.id) ? 'Remove from context' : 'Add to context', fn: () => toggleCtx(s.id) },
    { label: 'Rename…', fn: () => renameSnippet(s) },
    '-',
    { label: 'Detach (cut out)', fn: () => detachRegion(s, f.x + f.w + 80, f.y + BODY_TOP + s.start * LH) },
    { label: 'Pop out reference', key: 'Alt+drag', fn: () => { const r = makeReference(s, f.x + f.w + 80, f.y + BODY_TOP + s.start * LH); flyLines(s.start, s.end - s.start + 1, f, r); } },
    { label: 'Copy out', fn: () => detachRegion(s, f.x + f.w + 80, f.y + BODY_TOP + s.start * LH, true) },
    { label: 'Edit file here', key: 'dbl-click', fn: () => enterEdit(s) },
    '-',
    { label: 'Dissolve snippet (keep code)', fn: () => { pushUndo(); removeSnippet(s); } },
    { label: 'Dissolve all snippets in this file', fn: () => dissolveAll(f) },
    { label: 'Delete snippet and its lines', fn: () => { pushUndo(); const n = s.end - s.start + 1, a = s.start; removeSnippet(s); spliceFile(f, a, n, []); renderHost(f); hooks.onDirty?.(); } },
  ];
}

function onContextMenu(e, P) {
  e.preventDefault();
  if (S.pick) { cancelPick(); return; }
  setActive(P);
  const t = e.target;
  const card = cardFromEl(t);
  const w = toWorld(e.clientX, e.clientY, P);
  const selSnips = [...S.selected].map(id => S.snippets.get(id)).filter(s => s && !s.fileId);
  const selFiles = [...S.selected].map(id => S.files.get(id)).filter(Boolean);

  const rbar = t.closest('.rbar');
  if (rbar) { showMenu(e, regionMenu(S.snippets.get(rbar.closest('.region').dataset.sid))); return; }

  // A file under the cursor while snippets are selected is an obvious merge target.
  const mergeTarget = card?.type === 'file' ? card : selFiles.length === 1 ? selFiles[0] : null;
  const items = [];
  if (selSnips.length && mergeTarget) {
    items.push({ header: `${plural(selSnips.length, 'snippet')} → ${basename(mergeTarget.path)}` },
      { label: `Merge into ${basename(mergeTarget.path)}`, key: 'auto-place', fn: () => mergeSelection(mergeTarget) },
      { label: 'Merge at a line I pick…', fn: () => startPick(selSnips, mergeTarget) }, '-');
  } else if (selSnips.length) {
    items.push({ header: plural(selSnips.length, 'snippet') },
      { label: 'Merge into…', key: 'pick a line', fn: () => startPick(selSnips, null) });
    if (selSnips.some(s => s.target?.file)) items.push({ label: 'Apply drafted snippets to their targets', fn: async () => { for (const s of selSnips.filter(x => x.target?.file).sort((a, b) => (b.target.replaceStart || b.target.insertAt || 0) - (a.target.replaceStart || a.target.insertAt || 0))) await applyToTarget(s); } });
    if (selSnips.length > 1) items.push({ label: 'Combine into one snippet', fn: combineSelection });
    items.push({ label: 'Add all to context', fn: () => selSnips.forEach(s => !S.ctx.includes(s.id) && toggleCtx(s.id)) },
      { label: 'Delete', key: 'Del', fn: () => { pushUndo(); selSnips.forEach(s => removeSnippet(s)); } }, '-');
  }

  if (card?.type === 'file') {
    const f = card;
    const sel = selRange(f);
    if (sel) {
      items.push({ header: `Lines ${sel.a + 1}–${sel.b + 1}` },
        { label: 'Create snippet from selection', fn: () => makeSnippetFromSel(f) },
        { label: 'Detach selection as snippet', fn: () => { const s = makeSnippetFromSel(f); detachRegion(s, f.x + f.w + 80, f.y + BODY_TOP + sel.a * LH); } },
        { label: 'Copy selection out', fn: () => copyOut(f, sel) },
        { label: 'Ask the LLM about selection', fn: () => { const s = makeSnippetFromSel(f); openDrawer(s, true); } }, '-');
    }
    items.push({ header: basename(f.path) },
      { label: f.drawerOpen ? 'Hide file chat' : 'Chat about this file', fn: () => toggleDrawer(f) },
      { label: f.draftOpen ? 'Hide draft' : 'Draft changes to this file', fn: () => toggleDraft(f) },
      { label: 'Explain file', fn: () => explainFile(f.path) },
      { label: 'Dissolve all snippets in this file', disabled: !regionsOf(f).length, fn: () => dissolveAll(f) },
      { label: S.ctx.includes(f.id) ? 'Remove from context' : 'Add file to context', fn: () => toggleCtx(f.id) },
      { label: 'Edit in canvas', key: 'dbl-click', fn: () => enterEdit(f, lineAt(f, e.clientY)) },
      { label: 'Save', key: 'Ctrl+S', disabled: !isDirty(f), fn: () => saveFile(f) },
      { label: `Open in ${EDITOR}`, fn: () => send({ type: 'openInEditor', path: f.path, line: lineAt(f, e.clientY) + 1 }) },
      { label: 'Close', fn: () => closeFile(f) });
  } else if (card?.type === 'snippet' && !selSnips.includes(card)) {
    const s = card;
    const sel = ownLines(s) && selRange(s);
    if (sel) {
      items.push({ header: `Lines ${sel.a + 1}–${sel.b + 1}` },
        { label: 'Create snippet from selection', fn: () => makeSnippetFromSel(s) },
        { label: 'Detach selection as snippet', fn: () => { const x = makeSnippetFromSel(s); detachRegion(x, s.x + s.w + 80, s.y + BODY_TOP + sel.a * LH); } },
        { label: 'Copy selection out', fn: () => copyOut(s, sel) },
        { label: 'Ask the LLM about selection', fn: () => { const x = makeSnippetFromSel(s); openDrawer(x, true); } }, '-');
    }
    items.push({ header: s.title },
      { label: 'Merge into…', key: 'pick a line', fn: () => startPick([s], null) },
      s.target?.file ? { label: `Apply to ${basename(s.target.file)}`, fn: () => applyToTarget(s) } : null,
      { label: s.drawerOpen ? 'Hide chat' : 'Open chat', fn: () => toggleDrawer(s) },
      { label: s.draftOpen ? 'Hide draft' : 'Open draft', fn: () => toggleDraft(s) },
      { label: S.ctx.includes(s.id) ? 'Remove from context' : 'Add to context', fn: () => toggleCtx(s.id) },
      { label: 'Rename…', fn: () => renameSnippet(s) },
      { label: s.ref ? 'Remove reference' : 'Delete', fn: () => { pushUndo(); removeSnippet(s); } });
  } else if (!card) {
    items.push({ label: 'New snippet here', key: 'dbl-click', fn: () => { const s = createFloating({ title: 'New snippet', lines: [''], x: w.x, y: w.y, pane: P.id }); enterEdit(s); } },
      { label: 'Explain All', fn: () => hooks.explainAll?.() },
      { label: 'Arrange files in this view', fn: () => arrange(null, P) },
      { label: 'Dissolve all snippets (every file)', disabled: ![...S.snippets.values()].some(x => x.fileId), fn: () => dissolveAll(null) },
      { label: 'Fit everything', key: 'F', fn: () => fitAll(P) },
      '-',
      { label: 'Split right', key: 'Ctrl+\\', fn: () => splitPane(P, 'row') },
      { label: 'Split down', fn: () => splitPane(P, 'col') },
      panes.size > 1 ? { label: 'Close this view', fn: () => closePane(P) } : null,
      '-',
      { label: 'Undo', key: 'Ctrl+Z', disabled: !S.undo.length, fn: undo },
      { label: 'Redo', key: 'Ctrl+Y', disabled: !S.redo.length, fn: redo });
  }
  const clean = items.filter(Boolean);
  while (clean[clean.length - 1] === '-') clean.pop();
  if (clean.length) showMenu(e, clean);
}

// =====================================================================
// find and replace inside one card (modelled on VS Code's find widget)
// =====================================================================
const finds = new Map(); // card id -> find state; kept by id so it survives re-renders and undo
const MAX_HITS = 20000;

const findLines = c => c.lines || [];
/** Column a character index is drawn at (tab stops every 4, as the cards' `tab-size`). */
function visualCol(line, index) {
  let col = 0;
  for (let i = 0; i < index; i++) col += line[i] === '\t' ? 4 - (col % 4) : 1;
  return col;
}

function findRegex(st, sticky) {
  if (!st.q) return null;
  let src = st.regex ? st.q : st.q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  if (st.word) src = `\\b(?:${src})\\b`;
  return new RegExp(src, (sticky ? 'y' : 'g') + (st.case ? '' : 'i') + 'u');
}

function computeHits(c, st) {
  st.hits = []; st.error = '';
  let re;
  try { re = findRegex(st, false); } catch (err) { st.error = String(err.message || err).replace(/^Invalid regular expression: /, ''); return; }
  if (!re) return;
  const lines = findLines(c);
  const [a, b] = st.within || [0, lines.length - 1];
  scan: for (let i = Math.max(0, a); i <= Math.min(b, lines.length - 1); i++) {
    for (const m of lines[i].matchAll(re)) {
      if (!m[0]) continue;
      st.hits.push({ line: i, start: m.index, end: m.index + m[0].length });
      if (st.hits.length >= MAX_HITS) break scan;
    }
  }
  st.cur = Math.min(st.cur, Math.max(0, st.hits.length - 1));
}

/** VS Code's "preserve case": shape the replacement like the text it replaces. */
function likeCase(found, rep) {
  const letters = found.replace(/[^\p{L}]/gu, '');
  if (!letters) return rep;
  if (letters === letters.toUpperCase() && letters !== letters.toLowerCase()) return rep.toUpperCase();
  if (letters === letters.toLowerCase()) return rep.toLowerCase();
  if (letters[0] === letters[0].toUpperCase()) return rep.charAt(0).toUpperCase() + rep.slice(1);
  return rep;
}

/** What a match is replaced with: `$1`, `$&` and `$$` in regex mode, optionally shaped like the match. */
function replacementFor(st, line, hit) {
  let out = st.rep;
  if (st.regex) {
    const re = findRegex(st, true);
    re.lastIndex = hit.start;
    const m = re.exec(line);
    if (m) out = st.rep.replace(/\$(\$|&|\d{1,2})/g, (all, k) => (k === '$' ? '$' : k === '&' ? m[0] : m[+k] ?? (+k < m.length ? '' : all)));
  }
  return st.keep ? likeCase(line.slice(hit.start, hit.end), out) : out;
}

/** Write new text for some lines of a card as one undo step. A reference card writes to its source file. */
function setCardLines(c, changed) {
  if (!changed.length) return;
  pushUndo();
  const src = c.type === 'file' ? null : refSource(c);
  const f = c.type === 'file' ? c : src ? hostOf(src.fileId) : null;
  if (f) {
    const base = src ? src.start : 0;
    for (const [line, text] of changed) spliceFile(f, base + line, 1, [text]); // line by line, so snippets stay attached
    renderHost(f);
  } else {
    for (const [line, text] of changed) c.lines[line] = text;
    renderSnippetCard(c);
  }
  renderLinks();
  hooks.onDirty?.();
  scheduleSave();
}

function findStep(c, dir) {
  const st = finds.get(c.id);
  if (!st || !st.hits.length) return;
  st.cur = (st.cur + dir + st.hits.length) % st.hits.length;
  paintFind(c);
  revealHit(c);
}

/** Pan the view so the current match is on screen. */
function revealHit(c) {
  const st = finds.get(c.id), hit = st?.hits[st.cur];
  if (!hit) return;
  const P = paneOf(c), t = P.target;
  const top = c.el.offsetTop + (c.bodyEl?.offsetTop || HEAD) + PAD + hit.line * LH;
  const y = top * t.z + t.y, x = c.x * t.z + t.x;
  if (y < 90 || y > P.el.clientHeight - 60) { t.y = P.el.clientHeight * 0.4 - top * t.z; P.anchor = { x: 0, y: 0 }; }
  if (x < -200 || x > P.el.clientWidth - 200) { t.x = 80 - c.x * t.z; P.anchor = { x: 0, y: 0 }; }
}

function replaceOne(c) {
  const st = finds.get(c.id), hit = st?.hits[st.cur];
  if (!hit) return;
  const line = findLines(c)[hit.line];
  const out = replacementFor(st, line, hit);
  const after = hit.start + out.length;
  st.wantFocus = 'rep';
  setCardLines(c, [[hit.line, line.slice(0, hit.start) + out + line.slice(hit.end)]]);
  // go on to the next match after what was just written, even if the new text matches too
  const next = st.hits.findIndex(h => h.line > hit.line || (h.line === hit.line && h.start >= after));
  st.cur = next < 0 ? 0 : next;
  paintFind(c);
  revealHit(c);
}

function replaceAll(c) {
  const st = finds.get(c.id);
  if (!st || !st.hits.length) return;
  const lines = findLines(c), changed = new Map();
  for (let i = st.hits.length - 1; i >= 0; i--) { // right to left, so earlier matches on a line keep their offsets
    const h = st.hits[i];
    const cur = changed.get(h.line) ?? lines[h.line];
    changed.set(h.line, cur.slice(0, h.start) + replacementFor(st, lines[h.line], h) + cur.slice(h.end));
  }
  const n = st.hits.length;
  st.wantFocus = 'rep';
  setCardLines(c, [...changed]);
  toast(`Replaced ${plural(n, 'occurrence')}`, 'ok', 2500);
}

function toggleWithin(c) {
  const st = finds.get(c.id);
  if (st.within) st.within = null;
  else {
    const r = c.type === 'file' ? selRange(c) : null;
    if (!r) { toast('Select some lines first, then turn on Find in Selection.'); return; }
    st.within = [r.a, r.b];
  }
  refreshFind(c);
}

function closeFind(c) {
  const st = finds.get(c.id);
  finds.delete(c.id);
  st?.el.remove(); st?.layer.remove();
}
function toggleFind(c) { finds.has(c.id) ? closeFind(c) : openFind(c, false); }

/** Ctrl+F / Ctrl+H: the selected card, else the one under the pointer. A selected snippet inside a file limits the search to its lines. */
function openFindHere(replace) {
  const ids = [...S.selected];
  let c = null, within = null;
  if (ids.length === 1) {
    c = S.files.get(ids[0]) || S.snippets.get(ids[0]);
    if (c?.fileId) { within = [c.start, c.end]; c = hostOf(c.fileId); }
  }
  if (!c && lastPointer) c = cardFromEl(document.elementFromPoint(lastPointer.clientX, lastPointer.clientY) || root);
  if (!c) { toast('Select a file or snippet (or point at one) to search in it.'); return; }
  openFind(c, replace, '', within);
}

function openFind(c, replace, seed, within) {
  if (S.editing === c) exitEdit();
  let st = finds.get(c.id);
  if (!st) { st = { q: '', rep: '', case: false, word: false, regex: false, keep: false, replace: false, within: null, cur: 0, hits: [], error: '' }; finds.set(c.id, st); buildFindBar(c, st); }
  if (replace) st.replace = true;
  if (within) st.within = within;
  if (seed) { st.q = seed; st.cur = 0; }
  st.wantFocus = replace && st.q ? 'rep' : 'find';
  st.selectAll = true;
  mountFind(c);
}

function buildFindBar(c, st) {
  const tog = (label, title, key) => {
    const b = h('button', { class: 'fb-tog', title }, label);
    b.addEventListener('click', () => { if (key === 'within') toggleWithin(c); else { st[key] = !st[key]; refreshFind(c); } });
    b.dataset.key = key;
    return b;
  };
  const act = (label, title, fn) => { const b = h('button', { class: 'fb-act', title }, label); b.addEventListener('click', fn); return b; };
  const findIn = h('input', { class: 'fb-in', placeholder: 'Find', spellcheck: 'false' });
  const repIn = h('input', { class: 'fb-in', placeholder: 'Replace', spellcheck: 'false' });
  const count = h('span', { class: 'fb-count' });
  const more = h('button', { class: 'fb-more', title: 'Toggle Replace' }, '▸');
  more.addEventListener('click', () => { st.replace = !st.replace; refreshFind(c); });
  findIn.addEventListener('input', () => { st.q = findIn.value; st.cur = 0; refreshFind(c); revealHit(c); });
  repIn.addEventListener('input', () => { st.rep = repIn.value; });
  const keys = inRep => e => {
    e.stopPropagation();
    const ctrl = e.ctrlKey || e.metaKey, k = e.key.toLowerCase();
    let used = true;
    if (e.key === 'Escape') closeFind(c);
    else if (e.key === 'Enter' && ctrl && (e.altKey || inRep)) replaceAll(c);
    else if (e.key === 'Enter' && inRep) replaceOne(c);
    else if (e.key === 'Enter' || e.key === 'F3') findStep(c, e.shiftKey ? -1 : 1);
    else if (e.key === 'Tab' || e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      const toRep = e.key === 'Tab' ? !inRep : e.key === 'ArrowDown';
      if (toRep) st.replace = true;
      st.wantFocus = toRep ? 'rep' : 'find';
      refreshFind(c);
    }
    else if (ctrl && k === 'f') findIn.select();
    else if (ctrl && k === 'h') { st.replace = true; st.wantFocus = 'rep'; refreshFind(c); }
    else if (e.altKey && k === 'c') { st.case = !st.case; refreshFind(c); }
    else if (e.altKey && k === 'w') { st.word = !st.word; refreshFind(c); }
    else if (e.altKey && k === 'r') { st.regex = !st.regex; refreshFind(c); }
    else if (e.altKey && k === 'p') { st.keep = !st.keep; refreshFind(c); }
    else if (e.altKey && k === 'l') toggleWithin(c);
    else used = false;
    if (used) e.preventDefault();
  };
  findIn.addEventListener('keydown', keys(false));
  repIn.addEventListener('keydown', keys(true));
  st.el = h('div', { class: 'find-bar' }, more,
    h('div', { class: 'fb-rows' },
      h('div', { class: 'fb-row' },
        h('div', { class: 'fb-box' }, findIn, tog('Aa', 'Match Case (Alt+C)', 'case'), tog('ab', 'Match Whole Word (Alt+W)', 'word'), tog('.*', 'Use Regular Expression (Alt+R)', 'regex')),
        count,
        act('↑', 'Previous Match (Shift+Enter)', () => findStep(c, -1)), act('↓', 'Next Match (Enter)', () => findStep(c, 1)),
        tog('≡', 'Find in Selection (Alt+L)', 'within'), act('×', 'Close (Escape)', () => closeFind(c))),
      h('div', { class: 'fb-row fb-rep' },
        h('div', { class: 'fb-box' }, repIn, tog('AB', 'Preserve Case (Alt+P)', 'keep')),
        act('Replace', 'Replace (Enter)', () => replaceOne(c)), act('All', 'Replace All (Ctrl+Alt+Enter)', () => replaceAll(c)))));
  for (const ev of ['pointerdown', 'dblclick', 'contextmenu', 'wheel']) st.el.addEventListener(ev, e => e.stopPropagation());
  st.layer = h('div', { class: 'find-layer' });
  Object.assign(st, { findIn, repIn, count, more });
}

/** Recompute the matches and redraw the bar and the highlights. */
function refreshFind(c) {
  const st = finds.get(c.id);
  if (!st) return;
  computeHits(c, st);
  st.el.classList.toggle('with-replace', st.replace);
  st.more.textContent = st.replace ? '▾' : '▸';
  if (st.findIn.value !== st.q) st.findIn.value = st.q;
  if (st.repIn.value !== st.rep) st.repIn.value = st.rep;
  for (const b of st.el.querySelectorAll('.fb-tog')) b.classList.toggle('on', b.dataset.key === 'within' ? !!st.within : !!st[b.dataset.key]);
  paintFind(c);
  if (st.wantFocus) {
    const inp = st.wantFocus === 'rep' ? st.repIn : st.findIn;
    st.wantFocus = null;
    inp.focus({ preventScroll: true });
    if (st.selectAll) { st.selectAll = false; inp.select(); }
  }
}

function paintFind(c) {
  const st = finds.get(c.id);
  if (!st) return;
  const bad = !!st.error || (!!st.q && !st.hits.length);
  st.count.textContent = st.error || (!st.q ? '' : !st.hits.length ? 'No results' : `${st.cur + 1} of ${st.hits.length}${st.hits.length >= MAX_HITS ? '+' : ''}`);
  st.count.classList.toggle('bad', bad);
  st.findIn.parentElement.classList.toggle('bad', bad);
  const lines = findLines(c);
  // highlights near the current match only: a card can hold thousands
  const from = Math.max(0, st.cur - 1500), to = Math.min(st.hits.length, st.cur + 1500);
  let html = '';
  for (let i = from; i < to; i++) {
    const hit = st.hits[i], line = lines[hit.line];
    const a = visualCol(line, hit.start), b = visualCol(line, hit.end);
    html += `<i class="${i === st.cur ? 'cur' : ''}" style="top:${PAD + hit.line * LH}px;left:calc(var(--gutter, 44px) + ${a}ch);width:${b - a}ch"></i>`;
  }
  st.layer.innerHTML = html;
}

/** Put a card's find bar and highlights back after the card was redrawn. */
function mountFind(c) {
  const st = finds.get(c.id);
  if (!st || !c.el) return;
  const active = document.activeElement;
  if (!st.wantFocus && (active === st.findIn || active === st.repIn)) st.wantFocus = active === st.repIn ? 'rep' : 'find';
  c.el.append(st.el);
  if (c.bodyEl) c.bodyEl.prepend(st.layer);
  st.el.style.top = HEAD + 'px'; // just under the header (the card may not be laid out yet, so no measuring)
  refreshFind(c);
}

// =====================================================================
// keyboard
// =====================================================================
function onKey(e) {
  if (e.key === 'Escape' && drag && cancelDrag()) { e.preventDefault(); return; }
  const tag = (e.target.tagName || '').toLowerCase();
  if (tag === 'textarea' || tag === 'input') return;
  if (e.code === 'Space') { e.preventDefault(); if (!spaceDown) setSpace(true); return; }
  const ctrl = e.ctrlKey || e.metaKey;
  const P = AP();
  if (ctrl && !e.altKey && (e.key.toLowerCase() === 'f' || e.key.toLowerCase() === 'h')) { e.preventDefault(); openFindHere(e.key.toLowerCase() === 'h'); return; }
  if (e.key === 'Escape') { if (cancelDrag()) return; cancelPick(); hideMenu(); clearAllSel(); clearSelection(); return; }
  if (ctrl && e.key === '\\') { e.preventDefault(); splitPane(P, e.shiftKey ? 'col' : 'row'); return; }
  if (ctrl && e.key.toLowerCase() === 's') {
    e.preventDefault();
    const sel = [...S.selected].map(id => S.files.get(id)).filter(Boolean);
    (sel.length ? sel : [...S.files.values()]).forEach(f => isDirty(f) && saveFile(f));
    return;
  }
  if (ctrl && e.key.toLowerCase() === 'z' && !e.shiftKey) { e.preventDefault(); undo(); return; }
  if (ctrl && (e.key.toLowerCase() === 'y' || (e.shiftKey && e.key.toLowerCase() === 'z'))) { e.preventDefault(); redo(); return; }
  if (ctrl && e.key === '0') { e.preventDefault(); zoomTo(1, null, null, P); return; }
  if (ctrl && (e.key === '=' || e.key === '+')) { e.preventDefault(); zoomTo(P.target.z * 1.25, null, null, P); return; }
  if (ctrl && e.key === '-') { e.preventDefault(); zoomTo(P.target.z / 1.25, null, null, P); return; }
  if (ctrl && e.key.toLowerCase() === 'a') { e.preventDefault(); select(allCards().filter(c => inPane(c, P)).map(c => c.id)); return; }
  if (e.key === 'f' || e.key === 'F') {
    const sel = allCards().filter(c => S.selected.has(c.id) && inPane(c, P));
    if (sel.length) { const b = bounds(sel); frameRect(b.x0, b.y0, b.x1 + 260, b.y1, 1, P); } else fitAll(P);
    return;
  }
  if (e.key === 'Delete' || e.key === 'Backspace') {
    const snips = [...S.selected].map(id => S.snippets.get(id)).filter(Boolean);
    if (snips.length) { pushUndo(); snips.forEach(s => removeSnippet(s)); }
    [...S.selected].map(id => S.files.get(id)).filter(Boolean).forEach(f => closeFile(f));
  }
}

// =====================================================================
// layout
// =====================================================================
export function arrange(files, P = AP()) {
  files = files || [...S.files.values()].filter(f => inPane(f, P)).sort((a, b) => a.path.localeCompare(b.path));
  if (!files.length) return;
  const cols = clamp(Math.ceil(Math.sqrt(files.length / 1.5)), 1, 6);
  const colW = Math.max(...files.map(f => f.w)) + 320;
  const heights = new Array(cols).fill(0);
  for (const f of files) {
    if (!S.shared && paneOf(f) !== P) { f.pane = P.id; P.world.append(f.el); }
    const c = heights.indexOf(Math.min(...heights));
    const from = { left: f.el.style.left, top: f.el.style.top };
    f.x = c * colW; f.y = heights[c];
    heights[c] += cardH(f) + 60;
    f.el.animate([from, { left: f.x + 'px', top: f.y + 'px' }], { duration: 380, easing: 'cubic-bezier(.2,.8,.3,1)' });
    positionCard(f);
  }
  renderLinks();
  frameRect(0, 0, cols * colW, Math.min(Math.max(...heights), 4000), 0.9, P);
  scheduleSave();
}

// =====================================================================
// undo / persistence
// =====================================================================
function plainSnippet(s) {
  const o = {};
  for (const [k, v] of Object.entries(s)) if (!k.startsWith('_') && !k.endsWith('El') && k !== 'el') o[k] = v;
  return o;
}
const plainFile = (f, withLines) => ({
  id: f.id, path: f.path, pane: f.pane, x: f.x, y: f.y, w: f.w, chat: f.chat, mode: f.mode, drawerOpen: f.drawerOpen,
  draft: f.draft || undefined, draftOpen: f.draftOpen || undefined, wide: f.wide || undefined, syntax: f.syntax || undefined,
  ...(withLines ? { lines: f.lines, saved: f.saved, eol: f.eol } : { lines: isDirty(f) ? f.lines : undefined }),
});
function snapshot() {
  return JSON.stringify({
    files: [...S.files.values()].map(f => plainFile(f, true)),
    snippets: [...S.snippets.values()].map(plainSnippet),
    ctx: S.ctx,
  });
}
export function pushUndo() {
  S.undo.push(snapshot());
  if (S.undo.length > 60) S.undo.shift();
  S.redo = [];
}
function restoreSnap(json) {
  if (S.editing) { S.editing = null; }
  const d = JSON.parse(json);
  for (const f of S.files.values()) { f.el.remove(); f._drawer?.el.remove(); }
  for (const s of S.snippets.values()) { s.el?.remove(); s._drawer?.el.remove(); s._draftUI?.el.remove(); }
  S.files.clear(); S.snippets.clear(); S.selected.clear();
  for (const f of d.files) S.files.set(f.id, { ...f, type: 'file', sel: null, chat: f.chat || [] });
  for (const s of d.snippets) S.snippets.set(s.id, s);
  S.ctx = d.ctx.filter(id => S.files.has(id) || S.snippets.has(id));
  for (const f of S.files.values()) { renderHost(f); mount(f); }
  for (const s of S.snippets.values()) if (!s.fileId) { renderSnippetCard(s); mount(s); }
  renderLinks();
  hooks.onFilesChanged?.(); hooks.onContext?.(); hooks.onDirty?.();
  scheduleSave();
}
export function undo() { if (!S.undo.length || S.animating) return; S.redo.push(snapshot()); restoreSnap(S.undo.pop()); }
export function redo() { if (!S.redo.length || S.animating) return; S.undo.push(snapshot()); restoreSnap(S.redo.pop()); }

let saveTimer = 0;
export function scheduleSave() {
  hooks.onChange?.();
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => send({ type: 'saveState', state: serialize() }), 700);
}
function serialize() {
  return {
    v: 2,
    shared: S.shared,
    layout: S.layout,
    active: S.active,
    views: Object.fromEntries([...panes.values()].map(P => [P.id, { ...P.target }])),
    files: [...S.files.values()].map(f => plainFile(f, false)),
    snippets: [...S.snippets.values()].map(plainSnippet),
    ctx: S.ctx,
    history: S.history.slice(-150),
    globalChat: S.globalChat.slice(-200),
  };
}

export async function restoreSaved(st) {
  if (!st || (st.v !== 1 && st.v !== 2)) return;
  // rebuild the split layout
  if (st.v === 2 && st.layout) {
    for (const P of panes.values()) P.el.remove();
    panes.clear();
    mirrorObs?.disconnect();
    W = null; hostId = null;
    S.shared = st.shared !== false;
    hooks.onShared?.(S.shared);
    const ids = [];
    const walk = n => { if (n.pane) ids.push(n.pane); else n.children.forEach(walk); };
    walk(st.layout);
    for (const id of ids) {
      const P = createPane(id);
      const v = st.views?.[id];
      if (v) { P.view = { ...v }; P.target = { ...v }; }
    }
    S.layout = st.layout;
    renderLayout();
    setActive(panes.get(st.active) || panes.get(ids[0]));
  } else if (st.view) {
    const P = AP();
    P.view = { ...st.view }; P.target = { ...st.view };
  }
  S.history = st.history || [];
  S.globalChat.splice(0, S.globalChat.length, ...(st.globalChat || []));
  const results = await Promise.all((st.files || []).map(f => openFile(f.path, { x: f.x, y: f.y }, {
    id: f.id, w: f.w, lines: f.lines, silent: true, pane: f.pane, chat: f.chat, mode: f.mode, drawerOpen: f.drawerOpen,
  })));
  const ok = new Set(results.filter(Boolean).map(f => f.id));
  for (const f of st.files || []) { const c = S.files.get(f.id); if (c) Object.assign(c, { draft: f.draft, draftOpen: !!f.draftOpen, wide: !!f.wide, syntax: f.syntax }); }
  const list = st.snippets || [];
  for (const s of list) if (!s.fileId) S.snippets.set(s.id, s);
  for (const s of list) {
    if (!s.fileId) continue;
    const f = hostOf(s.fileId);
    if (!f || (f.type === 'file' && !ok.has(s.fileId)) || s.start >= (f.lines || []).length) continue;
    s.end = Math.min(s.end, f.lines.length - 1);
    S.snippets.set(s.id, s);
  }
  for (const s of S.snippets.values()) if (!s.fileId) { renderSnippetCard(s); mount(s); }
  for (const f of S.files.values()) renderRegions(f);
  S.ctx = (st.ctx || []).filter(id => S.files.has(id) || S.snippets.has(id));
  S.ctx.forEach(markCtx);
  renderLinks();
  for (const P of panes.values()) applyView(P);
  hooks.onContext?.();
}
