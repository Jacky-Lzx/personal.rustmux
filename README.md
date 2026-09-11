# rustmux

Rustmux is a terminal multiplexer written in Rust, built around a compact modal workflow and first-class support for modern terminal protocols.

It runs on macOS and Linux. Windows is not currently supported.

For installation guides, keybindings, configuration, session management, and protocol compatibility, see the [Rustmux Documentation](docs/index.md). The documentation is written in English by default and also includes a Chinese translation.

## Feature demos

### Windows, panes, and rearrangement

Create and rename windows, split and focus panes, rearrange layouts, and zoom the active pane.

![Rustmux window and pane workflow](demos/assets/windows-and-panes.gif)

### Session Manager

Search and switch between running or saved sessions from a compact overlay. Entering a new name creates a session immediately.

![Rustmux session manager](demos/assets/session-manager.gif)

### History search and contextual help

Browse and search scrollback, select text with the keyboard or mouse, and execute shortcuts directly from the help overlay.

![Rustmux help overlay and history search](demos/assets/history-and-help.gif)

The demos are generated from reproducible [VHS tapes](demos/README.md).

## Highlights

- **Zellij-inspired interface** — Configurable Mocha and light themes, powerline window tabs, modal keybindings, and compact floating overlays.
- **Persistent sessions** — Detach and reconnect without stopping applications, or save pane layouts and working directories for later restoration.
- **Flexible layouts** — Split, resize, move, zoom, and close panes using the keyboard or by dragging pane borders with the mouse.
- **Integrated history tools** — Search scrollback, copy selections through OSC 52, and open the full history or the previous command output in an editor.
- **Kitty and Yazi interoperability** — Supports Kitty graphics, keyboard enhancements, file drag and drop, rich clipboard and file transfer, terminal notifications, and related OSC protocols.
- **Attention indicators** — Bells and completed long-running commands mark the relevant window and pane until it receives focus.
- **Efficient rendering** — Each pane maintains an independent terminal state, including alternate screens, scrollback, colors, mouse modes, and image placeholders; only changed cells are redrawn.
- **Live configuration reloads** — Configuration changes are applied without restarting the session server, while invalid updates leave the last valid configuration active.

## Quick start

Run from the repository:

```sh
cargo run --release
```

Or install the binary locally:

```sh
cargo install --path .
rustmux
```

Rustmux creates or attaches to the default session. Use `Ctrl-b` to enter normal mode and `?` to open the contextual keybinding help.

## Documentation

Build the complete English and Chinese documentation site with:

```sh
./scripts/build-docs.sh
```

For live preview of the English documentation:

```sh
mdbook serve --open
```

Detailed configuration, commands, keybindings, benchmarks, fuzz targets, and terminal-protocol notes live in the [documentation](docs/index.md) rather than this README.
