// Shared helpers: ids, escaping, tiny markdown, syntax highlight, icons.

let _id = 0;
export const uid = (p = 'id') => `${p}_${Date.now().toString(36)}_${(_id++).toString(36)}`;

export const esc = s => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

export const h = (tag, attrs = {}, ...kids) => {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'html') el.innerHTML = v;
    else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
    else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? '' : v);
  }
  for (const k of kids.flat()) if (k != null) el.append(k instanceof Node ? k : document.createTextNode(String(k)));
  return el;
};

export const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
export const basename = p => p.split('/').pop();
export const dirname = p => { const i = p.lastIndexOf('/'); return i < 0 ? '' : p.slice(0, i + 1); };
export const extOf = p => { const b = basename(p); const i = b.lastIndexOf('.'); return i <= 0 ? '' : b.slice(i + 1).toLowerCase(); };
export const timeStr = t => new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
export const plural = (n, w) => `${n} ${w}${n === 1 ? '' : 's'}`;

const EXT_COLORS = {
  ts: '#3178c6', tsx: '#3178c6', js: '#f1e05a', jsx: '#f1e05a', mjs: '#f1e05a', cjs: '#f1e05a', json: '#a1a1a1',
  py: '#3572a5', rs: '#dea584', go: '#00add8', c: '#8e9aaf', h: '#8e9aaf', cpp: '#f34b7d', hpp: '#f34b7d', cc: '#f34b7d',
  cs: '#178600', java: '#b07219', kt: '#a97bff', swift: '#f05138', rb: '#cc342d', php: '#4f5d95', lua: '#000080',
  html: '#e34c26', css: '#563d7c', scss: '#c6538c', md: '#4a90d9', yml: '#cb171e', yaml: '#cb171e', toml: '#9c4221',
  sh: '#89e051', ps1: '#012456', sql: '#e38c00', vue: '#41b883', svelte: '#ff3e00',
};
export const extColor = p => EXT_COLORS[extOf(p)] || '#7f8794';

export const SNIPPET_COLORS = ['#c792ea', '#82aaff', '#f78c6c', '#7fdbca', '#ffcb6b', '#ff5370', '#c3e88d', '#89ddff'];
let _c = 0;
export const nextColor = () => SNIPPET_COLORS[_c++ % SNIPPET_COLORS.length];

// ---------- icons (inline svg, stroke = currentColor) ----------
const I = d => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
export const icons = {
  wide: I('<path d="M3 6h11M3 12h18M3 18h11M18 9l3 3-3 3"/><path d="M16 3v18" stroke-dasharray="2 2"/>'),
  close: I('<path d="M6 6l12 12M18 6L6 18"/>'),
  ctx: I('<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="8"/>'),
  explain: I('<path d="M9.1 9a3 3 0 1 1 3.9 2.9c-.6.2-1 .8-1 1.4V14"/><circle cx="12" cy="17.5" r=".6"/><circle cx="12" cy="12" r="10"/>'),
  editor: I('<path d="M14 3h7v7M10 14 21 3M21 14v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5"/>'),
  save: I('<path d="M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M7 3v5h8M7 21v-7h10v7"/>'),
  chat: I('<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/>'),
  merge: I('<path d="M6 3v6a6 6 0 0 0 6 6h6M15 12l3 3-3 3"/>'),
  send: I('<path d="M5 12h14M13 6l6 6-6 6"/>'),
  stop: I('<rect x="7" y="7" width="10" height="10" rx="1.5"/>'),
  trash: I('<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>'),
  notes: I('<path d="M4 6h16M4 12h10M4 18h7"/>'),
  fit: I('<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>'),
  gear: I('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>'),
  sparkle: I('<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>'),
  plus: I('<path d="M12 5v14M5 12h14"/>'),
  search: I('<circle cx="10.5" cy="10.5" r="6.5"/><path d="M20 20l-4.6-4.6"/>'),
  reset: I('<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>'),
  splitR: I('<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/>'),
  splitD: I('<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 12h18"/>'),
  logo: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="9" height="7" rx="1.5"/><rect x="13.5" y="9.5" width="8" height="6" rx="1.5"/><rect x="5.5" y="15" width="8" height="5.5" rx="1.5"/><path d="M11.5 7h2.5v2.5M9.5 15v-4.5"/></svg>',
};

