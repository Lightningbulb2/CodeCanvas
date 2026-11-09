// Web tools for the model: search through a SearXNG instance, and fetch pages as plain text.

export interface WebTools {
  search(query: string): Promise<string>;
  fetchPage(url: string): Promise<string>;
}

const UA = 'CodeCanvas (VS Code extension)';

export function makeWebTools(searxngUrl: string, maxResults: number, signal: AbortSignal): WebTools {
  const base = searxngUrl.trim().replace(/\/+$/, '');
  return {
    async search(query) {
      const url = `${base}/search?q=${encodeURIComponent(query)}&format=json`;
      let res: Response;
      try {
        res = await fetch(url, { headers: { accept: 'application/json', 'user-agent': UA }, signal: timeout(signal, 15000) });
      } catch (e: any) {
        return `SEARCH FAILED: couldn't reach SearXNG at ${base} (${e?.cause?.code || e?.message || e}).`;
      }
      if (res.status === 403 || res.status === 429) {
        return `SEARCH FAILED: SearXNG answered HTTP ${res.status}. JSON output is probably disabled — add "json" under search.formats in its settings.yml.`;
      }
      if (!res.ok) return `SEARCH FAILED: SearXNG answered HTTP ${res.status}.`;
      let j: any;
      try { j = await res.json(); } catch { return 'SEARCH FAILED: SearXNG did not return JSON (enable "json" under search.formats in settings.yml).'; }
      const results = (j.results || []).slice(0, maxResults);
      if (!results.length) return `No results for "${query}".`;
      const lines = results.map((r: any, i: number) =>
        `${i + 1}. ${clean(r.title || '(untitled)')}\n   ${r.url}\n   ${clean(r.content || '').slice(0, 300)}`);
      const answers = (j.answers || []).map((a: any) => (typeof a === 'string' ? a : a?.answer)).filter(Boolean);
      return (answers.length ? `Instant answer: ${clean(answers[0])}\n\n` : '') + lines.join('\n');
    },

    async fetchPage(url) {
      if (!/^https?:\/\//i.test(url)) return `FETCH FAILED: only http(s) URLs can be fetched.`;
      let res: Response;
      try {
        res = await fetch(url, { headers: { 'user-agent': UA, accept: 'text/html,text/plain,application/json;q=0.9,*/*;q=0.5' }, signal: timeout(signal, 20000), redirect: 'follow' });
      } catch (e: any) {
        return `FETCH FAILED: ${e?.cause?.code || e?.message || e}`;
      }
      if (!res.ok) return `FETCH FAILED: HTTP ${res.status}.`;
      const type = res.headers.get('content-type') || '';
      if (!/text|json|xml|javascript/.test(type)) return `FETCH FAILED: ${type || 'unknown'} content can't be read as text.`;
      const raw = await res.text();
      const text = /html/.test(type) ? htmlToText(raw) : raw;
      return text.length > 12000 ? text.slice(0, 12000) + '\n…(truncated)' : text;
    },
  };
}

function timeout(parent: AbortSignal, ms: number): AbortSignal {
  const ac = new AbortController();
  const t = setTimeout(() => ac.abort(), ms);
  parent.addEventListener('abort', () => ac.abort(), { once: true });
  ac.signal.addEventListener('abort', () => clearTimeout(t), { once: true });
  return ac.signal;
}

const clean = (s: string) => decode(String(s).replace(/<[^>]+>/g, '')).replace(/\s+/g, ' ').trim();

function decode(s: string): string {
  return s
    .replace(/&nbsp;/g, ' ').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;|&apos;/g, "'")
    .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(+n)).replace(/&#x([0-9a-f]+);/gi, (_, n) => String.fromCodePoint(parseInt(n, 16)))
    .replace(/&amp;/g, '&');
}

/** Rough but dependable HTML → readable text (keeps code blocks and line structure). */
export function htmlToText(html: string): string {
  let s = html
    .replace(/<(script|style|noscript|svg|nav|footer|header|form|iframe)[\s\S]*?<\/\1>/gi, '')
    .replace(/<!--[\s\S]*?-->/g, '');
  const main = /<(main|article)[\s\S]*?<\/\1>/i.exec(s);
  if (main && main[0].length > 500) s = main[0];
  s = s
    .replace(/<pre[^>]*>([\s\S]*?)<\/pre>/gi, (_, c) => '\n```\n' + c.replace(/<[^>]+>/g, '') + '\n```\n')
    .replace(/<(br|hr)\s*\/?>/gi, '\n')
    .replace(/<\/(p|div|li|h[1-6]|tr|section|article|blockquote|table)>/gi, '\n')
    .replace(/<li[^>]*>/gi, '- ')
    .replace(/<h([1-6])[^>]*>/gi, (_, n) => '\n' + '#'.repeat(+n) + ' ')
    .replace(/<[^>]+>/g, '');
  return decode(s).replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').replace(/[ \t]{2,}/g, ' ').trim();
}
