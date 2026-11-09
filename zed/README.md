# CodeCanvas for Zed

Installs CodeCanvas's connection to Zed so you don't have to set it up by hand.

Zed extensions can't draw UI or register shortcuts, but they can start a program as an MCP server. So this extension downloads a small bridge program, and Zed starts it automatically. Each time, the bridge:

- finds the CodeCanvas app (`codecanvas` on `PATH`, `~/.local/bin`, `%LOCALAPPDATA%\Programs\CodeCanvas`);
- keeps a **CodeCanvas: open project** task and a **Ctrl+Alt+C** shortcut in your global Zed config pointing at it, updating the path if the app moves;
- gives Zed's AI agent an `open_canvas` tool.

The first time it changes an existing `tasks.json` or `keymap.json` it saves a `.codecanvas-backup` beside it. It keeps your entries and comments, won't take a key you've already bound, and leaves a file alone if it can't edit it safely.

## The app it launches

Whichever `codecanvas` it finds: the [CodeCanvas desktop app](https://github.com/Lightningbulb2/CodeCanvas), written in Rust on GPUI.

## Settings

In Zed's `settings.json`, both optional:

```json
"context_servers": {
  "codecanvas": {
    "settings": {
      "app_path": "/path/to/codecanvas",
      "bridge_path": "/path/to/codecanvas-zed-bridge"
    }
  }
}
```

On Guix System the downloaded Linux bridge is fully static, so it runs as is. `bridge_path` is there if you'd rather package it.

## Publishing

Pushing a `v*` tag makes GitHub Actions build the bridge and attach it to a release, which is where the extension downloads it from. Until a release exists, install the extension with Zed's `zed: install dev extension` and set `bridge_path` to a local build (`cargo build --release --manifest-path bridge/Cargo.toml`).

Zed's docs say MCP server extensions are being deprecated in favour of the official MCP registry, so this mechanism may need revisiting.
