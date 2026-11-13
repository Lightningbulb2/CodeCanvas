import * as vscode from 'vscode';
import { DEFAULT_ENDPOINTS, LlmConfig, SECRET_KEY } from './llm';

/** VS Code settings → LLM config. Kept apart from llm.ts so that file has no editor dependency. */
export async function readLlmConfig(secrets: vscode.SecretStorage): Promise<LlmConfig> {
  const c = vscode.workspace.getConfiguration('codecanvas.llm');
  const typedEndpoint = (c.get<string>('endpoint') || '').trim();
  let provider = (c.get<string>('provider') || 'ollama') as LlmConfig['provider'];
  // No provider chosen but an OpenAI-style endpoint given (…/v1)? Then it isn't Ollama.
  const ins = c.inspect<string>('provider');
  const explicit = ins?.globalValue ?? ins?.workspaceValue ?? ins?.workspaceFolderValue;
  if (!explicit && typedEndpoint && /\/v1\/?$/.test(typedEndpoint)) provider = /:8080\b/.test(typedEndpoint) ? 'llamacpp' : 'openai';
  let endpoint = (typedEndpoint || DEFAULT_ENDPOINTS[provider]).replace(/\/+$/, '');
  if (provider === 'llamacpp' && !/\/v1$/.test(endpoint)) endpoint += '/v1';
  return {
    provider,
    endpoint,
    model: c.get<string>('model') || 'qwen2.5-coder:7b',
    temperature: c.get<number>('temperature') ?? 0.2,
    maxTokens: c.get<number>('maxTokens') ?? 4096,
    apiKey: (await secrets.get(SECRET_KEY)) || process.env.OPENAI_API_KEY || process.env.ANTHROPIC_API_KEY,
  };
}
