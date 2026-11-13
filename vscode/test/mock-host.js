// Fake extension host for testing the webview in a plain browser.
(function () {
  // ?timers: animation frames from timers, for a browser that pauses them while its tab is not shown
  if (/[?&]timers\b/.test(location.search)) window.requestAnimationFrame = cb => setTimeout(() => cb(performance.now()), 16);
  const FILES = {
    'src/math.ts': `import { clamp } from './util';\n\nexport function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport function mul(a: number, b: number): number {\n  return a * b;\n}\n\n/** Linear interpolation */\nexport function lerp(a: number, b: number, t: number) {\n  return a + (b - a) * clamp(t, 0, 1);\n}\n`,
    'src/util.ts': `export const clamp = (v: number, lo: number, hi: number) =>\n  Math.max(lo, Math.min(hi, v));\n\nexport function range(n: number): number[] {\n  const out: number[] = [];\n  for (let i = 0; i < n; i++) out.push(i);\n  return out;\n}\n`,
    'README.md': '# Demo\n\nA tiny demo project.\n',
    'lua/hud.lua': "local Group = import('/lua/maui/group.lua').Group\n\n-- The HUD panel\nfunction Init(self, parent)\n    local w = Config.Hud.Width\n    local h = math.floor(w * 0.5)\n    if w > 100 then\n        self.panelWidth = w\n    end\n    return h\nend\n",
    'app/main.py': 'import sys\n\ndef main(argv):\n    """Entry point."""\n    for a in argv:\n        print(a)\n\nif __name__ == "__main__":\n    main(sys.argv)\n',
  };
  FILES['src/wide.ts'] = Array.from({ length: 40 }, (_, i) => i % 6 === 2
    ? `export const message${i} = format('this line is deliberately very long so that it runs past the right edge of its card: ${'x'.repeat(40)}', ${i});`
    : `export const v${i} = ${i};`).join('\n');
  FILES['src/long.ts'] = Array.from({ length: 200 }, (_, i) => `export const v${i} = ${i}; // line ${i + 1}`).join('\n');
  const post = m => setTimeout(() => window.postMessage(m, '*'), 5);
  const log = (window.__hostLog = []);
  window.acquireVsCodeApi = () => ({
    postMessage(m) {
      log.push(m);
      switch (m.type) {
        case 'ready': post({ type: 'init', files: Object.keys(FILES), state: JSON.parse(localStorage.getItem('cc-state') || 'null'), workspace: 'demo-project' });
          post({ type: 'config', provider: 'ollama', model: 'qwen2.5-coder:7b', endpoint: 'http://localhost:11434' }); break;
        case 'readFile': post({ type: 'fileContent', reqId: m.reqId, path: m.path, text: FILES[m.path] }); break;
        case 'saveFile': FILES[m.path] = m.text; post({ type: 'saved', path: m.path }); break;
        case 'confirm': post({ type: 'confirmResult', reqId: m.reqId, ok: window.confirm(m.message) }); break;
        case 'saveState': localStorage.setItem('cc-state', JSON.stringify(m.state)); break;
        case 'listAll': post({ type: 'allFiles', reqId: m.reqId, files: Object.keys(FILES), total: 4 }); break;
        case 'listModels': post({ type: 'models', models: ['qwen2.5-coder:7b', 'llama3.1:8b', 'deepseek-coder-v2'], current: 'qwen2.5-coder:7b' }); break;
        case 'chat': {
          const lastQ = m.history[m.history.length - 1].content;
          if (/^Change this file/.test(lastQ)) {
            // a file chat: changes in the file's own numbering, combined into the file's draft
            const path = m.context[0].path;
            const snippets = [{ title: 'one', file: path, replaceStart: 1, replaceEnd: 1, code: '// changed first line' }, { title: 'two', file: path, insertAt: 4, code: '// inserted before line 4' }];
            const r = snippets.map(d => '```snippet file="' + path + '" ' + (d.replaceStart ? `replace="${d.replaceStart}-${d.replaceEnd}"` : `insert="${d.insertAt}"`) + '\n' + d.code + '\n```').join('\n\n');
            setTimeout(() => post({ type: 'chatDone', id: m.id, text: r, snippets }), 200);
            break;
          }
          if (/^Revise/.test(lastQ)) {
            const orig = m.context[0].text.split('\n');
            const revised = ['// revised: ' + lastQ.slice(17), ...orig.map(l => l.replace('a + b', 'b + a'))].join('\n');
            const r = 'Here is the revision:\n\n```snippet title="draft"\n' + revised + '\n```\n';
            setTimeout(() => post({ type: 'chatDone', id: m.id, text: r, snippets: [{ title: 'draft', code: revised }] }), 200);
            break;
          }
          const ctx = m.context.map(c => `${c.label} (${c.text.split('\n').length} lines)`).join(', ') || 'none';
          const reply = `Context I received: **${ctx}**.\n\nHere is a draft:\n\n\`\`\`snippet file="src/math.ts" insert="10" title="sub()"\nexport function sub(a: number, b: number): number {\n  return a - b;\n}\n\n\`\`\`\n`;
          let i = 0;
          const iv = setInterval(() => {
            if (i >= reply.length) { clearInterval(iv); post({ type: 'chatDone', id: m.id, text: reply, snippets: [{ title: 'sub()', file: 'src/math.ts', insertAt: 10, code: 'export function sub(a: number, b: number): number {\n  return a - b;\n}\n' }] }); return; }
            post({ type: 'chatDelta', id: m.id, text: reply.slice(i, i + 12) }); i += 12;
          }, 15);
          break;
        }
        case 'explain': {
          const n = m.text.split('\n').length;
          const segs = []; for (let s = 1; s <= n; s += 4) segs.push({ start: s, end: Math.min(n, s + 3), title: `Part ${segs.length + 1}`, explanation: `Lines ${s}-${Math.min(n, s + 3)} of ${m.path} do something useful.` });
          setTimeout(() => post({ type: 'explainProgress', id: m.id, path: m.path, segments: segs.slice(0, 1) }), 150);
          setTimeout(() => post({ type: 'explainDone', id: m.id, path: m.path, segments: segs }), 400);
          break;
        }
      }
    },
  });
})();
