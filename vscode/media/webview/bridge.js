// Messaging with the extension host.
const vscode = typeof acquireVsCodeApi === 'function' ? acquireVsCodeApi() : { postMessage: m => console.log('[host]', m) };

const handlers = new Map();
const waiters = new Map();
let seq = 0;

window.addEventListener('message', e => {
  const m = e.data;
  if (!m || !m.type) return;
  if (m.reqId && waiters.has(m.reqId)) { waiters.get(m.reqId)(m); waiters.delete(m.reqId); }
  for (const fn of handlers.get(m.type) || []) fn(m);
});

export const send = m => vscode.postMessage(m);

export function on(type, fn) {
  if (!handlers.has(type)) handlers.set(type, []);
  handlers.get(type).push(fn);
}

/** Request/response helper keyed by reqId. */
export function request(msg, timeout = 15000) {
  const reqId = `r${++seq}`;
  return new Promise((res, rej) => {
    waiters.set(reqId, res);
    send({ ...msg, reqId });
    setTimeout(() => { if (waiters.delete(reqId)) rej(new Error('timeout')); }, timeout);
  });
}

/** Stream-style listeners for a single id (chat / explain). */
const streams = new Map();
for (const t of ['chatDelta', 'chatStatus', 'chatDone', 'chatError', 'explainProgress', 'explainDone', 'explainError']) {
  on(t, m => streams.get(m.id)?.(m));
}
export function stream(id, fn) { streams.set(id, fn); return () => streams.delete(id); }
