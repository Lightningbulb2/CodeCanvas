# CodeCanvas

An infinite, Obsidian-style canvas for your code inside VS Code. You lay files out in space, cut them into **snippets**, draft new snippets with an LLM (local models first), and merge them back in line by line with animations.

## Run it

```bash
npm install
npm run compile
```

Open this folder in VS Code and press **F5** ("Run CodeCanvas"). In the new Extension Development Host window, click the **CodeCanvas** icon in the activity bar. The canvas opens in its own desktop window. That needs VS Code 1.85+; set `codecanvas.openInNewWindow: false` to keep it as a tab instead.

To install it permanently, run `npm run package` and install the generated `.vsix` (Extensions → … → Install from VSIX).

## Canvas controls

| Action | How |
|---|---|
| Scroll vertically / horizontally | Wheel / Shift + wheel |
| Zoom (smooth, toward the cursor) | Ctrl + wheel. Below ~30% the cards collapse into big labels (strategic view) |
| Pan | Middle-drag, or Space+drag on empty canvas |
| Open a file | Click it in the left explorer, or drag it onto the canvas |
| Move cards | Drag the header. Ctrl/Shift-click to multi-select, or drag a box on empty space |
| Fit everything / selection | `F` |
| Undo / redo | Ctrl+Z / Ctrl+Y |

## Snippets

- **Make one from a file:** drag across lines in a file, then pick from the toolbar:
  - ◆ **Snippet** keeps the lines in the file as a coloured region.
  - ✂ **Detach** cuts them out into a floating card.
  - ↗ **Reference** pops out a live reference card.
  - ⧉ **Copy out** copies them into an independent floating card.
  - **? Ask** makes a snippet and opens its chat.
  - ◎ **Context** adds the lines to global context.
- **Drag snippets around:** grab a region's coloured bar or its ▶ tab. Drag it up or down the file and the other lines slide aside in real time. Drag it into another file and that file opens a gap under it. Drag it out onto the canvas and the file closes up behind it. Snippets carry no fixed line numbers; they belong wherever you drop them.
- **References:** hold **Alt** while dragging a region out (or use ↗ Reference / *Pop out reference*). You get a dashed card that mirrors the lines and shows the file and lines it came from. The original lines stay put, grayed out. Editing the card edits the source. Removing it un-grays the source. Dropping it into a file inserts a copy.
- **Merge:** drag a floating snippet onto a file. The lines part to make room as you move it, and it slots in on release. You can also Ctrl/Shift-select snippets and a file, then right-click → **Merge into …**.
- **Edit:** double-click a file or snippet. Double-clicking a snippet that sits in a file edits the whole file. Esc finishes, Ctrl+S saves to disk. Regions stay attached to their lines through edits.
- **New blank snippet:** double-click empty canvas.

## Split views

Use the split buttons in the top bar, a view's hover bar (top-left), right-click → *Split right / Split down*, or **Ctrl+\** (**Ctrl+Shift+\** splits down). The **Shared canvas** checkbox in the top bar (on by default) sets whether views show the same canvas or each have their own. Each view has its own pan and zoom. Drag the sash between views to resize them. Drag files, floating snippets, and snippets inside files from one view into another, including straight into a file in the other view, where the lines part to make room. Closing a view moves its cards into the neighbouring one.

## Drafts and file chats

- **Drafts:** every snippet has a **✎ Draft** tab under its chat tab. The draft starts as a copy of the snippet, and any changes show as a git-style diff. Edit it directly, or type a request in the box at the bottom. When the snippet's chat suggests a change, it lands in the draft, and **Apply** writes it into the snippet (and its file).
- **File chats:** files have their own **Chat** tab in the top-right corner (or the chat icon in the header), with *File* / *Global* context.
- **Resetting a chat:** every chat has a ↺ button that resets it.

## Moving things

