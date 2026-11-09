// A chat thread UI, used by snippet drawers and the global (unfocused) chat panel.
import { send, stream } from './bridge.js';
import { h, esc, icons, renderMarkdown, uid } from './util.js';

/**
 * opts:
 *  messages()            -> mutable array of {role, content, spawned?} owned by the caller (persisted)
 *  context()             -> { items: ContextItem[], focus?: string }
 *  onSnippets(snips,msg) -> called with drafted snippets when a reply finishes; returns spawned ids
 *  onCodeToSnippet(code, info)
 *  onFocusSnippet(id)
 *  onChange()            -> persist
 *  placeholder, empty
 */
export function createChat(opts) {
  const msgsEl = h('div', { class: 'msgs' });
  const ta = h('textarea', { rows: 1, placeholder: opts.placeholder || 'Ask…' });
  const btn = h('button', { class: 'send-btn', title: 'Send (Enter)', html: icons.send });
  const el = h('div', { class: 'chat' }, msgsEl, h('div', { class: 'composer' }, ta, btn));
  let busy = null; // { id, el, text }

  const stopWheel = e => { if (!e.ctrlKey && msgsEl.scrollHeight > msgsEl.clientHeight) e.stopPropagation(); };
  msgsEl.addEventListener('wheel', stopWheel, { passive: true });
  el.addEventListener('mousedown', e => e.stopPropagation());
  el.addEventListener('dblclick', e => e.stopPropagation());
  el.addEventListener('contextmenu', e => e.stopPropagation());
  ta.addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); }
  });
  ta.addEventListener('input', () => { ta.style.height = 'auto'; ta.style.height = Math.min(140, ta.scrollHeight) + 'px'; });
  btn.addEventListener('click', () => (busy ? cancel() : submit()));

  function codeRenderer(msg) {
    return part => {
      if (part.info.startsWith('snippet')) {
        const attrs = Object.fromEntries([...part.info.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)].map(m => [m[1], m[2]]));
        const where = attrs.file ? `${attrs.file}${attrs.replace ? ':' + attrs.replace : attrs.insert ? ':' + attrs.insert : ''}` : opts.onCodeToDraft ? '→ draft' : 'unplaced';
        const chip = h('div', { class: 'snip-chip', title: 'Show snippet on canvas' },
          h('span', { html: icons.sparkle, style: { width: '14px', height: '14px', display: 'inline-flex' } }),
          h('span', { class: 'sc-t' }, attrs.title || 'Draft snippet'),
          h('span', { class: 'sc-w' }, part.closed ? where : 'drafting…'));
        const idx = (msg.content.slice(0, msg.content.indexOf(part.v)).match(/```snippet/g) || []).length - 1;
        chip.addEventListener('click', () => {
          const id = msg.spawned?.[idx];
          if (id) opts.onFocusSnippet?.(id);
          else opts.onCodeToSnippet?.(part.v, part.info, attrs);
        });
        return chip;
      }
      const pre = h('pre', {}, h('code', { html: esc(part.v) }));
      const b = h('button', { class: 'to-snip', title: 'Turn this code into a snippet card' }, '→ Snippet');
      b.addEventListener('click', () => opts.onCodeToSnippet?.(part.v, part.info, {}));
      pre.append(b);
      if (opts.onCodeToDraft) {
        const d = h('button', { class: 'to-snip to-draft', title: 'Use this code as the snippet draft (shown as a diff)' }, '→ Draft');
        d.addEventListener('click', () => opts.onCodeToDraft(part.v));
        pre.append(d);
      }
      return pre;
    };
  }

  function msgEl(m) {
    if (m.role === 'user') return h('div', { class: 'msg user' }, m.content);
    if (m.role === 'error') return h('div', { class: 'msg error' }, m.content);
    if (m.role === 'note') {
      const d = h('div', { class: 'msg note' }, h('div', { class: 'note-h' }, m.title || 'Explanation'));
      d.append(renderMarkdown(m.content));
      return d;
    }
    const d = h('div', { class: 'msg assistant' });
    d.append(renderMarkdown(m.content, { onCode: codeRenderer(m) }));
    return d;
  }

  function render() {
    msgsEl.replaceChildren();
    const msgs = opts.messages();
    if (!msgs.length && opts.empty) msgsEl.append(h('div', { class: 'empty', html: opts.empty }));
    for (const m of msgs) msgsEl.append(msgEl(m));
    if (busy) msgsEl.append(busy.el);
    msgsEl.scrollTop = msgsEl.scrollHeight;
  }

  let raf = 0;
  function paintBusy() {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      if (!busy) return;
      const stick = msgsEl.scrollHeight - msgsEl.scrollTop - msgsEl.clientHeight < 40;
      busy.el.replaceChildren();
      if (busy.text) busy.el.append(renderMarkdown(busy.text, { onCode: codeRenderer({ content: busy.text }) }));
      if (busy.status) busy.el.append(h('div', { class: 'status' }, busy.status));
      if (!busy.text) busy.el.append(h('span', { class: 'typing', html: '<i></i><i></i><i></i>' }));
      if (stick) msgsEl.scrollTop = msgsEl.scrollHeight;
    });
  }

  function submit(textOverride) {
    const text = (textOverride ?? ta.value).trim();
    if (!text || busy) return;
    ta.value = ''; ta.style.height = '';
    const msgs = opts.messages();
    msgs.push({ role: 'user', content: text });
    const history = msgs
      .filter(m => m.role === 'user' || m.role === 'assistant' || m.role === 'note')
      .map(m => ({ role: m.role === 'note' ? 'assistant' : m.role, content: m.content }));
    const { items, focus } = opts.context();
    const id = uid('chat');
    busy = { id, el: h('div', { class: 'msg assistant' }), text: '', status: '' };
    btn.classList.add('stop'); btn.innerHTML = icons.stop; btn.title = 'Stop';
    render(); paintBusy();
    opts.onChange?.();

    const off = stream(id, m => {
      if (m.type === 'chatDelta') { busy.text += m.text; busy.status = ''; paintBusy(); }
      else if (m.type === 'chatStatus') { busy.status = m.status; paintBusy(); }
      else if (m.type === 'chatDone' || m.type === 'chatError') {
        off();
        const finalText = m.type === 'chatDone' ? m.text : busy.text;
        busy = null;
        btn.classList.remove('stop'); btn.innerHTML = icons.send; btn.title = 'Send (Enter)';
        if (finalText) {
          const msg = { role: 'assistant', content: finalText };
          msgs.push(msg);
          if (m.snippets?.length) msg.spawned = opts.onSnippets?.(m.snippets, msg) || [];
        }
        if (m.type === 'chatError') msgs.push({ role: 'error', content: m.error });
        render();
        opts.onChange?.();
      }
    });
    send({ type: 'chat', id, history, context: items, focus });
  }

  function cancel() { if (busy) send({ type: 'cancel', id: busy.id }); }

  render();
  return {
    el, render, submit,
    focus: () => ta.focus({ preventScroll: true }),
    get busy() { return !!busy; },
    setDraft: t => { ta.value = t; ta.focus({ preventScroll: true }); ta.dispatchEvent(new Event('input')); },
  };
}
