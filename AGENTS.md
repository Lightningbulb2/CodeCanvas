# AGENTS.md

Notes for coding agents (and people) working on CodeCanvas. The READMEs in `rust/` and `vscode/` describe
the app for users; this file covers what you need to change it without breaking it.

## What is here

| Directory | What | Language |
|---|---|---|
| `rust/` | The desktop app: one native binary on GPUI 0.2.2 (Zed's UI framework) | Rust 2024 |
| `vscode/` | The extension for VS Code / VSCodium: an extension host plus a webview canvas | TypeScript + plain ES modules |

They are two implementations of the same product: an infinite canvas of code files, lines cut into movable
snippets, and a chat with a language model per file and per snippet. Each directory is its own project
with its own build, README and license.

## Ground rules

1. **Keep the two in step.** A feature or behaviour change in one is ported to the other in the same piece
   of work, unless the platform makes it unnecessary (say so in the commit). Controls, menus, the LLM
   protocol and saved-state meaning should match.
2. **Cheap to run comes first.** CodeCanvas is meant to run next to local models that take most of the CPU,
   GPU and memory. Measure before and after anything that touches drawing, scrolling or zooming (see
   *Measuring*), and don't trade frame time or memory for convenience.
3. **Text never shimmers, jumps or blurs.** Zooming and panning must keep every line still relative to its
   card and as sharp as the browser draws it. No level-of-detail text (blocks or squiggles instead of code)
   in the Rust app: when zoomed far out, cards show only their name and line count.
4. **Don't edit files on disk except through Save.** Canvas edits stay on the canvas (cards show a dirty
   dot) until the user saves.
5. **Small, focused commits** with a summary line and a short bulleted body saying what changed and why.
   Bump `vscode/package.json`'s version when the extension changes.

## Building and testing

### rust/

```bash
cargo build --release
cargo test --release          # model, text buffer, diff, highlighter, LLM parsing, file drafts
cargo run --release -- [folder] [file]
```

On Guix System, build inside the package's dev shell (the channel is
[hyperguix](https://github.com/Lightningbulb2/hyperguix); pass `-L` to a local checkout of it):

```bash
guix shell -L <hyperguix> -D codecanvas gcc-toolchain -- env CC=gcc cargo build --release
```

- `patches/xattr` is a one-line fix: GPUI 0.2.2 needs `xattr` 0.2.3, which no longer builds against current
  `libc`. Keep the `[patch.crates-io]` entry.
- `Cargo.lock` is committed and the crate has no git dependencies; the Guix package depends on both.
- Inside a Guix dev shell, Vulkan may not find the NVIDIA driver; run the binary with the installed
  package's library path (the wrapper's `LD_LIBRARY_PATH`) when testing on such a GPU.

### vscode/

```bash
npm install
npm run compile    # tsc: src/*.ts -> out/
npm run package    # .vsix
```

Press **F5** in VS Code ("Run CodeCanvas") for an Extension Development Host. Only `src/` (the extension
host) is compiled; `media/webview/*.js` are ES modules loaded as they are.

**Browser harness:** `test/harness.html` runs the whole webview in a plain browser against a fake host
(`test/mock-host.js`: a few demo files, canned chat replies, state in `localStorage`). Serve the `vscode/`
directory over HTTP and open `/test/harness.html`:

```bash
python3 -m http.server 8765 --directory vscode
```

- Add `?timers` when the browser pauses `requestAnimationFrame` (background or hidden tabs, automation);
  without it the view never glides.
- `import('/media/webview/canvas.js')` from the console gives you the module (`openFile`, `createFloating`,
  `toggleDraft`, `detachRegion`, …) to drive a test.
- The mock host answers a chat message starting with `Revise` with a snippet revision, one starting with
  `Change this file` with two line changes for that file, and anything else with a new snippet.

## The model (both versions)

- **Card:** a file (has a path, its lines, and what is on disk) or a floating snippet (its own lines).
- **Region ("snippet in a file"):** a line range `start..=end` marked inside a host card. The host is
  usually a file, but **a floating snippet can host regions too.** Regions follow their lines through every
  edit: all changes to a card's lines go through a splice that shifts, grows, shrinks or drops regions
  (`Doc::splice` / `set_file_lines` in Rust, `spliceFile` / `setFileLines` in JS). Never assign a card's
  lines directly if it may hold regions.
