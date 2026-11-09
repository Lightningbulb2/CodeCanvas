import * as vscode from 'vscode';
import { CanvasPanel } from './canvasPanel';
import { SECRET_KEY } from './llm';

export function activate(ctx: vscode.ExtensionContext) {
  const open = () => CanvasPanel.show(ctx);

  const relOf = (uri?: vscode.Uri) => {
    uri = uri ?? vscode.window.activeTextEditor?.document.uri;
    if (!uri) return undefined;
    const multi = (vscode.workspace.workspaceFolders?.length || 0) > 1;
    return vscode.workspace.asRelativePath(uri, multi).replace(/\\/g, '/');
  };

  ctx.subscriptions.push(
    vscode.commands.registerCommand('codecanvas.open', open),
    vscode.commands.registerCommand('codecanvas.openFile', async (uri?: vscode.Uri) => {
      const p = relOf(uri);
      (await open()).command({ cmd: 'openFile', path: p });
    }),
    vscode.commands.registerCommand('codecanvas.explainFile', async (uri?: vscode.Uri) => {
      const p = relOf(uri);
      (await open()).command({ cmd: 'explainFile', path: p });
    }),
    vscode.commands.registerCommand('codecanvas.explainAll', async () => {
      (await open()).command({ cmd: 'explainAll' });
    }),
    vscode.commands.registerCommand('codecanvas.setSearxng', async () => {
      const cfg = vscode.workspace.getConfiguration('codecanvas.web');
      const url = await vscode.window.showInputBox({
        title: 'SearXNG URL for web search',
        prompt: 'e.g. http://localhost:8888. Its settings.yml must list "json" under search.formats. Leave empty to turn web search off.',
        value: cfg.get<string>('searxngUrl') || '',
        ignoreFocusOut: true,
      });
      if (url === undefined) return;
      await cfg.update('searxngUrl', url.trim(), vscode.ConfigurationTarget.Global);
      vscode.window.showInformationMessage(url.trim() ? `CodeCanvas web search → ${url.trim()}` : 'CodeCanvas web search turned off.');
    }),
    vscode.commands.registerCommand('codecanvas.setApiKey', async () => {
      const key = await vscode.window.showInputBox({
        title: 'CodeCanvas API key',
        prompt: 'Stored in VS Code SecretStorage. Leave empty to clear. (Not needed for Ollama / LM Studio.)',
        password: true,
        ignoreFocusOut: true,
      });
      if (key === undefined) return;
      if (key) await ctx.secrets.store(SECRET_KEY, key);
      else await ctx.secrets.delete(SECRET_KEY);
      vscode.window.showInformationMessage(key ? 'CodeCanvas API key saved.' : 'CodeCanvas API key cleared.');
    }),
    vscode.window.registerWebviewViewProvider('codecanvas.launcher', new LauncherView(ctx, open)),
  );
}

/**
 * The tiny sidebar view behind the activity-bar button. Showing it (i.e. clicking the
 * CodeCanvas icon) pops the canvas window open; it also offers buttons to re-open it.
 */
class LauncherView implements vscode.WebviewViewProvider {
  constructor(private readonly ctx: vscode.ExtensionContext, private readonly open: () => Promise<CanvasPanel>) {}

  resolveWebviewView(view: vscode.WebviewView) {
    view.webview.options = { enableScripts: true };
    const nonce = Math.random().toString(36).slice(2);
    view.webview.html = `<!DOCTYPE html><html><head>
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'nonce-${nonce}';">
<style>
  body { padding: 12px; font-family: var(--vscode-font-family); color: var(--vscode-foreground); }
  button { display:block; width:100%; margin: 0 0 8px; padding: 7px; border: 0; border-radius: 3px; cursor: pointer;
    background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
  button.sec { background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); }
  button:hover { filter: brightness(1.1); }
  p { opacity: .7; font-size: 12px; line-height: 1.5; }
</style></head><body>
<button id="open">Open Canvas Window</button>
<button class="sec" id="all">Explain All</button>
<button class="sec" id="file">Explain Current File</button>
<button class="sec" id="settings">LLM Settings</button>
<p>Wheel scrolls vertically · Shift+wheel horizontally · Ctrl+wheel zooms. Double-click a card to edit.</p>
<script nonce="${nonce}">
  const vs = acquireVsCodeApi();
  for (const id of ['open','all','file','settings']) document.getElementById(id).onclick = () => vs.postMessage(id);
</script></body></html>`;
    view.webview.onDidReceiveMessage(async m => {
      if (m === 'open') await this.open();
      if (m === 'all') await vscode.commands.executeCommand('codecanvas.explainAll');
      if (m === 'file') await vscode.commands.executeCommand('codecanvas.explainFile');
      if (m === 'settings') await vscode.commands.executeCommand('workbench.action.openSettings', 'codecanvas');
    });
    // Clicking the activity-bar icon reveals this view → spawn / focus the canvas window.
    if (view.visible) void this.open();
    view.onDidChangeVisibility(() => { if (view.visible) void this.open(); });
  }
}

export function deactivate() {
  CanvasPanel.current?.dispose();
}