- **Esc:** cancels any drag in progress and puts everything back.
- **A snippet's colour bar** (in a file): click it to select and highlight the snippet, or drag it to move the snippet (Alt+drag makes a reference). Drag its top or bottom edge (↕ cursor) to grow or shrink it a line at a time. Snippets it runs into are dissolved on release, and their code stays.
- **Dissolving snippets:** right-click a file → *Dissolve all snippets in this file*, or right-click empty canvas → *Dissolve all snippets (every file)*.
- **Shift + drag** on a line selection moves those lines up or down inside their file. They don't become a snippet, and the wheel scrolls vertically while you drag.

- **Space + drag:** moves any file or snippet, wherever you grab it. On empty canvas it pans.
- **Selected snippets:** once a snippet in a file is selected (including one you just dropped in), you can drag it from anywhere on its lines.

## LLM

Every snippet has a chat drawer. Open it with the ▶ tab on its right edge.

- **Snippet** mode sends only that snippet.
- **Global** mode also sends everything in the context list.
- Either way, questions and answers stay in that snippet's thread.

The **context history** panel (top right) logs every add and remove with its line count. The **Chat** panel on the right is an unfocused, project-wide chat.

The model has the whole project. It sees the file tree and can pull in any file mid-answer. Code it proposes shows up as snippet cards on the canvas, with a dashed line to the place in the file where it belongs. Each card has a ⤵ button that applies it there.

- **Explain File** (the ? icon on a file, or right-click) splits the file into explained snippets. The 3-lines icon opens all the notes side by side.
- **Explain All** (top bar) opens every file, lays them out, and explains them one by one.

### Providers (`codecanvas.llm.*` settings or the model button in the top bar)

| provider | default endpoint | notes |
|---|---|---|
| `ollama` (default) | `http://localhost:11434` | e.g. `ollama pull qwen2.5-coder:7b` |
| `llamacpp` | `http://localhost:8080/v1` | llama.cpp's `llama-server` |
| `openai` | `http://localhost:1234/v1` | Any OpenAI-compatible server: LM Studio, vLLM, Jan, OpenRouter, OpenAI |
| `anthropic` | `https://api.anthropic.com` | Needs **CodeCanvas: Set LLM API Key** (stored in SecretStorage) |

If you set an endpoint ending in `/v1` but no provider, CodeCanvas assumes an OpenAI-style server.

### llama.cpp

```
llama-server -m model.gguf --port 8080 -c 16384 -ngl 99 --jinja
```

Then set `codecanvas.llm.provider` to `llamacpp`, or just set the endpoint to `http://localhost:8080/v1`.

- **Context:** use at least 16k (`-c`). CodeCanvas sends the project's file list plus your context.
- **Reasoning models:** CodeCanvas shows "thinking…" while a reasoning model reasons.

### Tools the model can use

All tools are plain-text tags, so they work with any model; native tool-calling support isn't needed. Each one is limited by `codecanvas.llm.maxFileReads` (calls per answer, default 8).

- **Read project files:** `<read path="…"/>`. Always on.
- **Search the web:** `<search query="…"/>`. Needs a SearXNG instance; see below.
- **Read a web page:** `<fetch url="…"/>`. The page arrives as cleaned-up text. Needs SearXNG configured.

Each tool use shows up in the chat as a line like *🔎 searched the web for "…"*.

### Web search with SearXNG

1. **Run SearXNG,** for example with Docker:
   ```
   docker run -d --name searxng -p 8888:8080 -v ./searxng:/etc/searxng searxng/searxng
   ```
2. **Enable JSON output.** In the generated `searxng/settings.yml`, add `json` under `search: formats:`, then restart the container:
   ```yaml
   search:
     formats:
       - html
       - json
   ```
3. **Point CodeCanvas at it.** Use the model button → *Web search*, or run **CodeCanvas: Set SearXNG URL**, and enter `http://localhost:8888`. The setting is `codecanvas.web.searxngUrl`.

## Development

- `src/`: extension host. It handles the window, file access, LLM streaming, the read-file loop and snippet parsing.
- `media/webview/`: the canvas UI, plain ES modules with no build step.
- `test/harness.html`: runs the canvas in a normal browser against a mock host. To use it, run `python -m http.server 5178` and open `http://localhost:5178/test/harness.html`.
