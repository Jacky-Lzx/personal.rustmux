<section class="landing-hero">
  <div class="hero-copy">
    <p class="hero-kicker">快速 · 持久 · 原生终端体验</p>
    <h1 class="hero-title">Rust<span>mux</span></h1>
    <p class="hero-summary">一个为专注工作而生的小型终端复用器：持久 session、流畅的 pane 布局，以及现代 Kitty 协议支持，不在你和 shell 之间堆叠沉重的抽象。</p>
    <div class="hero-actions">
      <a class="primary-button" href="getting-started/quick-start.html">开始使用 <span aria-hidden="true">→</span></a>
      <a class="secondary-button" href="https://github.com/Jacky-Lzx/personal.rustmux">查看 GitHub</a>
    </div>
    <div class="hero-meta" aria-label="项目特点">
      <span>Rust 编写</span>
      <span>macOS + Linux</span>
      <span>Kitty 原生</span>
    </div>
  </div>
  <div class="terminal-window" aria-label="Rustmux 终端预览">
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

## 围绕你的工作方式设计

<p class="section-intro">Rustmux 保留熟悉的终端使用模型，只加入恰到好处的结构，让长期运行的工作区保持清晰、稳定。</p>

<div class="feature-grid">
  <article><small class="feature-number">01 / FLOW</small><strong>清晰的交互模型</strong><span>默认保持 locked；按 Ctrl-b 执行动作，完成后立即回到终端。</span></article>
  <article><small class="feature-number">02 / PROTOCOLS</small><strong>现代终端互操作</strong><span>支持 Kitty keyboard、graphics、拖放、clipboard/file IPC，以及常用 OSC 协议。</span></article>
  <article><small class="feature-number">03 / YAZI</small><strong>为终端文件工作优化</strong><span>图片预览、跨 pane 文件拖放、鼠标输入和工作目录继承都能穿过复用器。</span></article>
  <article><small class="feature-number">04 / SESSIONS</small><strong>可恢复的工作区</strong><span>保持进程运行，保存布局与工作目录，在需要时随时重新连接。</span></article>
</div>

## 工作方式

Rustmux 运行一个轻量的后台 session server。客户端 detach 后，session 及其中的进程继续运行，之后可以从另一个终端重新连接。

1. **Session** 是一个持久运行的工作区。
2. **Window** 是顶部状态栏中的标签页。
3. **Pane** 是 window 内可调整大小的终端区域。
4. **Mode** 决定哪些按键由 Rustmux 处理，哪些按键传给当前应用。

## 平台支持

| 平台 | 状态 | 说明 |
| --- | --- | --- |
| macOS | 支持 | Kitty 提供最完整的功能支持 |
| Linux | 支持 | CI 持续运行测试与 fuzz smoke test |
| Windows | 暂不支持 | 当前实现依赖 Unix PTY |

> Rustmux 仍处于早期阶段，配置与持久化格式可能继续演进。
