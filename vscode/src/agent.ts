import { ChatMessage, LlmConfig, streamChat } from './llm';
import { WebTools } from './web';

/** A piece of context the webview attaches to a request (a snippet, a whole file, a selection). */
export interface ContextItem {
  kind: 'snippet' | 'file';
  label: string;
  path?: string;
  startLine?: number; // 1-based, if the text is a slice of a file
  text: string;
}

export interface DraftSnippet {
  title: string;
  file?: string;
  insertAt?: number;     // 1-based line the code should be inserted before
  replaceStart?: number; // 1-based inclusive
  replaceEnd?: number;   // 1-based inclusive
  code: string;
  lang?: string;
}

export interface ProjectAccess {
  tree(): Promise<string[]>;
  read(path: string): Promise<string | undefined>;
  /** Present only when a SearXNG instance is configured. */
  web?: WebTools;
}

const SNIPPET_PROTOCOL = `
## Drafting snippets
When you propose code, emit it as a snippet block so it appears on the user's canvas as a draggable card:

\`\`\`snippet file="relative/path.ext" insert="LINE" title="Short title"
...code...
\`\`\`

- Use insert="N" to insert the code before line N of that file, or replace="A-B" to replace lines A..B (inclusive, 1-based).
- Omit file/insert/replace when the code is not tied to a location.
- Keep each snippet focused: one function, one block, one change. Several small snippets beat one big one.
- Preserve the file's indentation exactly.

## Reading project files
You have read access to the whole project. To read a file, output exactly <read path="relative/path"/> on its own line and stop.
The file will be returned to you and you can continue. Only request files you actually need.`;

const WEB_PROTOCOL = `
## Searching the web
You can search the web. Output exactly <search query="your search terms"/> on its own line and stop; the results
(title, URL, snippet) are returned to you. To read a page, output <fetch url="https://..."/> on its own line and stop.
Search when the answer depends on documentation, APIs, versions or facts you are unsure of. Mention the URLs you relied on.`;

function numbered(text: string, start = 1): string {
  return text.split('\n').map((l, i) => `${String(i + start).padStart(4)}| ${l}`).join('\n');
}

function renderContext(items: ContextItem[]): string {
  if (!items.length) return '';
  const parts = items.map(it => {
    const where = it.path ? `${it.path}${it.startLine ? ` (lines ${it.startLine}-${it.startLine + it.text.split('\n').length - 1})` : ''}` : '';
    return `### ${it.kind === 'file' ? 'File' : 'Snippet'}: ${it.label}${where && where !== it.label ? ` — ${where}` : ''}\n\`\`\`\n${numbered(it.text, it.startLine || 1)}\n\`\`\``;
  });
  return `# Context on the canvas (line-numbered; the "NNNN| " prefix is not part of the code)\n\n${parts.join('\n\n')}`;
}

export async function buildSystemPrompt(project: ProjectAccess, focus?: string): Promise<string> {
  const files = await project.tree();
  const tree = files.slice(0, 400).join('\n') + (files.length > 400 ? `\n… (${files.length - 400} more)` : '');
  return [
    'You are CodeCanvas, a coding assistant embedded in a visual canvas where code lives as files and "snippets" (movable fragments of code).',
    'Be concise and precise. Prefer showing code as snippet blocks over describing it.',
    focus ? `The user is currently focused on: ${focus}. Questions refer to it unless stated otherwise.` : '',
    SNIPPET_PROTOCOL,
    project.web ? WEB_PROTOCOL : '',
    `## Project files\n${tree}`,
  ].filter(Boolean).join('\n\n');
}

/** Tool tags the model can emit as plain text (works with any model, no native tool-calling needed). */
const TOOL_RE = /<(read|search|fetch)\s+(?:path|query|url)\s*=\s*"([^"]+)"\s*\/?>/g;

/**
 * Runs a chat turn with the tool loop (read project files / search the web / fetch pages).
 * Streams cleaned text deltas through onDelta.
 */