// ---------- syntax highlighting (small, fast, line-based with block-comment state) ----------
const KW = new Set(('abstract as async await break case catch class const continue debugger def default defer del delete do elif else enum export extends ' +
  'extern false final finally fn for foreach from func function go goto if impl implements import in include interface is lambda let loop match ' +
  'mod module mut namespace new nil none None not null of or and override package pass private protected pub public raise readonly ref return ' +
  'self Self static struct super switch template this throw throws trait true True False try type typedef typeof union unsafe use using var virtual ' +
  'void volatile where while with yield local then end elseif require').split(' '));
const TYPES = new Set('int uint long short char bool boolean float double string String number any unknown never object Object Array Map Set Promise i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 usize isize str Vec Option Result size_t auto byte'.split(' '));

export function langOf(path) {
  const e = extOf(path || '');
  if (['py', 'rb', 'sh', 'yml', 'yaml', 'toml', 'ps1', 'r', 'pl', 'conf', 'ini', 'dockerfile', 'mk', 'makefile'].includes(e)) return 'hash';
  if (['lua', 'sql', 'hs'].includes(e)) return 'dash';
  if (['html', 'xml', 'svg', 'vue', 'md', 'txt', 'csv', ''].includes(e)) return 'plain';
  return 'c';
}

/** Returns {html, state} where state carries an open block comment across lines. */
export function highlightLine(line, lang, state) {
  if (lang === 'plain' || line.length > 2000) return { html: esc(line), state: null };
  let out = '', i = 0;
  const n = line.length;
  if (state === 'block') {
    const end = line.indexOf('*/');
    if (end < 0) return { html: `<span class="tok-com">${esc(line)}</span>`, state: 'block' };
    out += `<span class="tok-com">${esc(line.slice(0, end + 2))}</span>`;
    i = end + 2;
  } else if (state && state.startsWith('triple')) {
    const q = state.slice(6);
    const end = line.indexOf(q);
    if (end < 0) return { html: `<span class="tok-str">${esc(line)}</span>`, state };
    out += `<span class="tok-str">${esc(line.slice(0, end + 3))}</span>`;
    i = end + 3;
  }
  const lineCom = lang === 'hash' ? '#' : lang === 'dash' ? '--' : '//';
  while (i < n) {
    const c = line[i];
    if (line.startsWith(lineCom, i)) { out += `<span class="tok-com">${esc(line.slice(i))}</span>`; return { html: out, state: null }; }
    if (lang === 'c' && line.startsWith('/*', i)) {
      const end = line.indexOf('*/', i + 2);
      if (end < 0) { out += `<span class="tok-com">${esc(line.slice(i))}</span>`; return { html: out, state: 'block' }; }
      out += `<span class="tok-com">${esc(line.slice(i, end + 2))}</span>`; i = end + 2; continue;
    }
    if (lang === 'hash' && (line.startsWith('"""', i) || line.startsWith("'''", i))) {
      const q = line.slice(i, i + 3);
      const end = line.indexOf(q, i + 3);
      if (end < 0) { out += `<span class="tok-str">${esc(line.slice(i))}</span>`; return { html: out, state: 'triple' + q }; }
      out += `<span class="tok-str">${esc(line.slice(i, end + 3))}</span>`; i = end + 3; continue;
    }
    if (c === '"' || c === "'" || c === '`') {
      let j = i + 1;
      while (j < n && line[j] !== c) j += line[j] === '\\' ? 2 : 1;
      out += `<span class="tok-str">${esc(line.slice(i, j + 1))}</span>`; i = j + 1; continue;
    }
    if (/[0-9]/.test(c) && !/[\w$]/.test(line[i - 1] || '')) {
      const m = /^(0x[\da-f_]+|\d[\d_]*(\.\d+)?(e[+-]?\d+)?[a-z]*)/i.exec(line.slice(i));
      out += `<span class="tok-num">${esc(m[0])}</span>`; i += m[0].length; continue;
    }
    if (/[A-Za-z_$@]/.test(c)) {
      const m = /^[@A-Za-z_$][\w$]*/.exec(line.slice(i));
      const w = m[0];
      const after = line.slice(i + w.length).match(/^\s*(\(|<[\w\s,]*>\s*\()/);
      let cls = KW.has(w) ? 'kw' : TYPES.has(w) || /^[A-Z][a-z0-9]+[A-Za-z0-9]*$/.test(w) ? 'type' : after ? 'fn' : '';
      out += cls ? `<span class="tok-${cls}">${esc(w)}</span>` : esc(w);
      i += w.length; continue;
    }
    if (/[{}()[\];,.:=<>+\-*/%!&|^~?]/.test(c)) { out += `<span class="tok-punc">${esc(c)}</span>`; i++; continue; }
    out += esc(c); i++;
  }
  return { html: out, state: null };
}

export function highlightLines(lines, path) {
  const lang = langOf(path);
  let st = null;
  return lines.map(l => { const r = highlightLine(l, lang, st); st = r.state; return r.html; });
}

// ---------- markdown-lite for chat ----------
export function renderMarkdown(src, { onCode } = {}) {
  const root = document.createElement('div');
  const parts = [];
  const re = /```([^\n]*)\n([\s\S]*?)(```|$)/g;
  let last = 0, m;
  while ((m = re.exec(src))) {
    if (m.index > last) parts.push({ t: 'text', v: src.slice(last, m.index) });
    parts.push({ t: 'code', info: m[1].trim(), v: m[2].replace(/\n$/, ''), closed: m[3] === '```' });
    last = re.lastIndex;
    if (!m[3]) break;
  }
  if (last < src.length) parts.push({ t: 'text', v: src.slice(last) });

  for (const p of parts) {
    if (p.t === 'code') {
      const custom = onCode && onCode(p);
      if (custom) { root.append(custom); continue; }
      const pre = document.createElement('pre');
      pre.innerHTML = `<code>${esc(p.v)}</code>`;
      root.append(pre);
      continue;
    }
    root.insertAdjacentHTML('beforeend', inlineBlocks(p.v));
  }
  return root;
}

function inline(s) {
  return esc(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<i>$2</i>');
}

function inlineBlocks(text) {
  const lines = text.split('\n');
  let html = '', list = null, para = [];
  const flushP = () => { if (para.length) { html += `<p>${para.map(inline).join('<br>')}</p>`; para = []; } };
  const flushL = () => { if (list) { html += `<${list.t}>${list.items.map(x => `<li>${inline(x)}</li>`).join('')}</${list.t}>`; list = null; } };
  for (const l of lines) {
    let m;
    if (!l.trim()) { flushP(); flushL(); continue; }
    if ((m = /^(#{1,4})\s+(.*)/.exec(l))) { flushP(); flushL(); html += `<h${m[1].length + 1}>${inline(m[2])}</h${m[1].length + 1}>`; continue; }
    if ((m = /^\s*[-*]\s+(.*)/.exec(l))) { flushP(); if (!list || list.t !== 'ul') { flushL(); list = { t: 'ul', items: [] }; } list.items.push(m[1]); continue; }
    if ((m = /^\s*\d+[.)]\s+(.*)/.exec(l))) { flushP(); if (!list || list.t !== 'ol') { flushL(); list = { t: 'ol', items: [] }; } list.items.push(m[1]); continue; }
    flushL(); para.push(l);
  }
  flushP(); flushL();
  return html;
}

// ---------- toasts ----------
export function toast(text, level = 'info', ms = 4200) {
  const box = document.getElementById('toasts');
  if (!box) return;
  const t = h('div', { class: `toast ${level}` }, text);
  box.append(t);
  setTimeout(() => { t.style.transition = 'opacity .3s'; t.style.opacity = '0'; setTimeout(() => t.remove(), 300); }, ms);
}
