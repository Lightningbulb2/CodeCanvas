# CodeCanvas (Rust / GPUI)

An infinite canvas for your code, built on GPUI, Zed's UI framework. A single native binary with no bundled browser, which is what makes it practical to package on Guix System. It's the desktop app of the CodeCanvas family, alongside [CodeCanvas-vscode](https://github.com/Lightningbulb2/CodeCanvas-vscode) and [CodeCanvas-zed](https://github.com/Lightningbulb2/CodeCanvas-zed).

```bash
cargo run --release -- [folder] [file]
```

Launching again while a project is open shows it in the running window; the launcher returns immediately, so Zed tasks don't wait on it. Use the [CodeCanvas-zed](https://github.com/Lightningbulb2/CodeCanvas-zed) extension to connect it to Zed.

## Controls

| Action | How |
|---|---|
| Scroll / sideways / zoom | Wheel / Shift+wheel / Ctrl+wheel |
| Pan | Middle-drag; Space+drag on empty canvas |
| Move any card | Drag the header; Space+drag anywhere on it |
| Multi-select | Ctrl/Shift+click, or drag a box on empty canvas |
| Edit | Double-click a file or snippet. Esc finishes, Ctrl+S saves |
| New snippet | Double-click empty canvas |
| Fit / undo / redo / save all | `F` / Ctrl+Z / Ctrl+Y / Ctrl+S |
| Split views | Top-bar buttons, a view's bar, or Ctrl+\ (Ctrl+Shift+\ splits down) |
| Hide the explorer / chat | The handles on the canvas's left and right edges |
| Cancel a drag | Esc |

## Snippets

Drag across lines in a file, then pick from the toolbar: **Snippet**, **Detach**, **Reference**, **Copy out**, **Ask**, **Context**.

- **Colour bar:** click selects and highlights; drag the middle to move; drag the top or bottom edge to grow or shrink. Snippets it runs into are dissolved on release, and their code stays.
- **▶ tab:** click opens the chat; drag moves the snippet.
- **Moving:** lines part in real time as you drag up and down a file, into another file, or across views. Drop on empty canvas to pull it out.
- **Alt+drag:** onto canvas makes a reference (a card that mirrors the lines, which stay grayed in the file); into a file inserts a copy.
- **Shift+drag selected lines** moves them inside their file without making a snippet.
- **Shift+drag a snippet's lines** grabs the snippet without selecting it first: move it inside its file, into another file, or out onto the canvas. A Shift+click that doesn't drag still multi-selects, on release.
- **Merge:** drag a card onto a file, or select cards and right-click a file → Merge.
- **Dissolve all:** from a file's or the canvas's right-click menu.

## Find and replace

**Ctrl+F** (or the magnifier in a card's header) opens a find bar at the top of that file or snippet; **Ctrl+H** opens it with Replace. It searches the card being edited, else the selected card, else the one under the pointer. With a snippet inside a file selected, it searches the file limited to that snippet's lines.

| | |
|---|---|
| Next / previous match | Enter / Shift+Enter (also F3 / Shift+F3) |
| Match case, whole word, regular expression | `Aa` `ab` `.*` (Alt+C, Alt+W, Alt+R) |
| Find in selection | `≡` (Alt+L), limits the search to the selected lines |
| Replace / replace all | Enter in the Replace box / Ctrl+Alt+Enter |
| Preserve case | `AB` (Alt+P) |
| Close | Esc |

In regular-expression mode the replacement can use `$1`, `$2`, … for groups. Matches are found within a line; a pattern can't span lines. Replacing in a reference card changes its source file, and every replace is one undo step.

## LLM

Every file and snippet has its own chat (▶ tab), and the panel on the right is a general chat. *Snippet/File* sends just that code; *Global* adds the context list (top right, with its history log). ↺ resets a chat.

Snippets also have a **✎ Draft**: suggested changes appear as a diff to edit, reset or Apply. **Explain File** and **Explain All** split files into explained snippets.

Providers: llama.cpp (the default, `http://localhost:8080/v1`), Ollama, OpenAI-compatible servers, Anthropic. The model can read files inside the project, and search the web if you set a SearXNG URL. Settings are behind the ⚙ button.

## Where things live on Linux

| | |
|---|---|
| Settings and API key | `$XDG_CONFIG_HOME/codecanvas` (the key file is mode 600) |
| Per-project canvas state | `$XDG_STATE_HOME/codecanvas/projects` |
| Single-instance port files | `$XDG_RUNTIME_DIR/codecanvas` |

## Packaging for Guix

The Guix package lives in the Lightningbulb channel (`codecanvas.scm`) and installs `packaging/codecanvas.desktop`. The crate has no git dependencies, and `Cargo.lock` keeps the crate list reproducible.

`patches/xattr` is a one-line fix: GPUI 0.2.2 depends on `xattr` 0.2.3, which no longer builds on Linux against current `libc`.