export async function runChat(opts: {
  cfg: LlmConfig;
  project: ProjectAccess;
  history: ChatMessage[];        // user/assistant turns, last one is the new user question
  context: ContextItem[];
  focus?: string;
  maxReads: number;              // total tool calls per answer
  signal: AbortSignal;
  onDelta: (t: string) => void;
  onStatus: (s: string) => void;
}): Promise<{ text: string; snippets: DraftSnippet[] }> {
  const system = await buildSystemPrompt(opts.project, opts.focus);
  const ctx = renderContext(opts.context);
  const msgs: ChatMessage[] = [{ role: 'system', content: system + (ctx ? '\n\n' + ctx : '') }, ...opts.history];

  let shown = '';
  let calls = 0;
  for (;;) {
    let raw = '';
    let emitted = 0;
    let thinking = false;
    const out = await streamChat(opts.cfg, msgs, d => {
      raw += d;
      // Hold back anything that could be the start of a tool tag.
      const hidden = raw.replace(TOOL_RE, '');
      const lt = hidden.lastIndexOf('<');
      const safe = lt >= 0 && !hidden.slice(lt).includes('>') && hidden.length - lt < 300 ? hidden.slice(0, lt) : hidden;
      if (safe.length > emitted) { opts.onDelta(safe.slice(emitted)); emitted = safe.length; }
    }, opts.signal, () => { if (!thinking) { thinking = true; opts.onStatus('thinking…'); } });

    const reqs = [...out.matchAll(TOOL_RE)].map(m => ({ kind: m[1], arg: m[2] }));
    const cleaned = out.replace(TOOL_RE, '');
    if (cleaned.length > emitted) opts.onDelta(cleaned.slice(emitted));
    shown += cleaned;

    if (!reqs.length || calls >= opts.maxReads) break;
    msgs.push({ role: 'assistant', content: out });
    const results: string[] = [];
    const trace: string[] = [];
    for (const r of reqs.slice(0, opts.maxReads - calls)) {
      calls++;
      if (r.kind === 'read') {
        opts.onStatus(`reading ${r.arg}`);
        const t = await opts.project.read(r.arg);
        results.push(t === undefined ? `<file path="${r.arg}">NOT FOUND</file>` : `<file path="${r.arg}">\n${numbered(t.length > 60000 ? t.slice(0, 60000) + '\n…(truncated)' : t)}\n</file>`);
        trace.push(t === undefined ? `*📄 couldn't read ${r.arg}*` : `*📄 read ${r.arg}*`);
      } else if (!opts.project.web) {
        results.push(`<${r.kind}_result>Web access is not configured (codecanvas.web.searxngUrl is empty).</${r.kind}_result>`);
      } else if (r.kind === 'search') {
        opts.onStatus(`searching the web for “${r.arg}”`);
        results.push(`<search_results query="${r.arg}">\n${await opts.project.web.search(r.arg)}\n</search_results>`);
        trace.push(`*🔎 searched the web for “${r.arg}”*`);
      } else {
        opts.onStatus(`reading ${r.arg}`);
        results.push(`<page url="${r.arg}">\n${await opts.project.web.fetchPage(r.arg)}\n</page>`);
        trace.push(`*🌐 read ${r.arg}*`);
      }
    }
    msgs.push({ role: 'user', content: results.join('\n\n') + '\n\nContinue your answer.' });
    const note = (shown && !shown.endsWith('\n') ? '\n' : '') + (trace.length ? trace.join('\n') + '\n\n' : '');
    if (note) { opts.onDelta(note); shown += note; }
  }
  return { text: shown.trim(), snippets: parseSnippets(shown) };
}

export function parseSnippets(text: string): DraftSnippet[] {
  const out: DraftSnippet[] = [];
  const re = /```snippet([^\n]*)\n([\s\S]*?)```/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    const attrs: Record<string, string> = {};
    for (const a of m[1].matchAll(/(\w+)\s*=\s*"([^"]*)"/g)) attrs[a[1]] = a[2];
    const s: DraftSnippet = { title: attrs.title || 'Draft', code: m[2].replace(/\n$/, '') };
    if (attrs.file) s.file = attrs.file;
    if (attrs.lang) s.lang = attrs.lang;
    if (attrs.insert && /^\d+$/.test(attrs.insert)) s.insertAt = +attrs.insert;
    const r = /^(\d+)\s*-\s*(\d+)$/.exec(attrs.replace || '');
    if (r) { s.replaceStart = +r[1]; s.replaceEnd = Math.max(+r[1], +r[2]); }
    out.push(s);
  }
  return out;
}

