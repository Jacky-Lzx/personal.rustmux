<section class="landing-hero">
  <div class="hero-copy">
    <p class="hero-kicker">Fast, persistent, terminal-native</p>
    <h1 class="hero-title">Rust<span>mux</span></h1>
    <p class="hero-summary">A small terminal multiplexer built for focused work: persistent sessions, fluid pane layouts, and modern Kitty protocols without a heavy layer between you and your shell.</p>
    <div class="hero-actions">
      <a class="primary-button" href="getting-started/quick-start.html">Get started <span aria-hidden="true">→</span></a>
      <a class="secondary-button" href="https://github.com/Jacky-Lzx/personal.rustmux">View on GitHub</a>
    </div>
    <div class="hero-meta" aria-label="Project highlights">
      <span>Written in Rust</span>
      <span>macOS + Linux</span>
      <span>Kitty-native</span>
    </div>
  </div>
  <div class="terminal-window" aria-label="Rustmux terminal preview">
    <div class="terminal-chrome">
      <i class="terminal-dot"></i><i class="terminal-dot"></i><i class="terminal-dot"></i>
      <span class="terminal-label">rustmux — work</span>
    </div>
    <pre><code><span class="dim">┌─ Rustmux (work) ─────────────────┐</span>
 <span class="accent">1 editor</span>  ›  <span class="prompt">2 server</span>  ›  3 notes
<span class="dim">├────────────────┬─────────────────┤</span>
│ <span class="prompt">❯</span> cargo test   │ <span class="prompt">❯</span> yazi          │
│                │                 │
│ <span class="accent">running 110</span>    │  src/           │
│ tests          │  docs/          │
│                │  Cargo.toml     │
<span class="dim">└────────────────┴─────────────────┘</span>
 <span class="dim">Ctrl b</span>  ›  <span class="prompt">NORMAL</span>  ›  ? HELP</code></pre>
  </div>
</section>

## Built around the way you work

<p class="section-intro">Rustmux keeps the familiar terminal model, then adds just enough structure to make long-running workspaces calm and dependable.</p>

<div class="feature-grid">
  <article><small class="feature-number">01 / FLOW</small><strong>A clear interaction model</strong><span>Stay locked while you work. Press Ctrl-b for an action, then return directly to the terminal.</span></article>
  <article><small class="feature-number">02 / PROTOCOLS</small><strong>Modern terminal interoperability</strong><span>Kitty keyboard, graphics, drag-and-drop, clipboard and file IPC, plus common OSC protocols.</span></article>
  <article><small class="feature-number">03 / YAZI</small><strong>Designed for terminal file work</strong><span>Image previews, cross-pane drag-and-drop, mouse input, and working-directory inheritance work through the multiplexer.</span></article>
  <article><small class="feature-number">04 / SESSIONS</small><strong>Restorable workspaces</strong><span>Keep processes alive, save layouts and working directories, and reconnect when you are ready.</span></article>
</div>

## How it works

Rustmux runs a lightweight background session server. Detaching a client leaves the session and its processes running, ready to reconnect from another terminal.

1. A **Session** is a persistent workspace.
2. A **Window** is a tab in the top status line.
3. A **Pane** is a resizable terminal region inside a window.
4. A **Mode** decides which keys Rustmux handles and which reach the active application.

## Platform support

| Platform | Status | Notes |
| --- | --- | --- |
| macOS | Supported | Kitty provides the most complete feature set |
| Linux | Supported | CI continuously runs tests and fuzz smoke tests |
| Windows | Not yet supported | The current implementation depends on Unix PTYs |

> Rustmux is still at an early stage. Configuration and persistence formats may evolve.
