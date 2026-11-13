# CodeCanvas

An infinite canvas for source code: files are laid out on a canvas, lines
can be cut into movable snippets and merged back, and each file or snippet
can have its own chat with a language model.

| Directory | What |
|---|---|
| [`rust/`](rust) | The desktop app, a native binary built on GPUI (Zed's UI framework) |
| [`vscode/`](vscode) | The extension for VS Code and VSCodium |

Each directory is a project of its own, with its own README, build and
license file.

On Guix, both are packaged in the
[hyperguix](https://github.com/Lightningbulb2/hyperguix) channel as
`codecanvas` and `codecanvas-vscode`.

Working on the code: [ARCHITECTURE.md](ARCHITECTURE.md) explains how both are built, and
[AGENTS.md](AGENTS.md) has the rules, build and test commands, and how to measure.