- **Reference:** a floating card that mirrors a region's lines (`ref_of` / `ref`); edits go to the source.
- **Merging** a floating snippet into a file turns it into a region of that file; regions inside it move
  along with the same offsets. Removing a card removes the regions it hosts.
- **Talk:** every file, snippet and region has a chat and a **draft**. The draft is an editable copy shown
  as a diff against the current lines; *Apply* writes it back through the splice. Files get drafts too.
- **Context list:** cards and regions the user adds; *Global* chat mode sends them with the question.

### LLM reply protocol

The model proposes code as fenced blocks, parsed on both sides:

````
```snippet file="relative/path" replace="A-B" title="Short title"
...code...
```
````

`insert="N"` inserts before line N; `replace="A-B"` replaces lines A..B (inclusive, 1-based). Routing:

- In a **snippet's** chat, the block revising that snippet becomes its draft; others become cards.
- In a **file's** chat, every block with `file=` that file is applied to the file's draft, in the file's
  numbering before any change (applied bottom-up; a block with neither attribute is the whole new file).
  See `file_draft` (Rust, tested) and `fileDraft` (JS).
- Anything else becomes a floating card near the chat, with a dashed link to its suggested place.

Tools the model can call in the plain-text tool loop: `<read path="…"/>` (project files only) and
`<search query="…"/>` (needs a SearXNG URL). See `rust/src/agent.rs` and `vscode/src/agent.ts`.

## rust/ architecture

| File | What |
|---|---|
| `main.rs` | Single instance (a second launch hands its folder to the running window), detached relaunch with `--cc-child`, `mallopt` tuning, window options (`app_id` `codecanvas`) |
| `model.rs` | `Doc`, `Card`, `Region`, `Talk`, drafts, context, splice, saving and loading (tests here) |
| `agent.rs`, `llm.rs`, `web.rs` | Prompts and the snippet protocol, providers (llama.cpp, Ollama, OpenAI-compatible, Anthropic), SearXNG |
| `text.rs`, `highlight.rs`, `diff.rs` | Editor buffer, compact syntax spans, line diff |
| `ui/mod.rs` | `CanvasView` state, input, drags (`enum Drag`), live drag-and-drop, chats, file drafts |
| `ui/render.rs` | Drawing: cards, text lines, cells, overflow pillars, tabs, dot grid, cached views |
| `ui/tiles.rs` | Text cell cache: worker rasterisers and the cell format |
| `ui/panels.rs`, `ui/find.rs`, `ui/fx.rs` | Top bar, explorer, chat panel, menus; find/replace; animations and icons |

State lives in `$XDG_STATE_HOME/codecanvas/projects/<hash>.json` (file text that matches disk is left out
and re-read on load), settings and the API key in `$XDG_CONFIG_HOME/codecanvas`.

### How text is drawn, and the rules that keep it fast and steady

GPUI rebuilds the element tree every frame, so per-frame work is what matters. The pieces below were each
measured; keep them unless you measure something better.

- **Text cells.** At rest and while panning, code is drawn from cached bitmaps: 256×256 device-pixel cells
  per (view, card, zoom, column, row), rasterised by worker threads with cosmic-text/swash using only the code
  font's two faces. All cells are the same size because GPUI's upload staging buffers are reused only for
  uploads of equal or smaller size.
- **Cells must match directly drawn glyphs pixel for pixel**, so switching between them is invisible: same
  quarter-pixel glyph size (`code_px`), quarter-pixel horizontal offsets, whole-pixel baselines computed the
  same way, the platform descent taken as a positive distance, and GPUI's glyph gamma (1.8) and contrast
  correction applied to coverage (`glyph_alpha`). If you change one path, change the other.
- **While a zoom glides**, lines are drawn as real glyphs at each frame's size (stretched bitmaps look dim
  and soft). Cells for the zoom being landed on are requested once the target has held for a few frames, so
  they are ready when the glide stops.
- **Zoom levels** from the wheel, keys and Fit are snapped so a line is a whole number of pixels tall; card
  origins are rounded to whole pixels.
- **One draw order per card's text** (`window.paint_layer`): GPUI's scene otherwise searches its bounds tree
  for every glyph, which is very slow where cards overlap.
- **Occlusion:** lines fully covered by opaque cards drawn above are neither drawn nor cached.
- **Cell budget:** about four screens; least recently used cells go first, never one drawn in the last two
  frames (dropping visible cells makes text flicker between cells and glyphs). Finished cells nobody wants
  any more are dropped before upload; uploads are capped per frame.
