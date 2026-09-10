# Rustmux

<p class="hero-kicker">A modern terminal multiplexer, written in Rust.</p>

Rustmux 是一个小而可靠的终端复用器。它把 **Session → Window → Pane** 的工作流、Zellij 风格的状态栏和现代 Kitty 终端协议放进一个轻量的 Rust 程序中。

<div class="hero-actions">
  <a class="primary-button" href="getting-started/quick-start.html">5 分钟开始使用</a>
  <a class="secondary-button" href="https://github.com/Jacky-Lzx/personal.rustmux">查看源代码</a>
</div>

```console
$ rustmux
┌ Rustmux (work)  ❯  1 editor  ❯  2 server [!] ┐
│                                                    │
│  多窗口、多 pane、图片预览与现代终端协议              │
│                                                    │
└────────────────────────────────────────────────────┘
  Ctrl b  ❯  NORMAL  ❯  ? HELP
```

## 为什么使用 Rustmux？

<div class="feature-grid">
  <article><strong>清晰的交互模型</strong><span>默认保持 locked；按 Ctrl-b 进入操作模式，完成动作后自动回到终端。</span></article>
  <article><strong>现代终端互操作</strong><span>支持 Kitty keyboard、graphics、DnD、clipboard/file IPC，以及常用 OSC 协议。</span></article>
  <article><strong>为 Yazi 优化</strong><span>图片预览、文件拖放、鼠标输入和工作目录继承可以穿过复用器正常工作。</span></article>
  <article><strong>可恢复的工作区</strong><span>保存 window、pane 布局与工作目录；重新创建 session 时恢复工作空间。</span></article>
</div>

## 工作方式

Rustmux 启动一个后台 session server。客户端 detach 后，session 及其中的进程继续运行；之后可以从新的终端重新连接。

1. **Session** 是一个可持久运行的工作区。
2. **Window** 是顶部状态栏中的标签页。
3. **Pane** 是 window 内可拆分、缩放和重排的终端区域。
4. **Mode** 决定 Rustmux 当前处理哪些按键，其余输入会传给 pane 中的程序。

## 平台支持

| 平台 | 状态 | 说明 |
| --- | --- | --- |
| macOS | 支持 | Kitty 是功能最完整的外层终端 |
| Linux | 支持 | CI 持续运行测试与 fuzz smoke test |
| Windows | 暂不支持 | 当前实现依赖 Unix PTY |

> Rustmux 仍处于早期阶段。配置与存储格式可能在后续版本中调整。

