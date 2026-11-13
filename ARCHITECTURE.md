# Architecture

How CodeCanvas is put together. [AGENTS.md](AGENTS.md) has the rules for changing it, the build and test
commands, and how to measure.

CodeCanvas exists twice: `rust/` is a native desktop app on GPUI (Zed's UI framework) and `vscode/` is a VS
Code extension whose canvas is a webview. They share one model and one LLM protocol, and are kept
feature for feature in step; only the drawing and the platform glue differ.

## The shared model

```
Doc
├── cards        files and floating snippets, in drawing order (later = on top)
├── regions      line ranges marked inside a card ("snippets in a file")
├── ctx          the context list (card and region ids)
├── history      log of context changes
└── global talk  the general chat
```

- **Card.** A *file* has a path, its current lines and the lines on disk (dirty when they differ). A
  *floating snippet* has only its own lines, plus optional `target` (where the LLM suggested it goes) and
  `ref_of` (a *reference*: it mirrors a region's lines, and editing it edits the source). Every card has a
  position and width in world units, a pane, a colour and a `Talk`.
- **Region.** `start..=end` in a host card. Usually the host is a file; a floating snippet can host regions
  too. A region has a title, colour and its own `Talk`. Regions may nest; the smallest one wins a click.
- **Talk.** A chat (messages, *Snippet/File* or *Global* mode, open or closed) and a *draft*: an editable
  copy of the owner's lines shown as a diff against them. Files, floating snippets and regions all have one.
- **Splice.** Every change to a card's lines goes through one splice (Rust `Doc::splice`, JS `spliceFile`),
  which shifts regions after the change, grows or shrinks the ones it falls inside, and drops the ones it
  deletes entirely. `set_file_lines` / `setFileLines` turn a whole new text into the smallest splice (common
  prefix and suffix kept), so regions survive edits, applied drafts and reloads from disk.
- **Moves.** Detaching a region turns it into a floating snippet; merging a floating snippet into a file
  turns it into a region there. Regions inside a moved snippet ride along with the same offsets; removing a
  card removes the regions it hosts. References are kept in sync after every change (`sync_refs`).

### Views

The canvas can be split into views (panes) in a tree (`Layout::Split` with sizes). Each pane has a view
`{x, y, z}` (screen = world · z + offset), split into the view now (`v`) and the view it is gliding to
(`t`), plus the zoom anchor. Each frame moves `v` toward `t`: zoom geometrically around the anchor, pan by a
fixed fraction. Wheel, keys and Fit only change `t`. In *shared canvas* mode every pane shows all cards;
otherwise each card belongs to one pane.

### The LLM

- **Context.** A chat sends its owner's text (the file, the snippet, or the region with its place), plus
  the context list in *Global* mode, plus the project's file list. A "focus" line tells the model what it
  is looking at and how to answer.
- **Tool loop.** Plain-text tags in the reply: `<read path="…"/>` (files inside the project only) and
  `<search query="…"/>` (SearXNG, if set). The loop runs a tool, appends its result, and asks again, up to
  a limit. Finished tool tags are hidden from the streamed text.
- **Code blocks.** Proposed code comes as ` ```snippet ` fences with optional `file`, `insert="N"` (before
  line N), `replace="A-B"` (inclusive, 1-based) and `title`. Routing by where the chat is:
  - a snippet's chat: the block revising that snippet becomes its draft;
  - a file's chat: every block for that file is applied to the file's draft in the file's own numbering
    (bottom-up; a block with no place is the whole file) — Rust `file_draft`, JS `fileDraft`;
  - anything else becomes a floating card near the chat, linked by a dashed line to its suggested place.
- **Explain File** asks for a list of segments (lines, title, explanation) and turns them into explained
  regions with the explanation as a note in their chat.
- **Providers.** llama.cpp (default), Ollama, OpenAI-compatible servers and Anthropic, streamed.

## rust/

### Process

- `main.rs` parses `[folder] [file]`. Unless it was started with `--cc-child`, it relaunches itself detached
  and exits at once (Zed tasks wait for their program).
- **One window per project.** The child writes a TCP port to `$XDG_RUNTIME_DIR/codecanvas/<project>.port`
  and listens on localhost. A later launch for the same project connects, sends the file to show, and exits;
  the running window opens it. The Zed bridge uses the same channel.
- glibc's allocator is tuned before anything else (`mallopt`: fewer arenas, earlier trim) to keep RSS low.
- The window's `app_id` is `codecanvas`, matching the desktop entry.

### Threads

| Thread | Does | Talks back through |
|---|---|---|
| UI (GPUI main) | Everything in `CanvasView`: input, layout, drawing | — |
| Pump (GPUI timer) | Every 33 ms while something is running, else 250 ms: drains the event channel and the single-instance inbox, polls files on disk every 800 ms, saves state 700 ms after the last change | — |
| Chat / Explain / model list | One short-lived thread per request: the LLM call and tool loop | `Ev` channel (`Delta`, `Status`, `Done`, `Error`, `Progress`, `ExplainDone`, `Models`) |
| Saver | Serialises the document when it changed and writes the state file; panning only rewrites the small view part | `Ev::SaveFailed` |
| Rasterisers | Draw text cells (below), a few threads | finished-cell channel |

### Modules

| File | What |
|---|---|
| `main.rs` | Launch, single instance, allocator tuning, window |
| `model.rs` | `Doc`, `Card`, `Region`, `Talk`, splice, drafts, context, project files, saving and loading |
| `agent.rs` | Prompts, the tool loop, the snippet protocol parser, Explain |
| `llm.rs`, `web.rs` | Streaming providers; SearXNG search |
| `text.rs` | `TextBuffer`: caret, selection and undo for the editor and every input box |
| `highlight.rs` | Line-based syntax spans in a compact form (`Spans`) |
| `diff.rs` | Line diff for drafts |
| `settings.rs` | Settings and API key in `$XDG_CONFIG_HOME/codecanvas` |
| `ui/mod.rs` | `CanvasView`: state, keyboard and mouse handling, drags, live drop preview, chats, drafts, persistence |
| `ui/render.rs` | Drawing the canvas: cards, lines, text cells, regions, overflow pillars, tabs and drawers, links, dot grid; the frame's order |
| `ui/tiles.rs` | Text cells: the rasteriser pool, cell format, glyph correction |
| `ui/panels.rs` | Top bar, explorer, chat panels, draft panel, menus, settings, toasts |
| `ui/find.rs` | Find and replace bars |
| `ui/fx.rs` | Tweens, flying lines, line effects, icons |

### A frame

`CanvasView::render` (in `ui/render.rs`), every frame something changes:

1. Advance animations; take finished text cells and upload a limited number; retire unused ones.
2. Step each pane's view toward its target (`step`).
3. For each pane (`pane_el` → `render_cards`), in card order:
   - work out which screen rectangles each opaque card hides (for occlusion);
   - per card: the card box, its text (`lines_el`: from cells or as glyphs), region bars, header, edge
     grips, overflow marks and pillars, find bar; then its tabs and drawers (drawn above every card);
   - the dragged block's ghost, selection toolbar, suggestion links and the dot grid.
4. The top bar, explorer, context list and chat panel as cached views, redrawn only when their
   fingerprint changed (`chrome_key`, `chat_key`).
5. Ask for another frame only if something is still moving.

GPUI rebuilds the element tree each frame, so per-frame work is what costs.

### Input and drags

Mouse-downs land on the most specific element (a pillar, an edge grip, a region bar or edge, a tab, a card
body, a header, the pane) and start a `Drag`: `Pan`, `Marquee`, `Move` (cards), `Select` and `EditSelect`
(lines), `Resize` (either edge or the corner), `Region` (a snippet by its bar, tab or lines), `Edge`
(grow or shrink a region), `ShiftLines`, `Sash`, `Explorer`. Moves update the model directly; `on_up`
finishes the drag. Space held turns any press into a move (or a pan on empty canvas).

**Live drop preview.** While a block of lines is dragged (a region, Shift-dragged lines, or a floating
snippet over a file), `Live` records the block, the card it is over and the line it would land at. The
target card's lines part around a gap, the block glides into the slot, and on release the model change is
made once (move, insert a copy with Alt, detach, or make a reference).

### How code is drawn

Text is drawn two ways that must look identical:

- **Cells** (`ui/tiles.rs`): at rest and while panning, code comes from cached 256×256 device-pixel bitmaps
  keyed by (pane, card, zoom, column, row). Worker threads draw them with cosmic-text/swash, loading only
  the code font's two faces. All cells are one size because GPUI reuses an upload staging buffer only for
  uploads of the same size or smaller.
- **Glyphs** (`paint_mono`): while a zoom glides, while a card is edited or animated, and for lines no cell
  covers yet, lines are drawn glyph by glyph at the frame's exact size.

To make switching invisible, both use the same quarter-pixel glyph size (`code_px`), quarter-pixel
horizontal offsets, the same whole-pixel baseline formula (with the font's descent as a positive distance),
and the cells apply GPUI's glyph gamma (1.8) and contrast correction to coverage (`glyph_alpha`).

Around that:

- **Zoom** from the wheel, keys and Fit is snapped so a line is a whole number of pixels tall, and card
  origins are whole pixels, so cells map 1:1 at rest.
- **Glide:** cells for the zoom being landed on are requested once the target has held for a few frames;
  they are ready when the glide stops.
- **One draw order per card's text** (`window.paint_layer`): GPUI's scene otherwise searches its bounds tree
  for every glyph, which is very slow where cards overlap.
- **Occlusion:** lines fully covered by opaque cards drawn above are neither drawn nor cached.
- **Cell budget:** about four screens, least recently used first, never a cell drawn in the last two frames;
  finished cells nobody wants any more are dropped before upload; uploads are capped per frame.
- **Far zoom:** below a threshold, cards show only their name and line count; there is no simplified text.
- **Dot grid:** one quad per dot at its exact canvas position, snapped per dot, in one layer, so the dots
  stay fixed to the canvas while zooming.
- **Lines wider than the card** end in a fade and a bright edge; their hidden part is drawn as a pillar
  beside the card (on hover, for all of them while editing or with the card's toggle on).

### Memory

Card lines are shared `Arc<Vec<String>>` (cloning a card is cheap; the saver and drawing share them), chat
text is `Arc<str>`, highlight spans are compact and dropped after 30 s unused, the interface font is one
whose family has few faces (GPUI loads every face), and the state file leaves out file text that matches
disk.

### State on disk

| | |
|---|---|
| Canvas per project | `$XDG_STATE_HOME/codecanvas/projects/<hash>.json`: the document (minus on-disk text), views, layout |
| Settings, API key | `$XDG_CONFIG_HOME/codecanvas` (key file mode 600) |
| Single-instance ports | `$XDG_RUNTIME_DIR/codecanvas/<hash>.port` |

## vscode/

### Extension host and webview

```
VS Code ── extension host (src/*.ts, compiled to out/) ──postMessage── webview (media/webview/*.js)
            files, settings, LLM calls, state                       canvas, chats, all interaction
```

- `extension.ts` registers the commands (`codecanvas.open`, `openFile`, `explainFile`, `explainAll`,
  `setApiKey`, `setSearxng`) and the activity-bar launcher; `canvasPanel.ts` owns the webview, in its own
  window by default (`codecanvas.openInNewWindow`).
- The host does everything that needs Node or VS Code: reading and saving files (`readFile` → `fileContent`,
  `saveFile` → `saved`), watching them (`fileChanged`), listing the project (`files`, `listAll`), dialogs
  (`confirm`), the LLM (`chat` → `chatDelta`/`chatStatus`/`chatDone`/`chatError`; `explain` →
  `explainProgress`/`explainDone`), models and settings (`listModels`, `setModel`, `setProvider`,
  `setApiKey`, `setSearxng` → `config`, `models`), and state (`saveState`, stored in `workspaceState` under
  `codecanvas.state.v1`, and sent back in `init`).
- `bridge.js` wraps messaging (`send`, and `request` for calls that expect a reply).

### Webview modules

| File | What |
|---|---|
| `canvas.js` | The canvas: model (`S.files`, `S.snippets`), panes and views, rendering cards, regions, tabs and drawers, drags and live drop preview, drafts, find, undo and persistence |
| `main.js` | App shell: top bar, file explorer, global chat, context list and history, Explain All, host messages |
| `chat.js` | One chat UI (used by every drawer and the global panel), streaming, code blocks with their buttons |
| `util.js` | DOM helper `h`, icons, the syntax highlighter, small helpers |

### Model in the webview

`S.files` (id → file card) and `S.snippets` (id → snippet). A snippet with `fileId` is a region of that host
(`start`, `end`); without it, it is a floating card (`x`, `y`, `w`, `lines`, optional `ref` and `target`).
`hostOf(id)` finds a host (a file or a floating snippet), `hostPath(card)` names its language, and
`renderHost(card)` redraws it.

### Drawing

Cards are DOM elements in a *world* element per pane. The pane's view is a CSS transform
(`translate(x, y) scale(z)`) on the world, so the browser zooms and keeps text sharp. A `requestAnimationFrame`
loop steps each pane's view toward its target and applies it. Lines are `.ln` rows with `.no` and `.code`;
off-screen rows use `content-visibility: auto`. Regions are absolutely placed overlays with a bar; tabs
and drawers live in each card's margin. While editing, a transparent textarea sits over a highlighted copy,
as wide as the widest line so it never scrolls sideways; pillars show wide lines' hidden parts.

### Drags

A press is routed in `onPointerDown`; `capture` sends the rest of the pointer stream to the drag's move
and up handlers and returns an abort for Esc. The live drop preview mirrors the Rust one: lines in the
target card part around a gap (`prepLive`, `layoutLive`, `liveHover`) and the ghost slides into the slot on
release (`slideInto`), then the model changes once.

### State and undo

`scheduleSave` sends the serialised state to the host 700 ms after the last change: files (path, place,
chat, draft, toggle, and their lines only if dirty) and snippets (plain copies minus keys starting with `_` or
ending in `El`). Undo keeps up to 60 JSON snapshots of files and snippets.

## Where things live

| To change | rust/ | vscode/ |
|---|---|---|
| Card header buttons | `header_el` in `ui/render.rs` | `renderFile` / `renderSnippetCard` in `canvas.js` |
| What a click or drag on a card does | `file_body_down`, `start_move`, `enum Drag`, `on_up` in `ui/mod.rs` | `onPointerDown` and the `start…` functions in `canvas.js` |
| Selection toolbar, right-click menus | `selection_tools` (render), `card_menu`, `region_menu` (panels) | `showSelTools`, the menu builder in `canvas.js` |
| Tabs and drawers beside a card | `overlays`, `chat_stack` in `ui/render.rs` | `renderRegions`, `buildTab`, `buildDraftTab` |
| Drafts | `draft_panel` (panels), `apply_draft`, `route_drafts`, `file_draft` (mod) | `paintDraft`, `applyDraft`, `routeDrafts`, `routeFileDrafts`, `fileDraft` |
| Prompts and the protocol | `agent.rs` | `src/agent.ts` (prompts, tool loop, `parseSnippets`); routing in `canvas.js` |
| Saved state | `model.rs` (`for_saving`, `load_state`) | `serialize`, load in `canvas.js`; `canvasPanel.ts` |
| Colours and sizes | constants at the top of `ui/render.rs` and `ui/mod.rs` | CSS variables in `canvas.css` |
