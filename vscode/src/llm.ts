export interface ChatMessage {
  role: 'system' | 'user' | 'assistant';
  content: string;
}

export interface LlmConfig {
  provider: 'ollama' | 'openai' | 'llamacpp' | 'anthropic';
  endpoint: string;
  model: string;
  temperature: number;
  maxTokens: number;
  apiKey?: string;
}

export const DEFAULT_ENDPOINTS: Record<LlmConfig['provider'], string> = {
  ollama: 'http://localhost:11434',
  openai: 'http://localhost:1234/v1',
  llamacpp: 'http://localhost:8080/v1',
  anthropic: 'https://api.anthropic.com',
};

/** llama.cpp's llama-server speaks the OpenAI protocol. */
const wire = (p: LlmConfig['provider']) => (p === 'llamacpp' ? 'openai' : p);

export const SECRET_KEY = 'codecanvas.apiKey';

/** Reads a streaming HTTP body line by line. */
async function* lines(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader();
  const dec = new TextDecoder();
  let buf = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, nl).replace(/\r$/, '');
      buf = buf.slice(nl + 1);
      yield line;
    }
  }
  if (buf.trim()) yield buf;
}

async function failText(res: Response, cfg?: LlmConfig): Promise<string> {
  let t = '';
  try { t = await res.text(); } catch { /* ignore */ }
  let hint = '';
  if (res.status === 404 && cfg) {
    hint = cfg.provider === 'ollama'
      ? ` — ${res.url} doesn't exist. If this is llama.cpp, LM Studio or another OpenAI-style server, set codecanvas.llm.provider to "llamacpp" or "openai".`
      : ` — ${res.url} doesn't exist. Check codecanvas.llm.endpoint (OpenAI-style servers usually end in /v1).`;
  }
  return `HTTP ${res.status} ${res.statusText}${t ? ': ' + t.slice(0, 500) : ''}${hint}`;
}

/**
 * Streams a chat completion. Calls onDelta for each text chunk and resolves with the full text.
 */
export async function streamChat(
  cfg: LlmConfig,
  messages: ChatMessage[],
  onDelta: (t: string) => void,
  signal: AbortSignal,
  onThinking?: () => void,
): Promise<string> {
  let full = '';
  const emit = (t: string) => { if (t) { full += t; onDelta(t); } };

  if (wire(cfg.provider) === 'ollama') {
    const res = await fetch(`${cfg.endpoint}/api/chat`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        model: cfg.model,
        messages,
        stream: true,
        options: { temperature: cfg.temperature, num_predict: cfg.maxTokens, num_ctx: 16384 },
      }),
      signal,
    });
    if (!res.ok || !res.body) throw new Error(await failText(res, cfg));
    for await (const line of lines(res.body)) {
      if (!line.trim()) continue;
      const j = JSON.parse(line);
      if (j.error) throw new Error(j.error);
      emit(j.message?.content ?? '');
      if (j.done) break;
    }
    return full;
  }

  if (wire(cfg.provider) === 'openai') {
    const headers: Record<string, string> = { 'content-type': 'application/json' };
    if (cfg.apiKey) headers.authorization = `Bearer ${cfg.apiKey}`;
    const res = await fetch(`${cfg.endpoint}/chat/completions`, {
      method: 'POST',
      headers,
      body: JSON.stringify({
        model: cfg.model,
        messages,
        stream: true,
        temperature: cfg.temperature,
        max_tokens: cfg.maxTokens,
      }),
      signal,
    });
    if (!res.ok || !res.body) throw new Error(await failText(res, cfg));
    for await (const line of lines(res.body)) {
      const m = /^data:\s*(.*)$/.exec(line);
      if (!m) continue;
      if (m[1] === '[DONE]') break;
      try {
        const j = JSON.parse(m[1]);
        const d = j.choices?.[0]?.delta;
        // reasoning models (llama.cpp reasoning_content) think before answering — show that, don't print it
        if (d?.reasoning_content) onThinking?.();
        emit(d?.content ?? '');
      } catch { /* partial / keep-alive */ }
    }
    return full;
  }

  // anthropic
  if (!cfg.apiKey) throw new Error('No API key set. Run "CodeCanvas: Set LLM API Key".');
  const system = messages.filter(m => m.role === 'system').map(m => m.content).join('\n\n');
  const convo = messages.filter(m => m.role !== 'system');
  const res = await fetch(`${cfg.endpoint}/v1/messages`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-api-key': cfg.apiKey,
      'anthropic-version': '2023-06-01',
    },
    body: JSON.stringify({ model: cfg.model, system, messages: convo, max_tokens: cfg.maxTokens, stream: true }),
    signal,
  });
  if (!res.ok || !res.body) throw new Error(await failText(res, cfg));
  for await (const line of lines(res.body)) {
    const m = /^data:\s*(.*)$/.exec(line);
    if (!m) continue;
    try {
      const j = JSON.parse(m[1]);
      if (j.type === 'content_block_delta' && j.delta?.type === 'text_delta') emit(j.delta.text);
      if (j.type === 'error') throw new Error(j.error?.message || 'Anthropic error');
      if (j.type === 'message_stop') break;
    } catch (e) {
      if (e instanceof Error && !(e instanceof SyntaxError)) throw e;
    }
  }
  return full;
}

export async function listModels(cfg: LlmConfig): Promise<string[]> {
  try {
    if (wire(cfg.provider) === 'ollama') {
      const r = await fetch(`${cfg.endpoint}/api/tags`);
      const j: any = await r.json();
      return (j.models || []).map((m: any) => m.name);
    }
    if (wire(cfg.provider) === 'openai') {
      const headers: Record<string, string> = {};
      if (cfg.apiKey) headers.authorization = `Bearer ${cfg.apiKey}`;
      const r = await fetch(`${cfg.endpoint}/models`, { headers });
      const j: any = await r.json();
      return (j.data || []).map((m: any) => m.id);
    }
    if (cfg.apiKey) {
      const r = await fetch(`${cfg.endpoint}/v1/models`, {
        headers: { 'x-api-key': cfg.apiKey, 'anthropic-version': '2023-06-01' },
      });
      const j: any = await r.json();
      return (j.data || []).map((m: any) => m.id);
    }
  } catch { /* server not running */ }
  return [];
}