- **Cached views:** the chat panel, top bar, explorer and context list are `AnyView::cached` entities that
  redraw only when a fingerprint of what they show changes (`chat_key`, `chrome_key`). If something a panel
  shows can change, include it in the fingerprint, or the panel goes stale. A cached view notified during a
  draw also needs another frame (`fx.want_frame = true`).
- **Dot grid:** one quad per dot at its exact canvas position (snapped per dot), in one layer. Rounding the
  spacing instead makes the dots slide against the cards while zooming.
- **Memory:** lines are shared `Arc`s, chat text is `Arc<str>`, spans are compact, the UI font is one that
  doesn't make GPUI load dozens of faces, and glibc's arenas and trim threshold are tuned in `main.rs`.
- The pump runs at 33 ms only while something is moving or streaming, otherwise 250 ms.

## vscode/ architecture

| File | What |
|---|---|
| `src/extension.ts`, `src/canvasPanel.ts` | Commands, the webview panel or window, file access, state (`workspaceState` key `codecanvas.state.v1`) |
| `src/agent.ts`, `src/llm.ts`, `src/web.ts` | Prompts and tool loop, providers, SearXNG (same behaviour as the Rust files) |
| `media/webview/canvas.js` | The canvas: model (`S.files`, `S.snippets`), rendering, drags, regions, drafts, find, persistence |
| `media/webview/main.js`, `chat.js`, `util.js`, `bridge.js` | Top bar and panels, chat UI, helpers and highlighter, host messaging |

- The view is a CSS transform on the world element; the browser does the zooming, so text stays sharp.
- Regions are entries in `S.snippets` with `fileId` naming their host; look hosts up with `hostOf(id)` (a
  file or a floating snippet), name their language with `hostPath(card)`, and redraw with `renderHost(card)`.
- Off-screen lines use `content-visibility: auto`; far-zoom labels get `--inv-z` on the labels only (on the
  world it restyles every line each frame).
- Wide lines: `.ln.wide` draws the fade; pillars are absolutely placed in the card, positioned from inside
  its border. While editing, the textarea is as wide as its widest line so it never scrolls sideways.
- Saved snippets are plain copies of the objects minus keys starting with `_` or ending in `El` (DOM).

## Measuring

Judge drawing changes by frame time and main-thread CPU on a real GPU, not by feel:

- Run the app in a headless compositor on the GPU (for example sway with `WLR_BACKENDS=headless
  WLR_RENDERER=vulkan`) with a temporary hook that moves the view each frame (pan, scroll, wheel-style zoom
  steps, large zoom jumps) and prints frame intervals, the main thread's CPU per frame and RSS. Use a real
  canvas: several overlapping files of a few hundred lines each. Keep the hook out of commits.
- Profile with `perf record -e cpu-clock` and read the main thread (`--tid`). A software GPU (llvmpipe)
  dominates CPU profiles; count only the app's own code there.
- Compare screenshots (ImageMagick `compare`) to prove that two drawing paths give the same pixels.

Reference points on a 1920×1080 canvas with nine overlapping files: about 16.2 ms per frame (60 Hz) for
pan, scroll and zoom with no frames over 20 ms; main thread about 2–3 ms per frame when panning, 5–11 ms while
a zoom glides; RSS about 300 MB.

## Packaging

Both are packaged for Guix in hyperguix as `codecanvas` and `codecanvas-vscode` (plus
`codecanvas-zed-bridge` from [CodeCanvas-zed](https://github.com/Lightningbulb2/CodeCanvas-zed)). The
packages pin commits; update the pins when releasing. The Rust package installs
`rust/packaging/codecanvas.desktop` and `codecanvas.svg`; the window's `app_id` must stay `codecanvas` to
match the desktop entry.

## Gotchas

- The Zed extension is a separate repo; on Guix, Zed can't compile Rust extensions (no rustup or
  `wasm32-wasip2`), so Zed is pointed at the bridge binary as a context server instead.
- GPUI loads every face of a font family; a family with dozens of faces costs tens of MB.
- `cx.notify()` on an entity during a draw takes effect on the next frame only.
- Line endings differ by file: `vscode/src/*.ts`, `media/webview/main.js` and `chat.js`, `test/mock-host.js`
  and the extension's ignore files use CRLF, the rest LF. Keep each file's own endings (some editing tools
  rewrite them; check `git diff --stat` for whole-file changes).