export interface ExplainSegment { start: number; end: number; title: string; explanation: string; }

/** Asks the model to split a file into logical segments and explain each. Large files are processed in windows. */
export async function explainFile(opts: {
  cfg: LlmConfig; path: string; text: string; signal: AbortSignal;
  onProgress: (segments: ExplainSegment[]) => void;
}): Promise<ExplainSegment[]> {
  const lines = opts.text.split('\n');
  const WINDOW = 320;
  const all: ExplainSegment[] = [];
  for (let off = 0; off < lines.length; off += WINDOW) {
    const slice = lines.slice(off, off + WINDOW).join('\n');
    const lastLine = Math.min(lines.length, off + WINDOW);
    const prompt = `Split this part of \`${opts.path}\` (lines ${off + 1}-${lastLine}) into logical, contiguous segments (imports, a function, a class method, a config block…), covering every line from ${off + 1} to ${lastLine} with no gaps or overlaps. Aim for segments of 5-60 lines.
For each segment give a short title and a 1-3 sentence explanation of what it does and why.

Respond with ONLY a JSON array, no prose:
[{"start": ${off + 1}, "end": 12, "title": "…", "explanation": "…"}, …]

\`\`\`
${numbered(slice, off + 1)}
\`\`\``;
    let raw = '';
    try {
      raw = await streamChat(opts.cfg, [
        { role: 'system', content: 'You are a precise code explainer. You output strict JSON only.' },
        { role: 'user', content: prompt },
      ], () => { /* not streamed to UI */ }, opts.signal);
    } catch (e) {
      if (opts.signal.aborted) throw e;
      throw e;
    }
    let segs = parseSegments(raw, off + 1, lastLine);
    if (!segs.length) segs = fallbackSegments(lines, off, lastLine);
    all.push(...segs);
    opts.onProgress([...all]);
  }
  return all;
}

function parseSegments(raw: string, lo: number, hi: number): ExplainSegment[] {
  const a = raw.indexOf('['), b = raw.lastIndexOf(']');
  if (a < 0 || b <= a) return [];
  let arr: any[];
  try { arr = JSON.parse(raw.slice(a, b + 1)); } catch {
    try { arr = JSON.parse(raw.slice(a, b + 1).replace(/,\s*([\]}])/g, '$1')); } catch { return []; }
  }
  const segs = arr
    .filter(s => s && Number.isFinite(+s.start) && Number.isFinite(+s.end))
    .map(s => ({
      start: Math.max(lo, Math.min(hi, Math.round(+s.start))),
      end: Math.max(lo, Math.min(hi, Math.round(+s.end))),
      title: String(s.title || 'Segment').slice(0, 80),
      explanation: String(s.explanation || ''),
    }))
    .filter(s => s.end >= s.start)
    .sort((x, y) => x.start - y.start);
  // Remove overlaps (models are sloppy).
  const out: ExplainSegment[] = [];
  let cursor = lo;
  for (const s of segs) {
    if (s.end < cursor) continue;
    s.start = Math.max(s.start, cursor);
    out.push(s);
    cursor = s.end + 1;
  }
  return out;
}

function fallbackSegments(lines: string[], off: number, last: number): ExplainSegment[] {
  const out: ExplainSegment[] = [];
  let start = off + 1;
  for (let i = off; i < last; i++) {
    const blank = !lines[i].trim();
    if ((blank && i + 1 - start >= 8) || i + 1 - start >= 40 || i === last - 1) {
      out.push({ start, end: i + 1, title: `Lines ${start}-${i + 1}`, explanation: '(The model did not return a usable breakdown for this part.)' });
      start = i + 2;
    }
  }
  return out;
}
