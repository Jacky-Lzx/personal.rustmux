# Rustmux

<p class="hero-kicker">A modern terminal multiplexer, written in Rust.</p>

Rustmux is a small, reliable terminal multiplexer. It combines a **Session → Window → Pane** workflow, Zellij-inspired status bars, and modern Kitty terminal protocols in a lightweight Rust application.

<div class="hero-actions">
  <a class="primary-button" href="getting-started/quick-start.html">Get started in 5 minutes</a>
  <a class="secondary-button" href="https://github.com/Jacky-Lzx/personal.rustmux">View source</a>
</div>

```console
$ rustmux
┌ Rustmux (work)  ❯  1 editor  ❯  2 server [!] ┐
│                                                  │
│  Multiple windows, panes, previews, and protocols │
│                                                  │
└──────────────────────────────────────────────────┘
  Ctrl b  ❯  NORMAL  ❯  ? HELP
```

## Why Rustmux?

<div class="feature-grid">
  <article><strong>A clear interaction model</strong><span>Stay locked while you work. Press Ctrl-b for actions, then return directly to the terminal.</span></article>
  <article><strong>Modern terminal interoperability</strong><span>Kitty keyboard, graphics, drag-and-drop, clipboard/file IPC, and common OSC protocols.</span></article>
  <article><strong>Designed for Yazi</strong><span>Image previews, file drag-and-drop, mouse input, and working-directory inheritance work across the multiplexer.</span></article>
  <article><strong>Restorable workspaces</strong><span>Save window and pane layouts with working directories, then rebuild the workspace later.</span></article>
</div>

## How it works

Rustmux starts a background session server. Detaching or closing a client leaves the session and its processes running, ready to reconnect from another terminal.

1. A **Session** is a persistent workspace.
2. A **Window** is a tab in the top status line.
3. A **Pane** is a resizable terminal region inside a window.
4. A **Mode** determines which keys Rustmux handles and which keys reach the application inside the active pane.

## Platform support

| Platform | Status | Notes |
| --- | --- | --- |
| macOS | Supported | Kitty provides the most complete feature set |
| Linux | Supported | CI continuously runs tests and fuzz smoke tests |
| Windows | Not yet supported | The current implementation depends on Unix PTYs |

> Rustmux is still at an early stage. Configuration and persistence formats may evolve.

