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

## How it is built

Read [ARCHITECTURE.md](ARCHITECTURE.md) before changing anything non-trivial: the shared model, the LLM
protocol, each app's threads, frame and drawing pipeline, and a table of where each feature lives in both
versions (use it to port a change).

### Don't break these

Each of these was measured or fixed a visible bug; keep them unless you measure something better.

- **Every change to a card's lines goes through the splice** (`Doc::splice` / `set_file_lines`,
  `spliceFile` / `setFileLines`), so regions stay on their lines. Never assign a card's lines directly.
- **Rust: cells and directly drawn glyphs must match pixel for pixel**: same `code_px` size, quarter-pixel
  offsets, baseline formula and glyph gamma/contrast (`glyph_alpha`). Change one path, change the other,
  and compare screenshots.
- **Rust: zoom levels stay whole-pixel line heights** and card origins whole pixels.
- **Rust: one `paint_layer` per card's text**, occlusion of covered lines, and cell eviction that never
  drops a cell drawn in the last two frames.
- **Rust: cached panels redraw only when their fingerprint changes** (`chat_key`, `chrome_key`). Anything a
  panel shows must be in its fingerprint; a cached view notified mid-draw also needs `fx.want_frame = true`.
- **Rust: all text cells are the same size** (GPUI's upload buffers are reused only for same-or-smaller).
- **The dot grid is placed per dot at exact canvas positions**; rounding the spacing makes it slide.
- **VS Code: look up a region's host with `hostOf`**, not `S.files`; hosts can be floating snippets.
- **VS Code: `--inv-z` goes on far-zoom labels only**, never on the world (it would restyle every line).
- **Both: the state format** is read by older and newer versions alike; add fields with defaults.

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
