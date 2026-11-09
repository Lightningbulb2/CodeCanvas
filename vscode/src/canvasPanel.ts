import * as vscode from 'vscode';
import { ContextItem, explainFile, ProjectAccess, runChat } from './agent';
import { ChatMessage, listModels } from './llm';
import { readLlmConfig } from './llmConfig';
import { makeWebTools } from './web';

const STATE_KEY = 'codecanvas.state.v1';

export class CanvasPanel implements ProjectAccess {
  static current: CanvasPanel | undefined;

  private readonly disposables: vscode.Disposable[] = [];
  private readonly running = new Map<string, AbortController>();
  private readonly watched = new Set<string>();
  private readonly selfWrites = new Map<string, number>();
  private fileIndex = new Map<string, vscode.Uri>();
  private ready = false;
  private queue: any[] = [];

  static async show(ctx: vscode.ExtensionContext): Promise<CanvasPanel> {
    if (CanvasPanel.current) {
      CanvasPanel.current.panel.reveal(undefined, false);
      return CanvasPanel.current;
    }
    const panel = vscode.window.createWebviewPanel(
      'codecanvas.canvas',
      'CodeCanvas',
      { viewColumn: vscode.ViewColumn.Active, preserveFocus: false },
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [vscode.Uri.joinPath(ctx.extensionUri, 'media')],
      },
    );
    panel.iconPath = vscode.Uri.joinPath(ctx.extensionUri, 'media', 'icon.svg');
    CanvasPanel.current = new CanvasPanel(panel, ctx);
    if (vscode.workspace.getConfiguration('codecanvas').get<boolean>('openInNewWindow', true)) {
      try {
        // Pops the active editor (our panel) out into its own desktop window (VS Code ≥ 1.85).
        await vscode.commands.executeCommand('workbench.action.moveEditorToNewWindow');
      } catch { /* older VS Code: stays as a tab */ }
    }
    return CanvasPanel.current;
  }

  private constructor(private readonly panel: vscode.WebviewPanel, private readonly ctx: vscode.ExtensionContext) {
    panel.webview.html = this.html();
    panel.onDidDispose(() => this.dispose(), null, this.disposables);
    panel.webview.onDidReceiveMessage(m => this.onMessage(m).catch(e => this.post({ type: 'toast', level: 'error', text: String(e?.message || e) })), null, this.disposables);

    const w = vscode.workspace.createFileSystemWatcher('**/*');
    const changed = async (uri: vscode.Uri) => {
      const rel = this.rel(uri);
      const t = this.selfWrites.get(rel);
      if (t && Date.now() - t < 1500) return;
      if (!this.watched.has(rel)) return;
      const text = await this.read(rel);
      if (text !== undefined) this.post({ type: 'fileChanged', path: rel, text });
    };
    w.onDidChange(changed, null, this.disposables);
    w.onDidCreate(() => this.refreshTree(), null, this.disposables);
    w.onDidDelete(() => this.refreshTree(), null, this.disposables);
    this.disposables.push(w);
    vscode.workspace.onDidChangeConfiguration(e => {
      if (e.affectsConfiguration('codecanvas')) this.sendConfig();
    }, null, this.disposables);
  }

  /** Send a message; queued until the webview reports ready. */
  post(msg: any) {
    if (!this.ready) { this.queue.push(msg); return; }
    this.panel.webview.postMessage(msg);
  }

  command(cmd: any) { this.post({ type: 'command', ...cmd }); }

  // ---------- ProjectAccess ----------
  async tree(): Promise<string[]> {
    const exclude = vscode.workspace.getConfiguration('codecanvas').get<string>('exclude') || undefined;
    const uris = await vscode.workspace.findFiles('**/*', exclude, 8000);
    this.fileIndex = new Map(uris.map(u => [this.rel(u), u]));
    return [...this.fileIndex.keys()].sort((a, b) => a.localeCompare(b));
  }

  async read(rel: string): Promise<string | undefined> {
    const uri = this.resolve(rel);
    if (!uri) return undefined;
    try {
      const open = vscode.workspace.textDocuments.find(d => d.uri.toString() === uri.toString());
      if (open) return open.getText();
      const bytes = await vscode.workspace.fs.readFile(uri);
      if (bytes.subarray(0, 8000).includes(0)) return undefined; // binary
      return new TextDecoder().decode(bytes);
    } catch { return undefined; }
  }

  // ---------- helpers ----------
  private rel(uri: vscode.Uri) {
    const multi = (vscode.workspace.workspaceFolders?.length || 0) > 1;
    return vscode.workspace.asRelativePath(uri, multi).replace(/\\/g, '/');
  }

  private resolve(rel: string): vscode.Uri | undefined {
    rel = rel.replace(/\\/g, '/').replace(/^\.\//, '');
    const hit = this.fileIndex.get(rel);
    if (hit) return hit;
    const folders = vscode.workspace.workspaceFolders || [];
    if (!folders.length) return undefined;
    if (folders.length > 1) {
      const [head, ...rest] = rel.split('/');
      const f = folders.find(x => x.name === head);
      if (f) return vscode.Uri.joinPath(f.uri, ...rest);
    }
    return vscode.Uri.joinPath(folders[0].uri, ...rel.split('/'));
  }

  private async refreshTree() {
    this.post({ type: 'files', files: await this.tree() });
  }

  private async sendConfig() {
    const cfg = await readLlmConfig(this.ctx.secrets);
    const searx = (vscode.workspace.getConfiguration('codecanvas.web').get<string>('searxngUrl') || '').trim();
    this.post({ type: 'config', provider: cfg.provider, model: cfg.model, endpoint: cfg.endpoint, hasKey: !!cfg.apiKey, web: searx });
  }

  private async onMessage(m: any) {
    switch (m.type) {
      case 'ready': {
        this.ready = true;
        const files = await this.tree();
        this.panel.webview.postMessage({
          type: 'init',
          files,
          state: this.ctx.workspaceState.get(STATE_KEY),
          workspace: vscode.workspace.name || 'No folder open',
        });
        await this.sendConfig();
        for (const q of this.queue.splice(0)) this.panel.webview.postMessage(q);
        break;
      }
      case 'readFile': {
        const text = await this.read(m.path);
        if (text === undefined) { this.post({ type: 'toast', level: 'error', text: `Can't open ${m.path} (missing or binary)` }); break; }
        this.watched.add(m.path);
        this.post({ type: 'fileContent', path: m.path, text, reqId: m.reqId });
        break;
      }
      case 'saveFile': {
        const uri = this.resolve(m.path);
        if (!uri) break;
        this.selfWrites.set(m.path, Date.now());
        const open = vscode.workspace.textDocuments.find(d => d.uri.toString() === uri.toString());
        if (open && open.isDirty) {
          // Respect an open VS Code buffer: edit it and save, so undo history stays intact.
          const edit = new vscode.WorkspaceEdit();
          edit.replace(uri, new vscode.Range(0, 0, open.lineCount, 0), m.text);
          await vscode.workspace.applyEdit(edit);
          await open.save();
        } else {
          await vscode.workspace.fs.writeFile(uri, new TextEncoder().encode(m.text));
        }
        this.post({ type: 'saved', path: m.path });
        break;
      }
      case 'confirm': {
        const pick = await vscode.window.showWarningMessage(m.message, { modal: true }, m.action || 'OK');
        this.post({ type: 'confirmResult', reqId: m.reqId, ok: !!pick });
        break;
      }
      case 'refreshFiles':
        await this.refreshTree();
        break;
      case 'saveState':
        await this.ctx.workspaceState.update(STATE_KEY, m.state);
        break;
      case 'openInEditor': {
        const uri = this.resolve(m.path);
        if (!uri) break;
        const doc = await vscode.workspace.openTextDocument(uri);
        const line = Math.max(0, (m.line || 1) - 1);
        await vscode.window.showTextDocument(doc, { selection: new vscode.Range(line, 0, line, 0), viewColumn: vscode.ViewColumn.One });
        break;
      }
      case 'openSettings':
        await vscode.commands.executeCommand('workbench.action.openSettings', 'codecanvas');
        break;
      case 'setApiKey':
        await vscode.commands.executeCommand('codecanvas.setApiKey');
        break;
      case 'listModels': {
        const cfg = await readLlmConfig(this.ctx.secrets);
        this.post({ type: 'models', models: await listModels(cfg), current: cfg.model });
        break;
      }
      case 'setModel':
        await vscode.workspace.getConfiguration('codecanvas.llm').update('model', m.model, vscode.ConfigurationTarget.Global);
        break;
      case 'setSearxng':
        await vscode.commands.executeCommand('codecanvas.setSearxng');
        break;
      case 'setProvider':
        await vscode.workspace.getConfiguration('codecanvas.llm').update('provider', m.provider, vscode.ConfigurationTarget.Global);
        break;
      case 'cancel':
        this.running.get(m.id)?.abort();
        break;
      case 'chat':
        await this.chat(m.id, m.history as ChatMessage[], m.context as ContextItem[], m.focus);
        break;
      case 'explain':
        await this.explain(m.id, m.path, m.text);
        break;
      case 'listAll': {
        // Text files for Explain All, biggest-first source files are usually most interesting; keep it simple: sorted by path.
        const max = vscode.workspace.getConfiguration('codecanvas').get<number>('explainAll.maxFiles', 40);
        const files = (await this.tree()).filter(f => !/\.(png|jpe?g|gif|ico|svg|woff2?|ttf|lock|map|min\.js|pdf|zip|exe|dll|bin)$/i.test(f) && !/(^|\/)(package-lock\.json|yarn\.lock|pnpm-lock\.yaml)$/.test(f));
        this.post({ type: 'allFiles', reqId: m.reqId, files: files.slice(0, max), total: files.length });
        break;
      }
    }
  }

  private async chat(id: string, history: ChatMessage[], context: ContextItem[], focus?: string) {
    const ac = new AbortController();
    this.running.set(id, ac);
    try {
      const cfg = await readLlmConfig(this.ctx.secrets);
      const maxReads = vscode.workspace.getConfiguration('codecanvas.llm').get<number>('maxFileReads', 6);
      const wc = vscode.workspace.getConfiguration('codecanvas.web');
      const searx = (wc.get<string>('searxngUrl') || '').trim();
      const project: ProjectAccess = {
        tree: () => this.tree(),
        read: p => this.read(p),
        web: searx ? makeWebTools(searx, wc.get<number>('maxResults', 6), ac.signal) : undefined,
      };
      const res = await runChat({
        cfg, project, history, context, focus, maxReads, signal: ac.signal,
        onDelta: text => this.post({ type: 'chatDelta', id, text }),
        onStatus: status => this.post({ type: 'chatStatus', id, status }),
      });
      this.post({ type: 'chatDone', id, text: res.text, snippets: res.snippets });
    } catch (e: any) {
      this.post({ type: 'chatError', id, error: ac.signal.aborted ? 'Stopped.' : this.friendly(e) });
    } finally {
      this.running.delete(id);
    }
  }

  private async explain(id: string, path: string, text: string) {
    const ac = new AbortController();
    this.running.set(id, ac);
    try {
      const cfg = await readLlmConfig(this.ctx.secrets);
      const segments = await explainFile({
        cfg, path, text, signal: ac.signal,
        onProgress: segs => this.post({ type: 'explainProgress', id, path, segments: segs }),
      });
      this.post({ type: 'explainDone', id, path, segments });
    } catch (e: any) {
      this.post({ type: 'explainError', id, path, error: ac.signal.aborted ? 'Stopped.' : this.friendly(e) });
    } finally {
      this.running.delete(id);
    }
  }

  private friendly(e: any): string {
    const msg = String(e?.cause?.code || e?.message || e);
    if (/ECONNREFUSED|fetch failed/i.test(msg)) {
      const p = vscode.workspace.getConfiguration('codecanvas.llm').get<string>('provider');
      return `Couldn't reach the ${p} server (${msg}). Is it running? Check CodeCanvas settings → endpoint.`;
    }
    return msg;
  }

  private html(): string {
    const w = this.panel.webview;
    const media = (...p: string[]) => w.asWebviewUri(vscode.Uri.joinPath(this.ctx.extensionUri, 'media', ...p));
    const nonce = Array.from({ length: 24 }, () => Math.random().toString(36)[2]).join('');
    return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${w.cspSource} data:; style-src ${w.cspSource} 'unsafe-inline'; font-src ${w.cspSource}; script-src 'nonce-${nonce}' ${w.cspSource};">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<link rel="stylesheet" href="${media('webview', 'canvas.css')}">
<title>CodeCanvas</title>
</head>
<body>
<script nonce="${nonce}">window.CC_HOST = 'vscode';</script>
<script nonce="${nonce}" type="module" src="${media('webview', 'main.js')}"></script>
</body>
</html>`;
  }

  dispose() {
    CanvasPanel.current = undefined;
    for (const ac of this.running.values()) ac.abort();
    this.disposables.forEach(d => d.dispose());
  }
}
