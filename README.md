# rustmux

一个用 Rust 编写的最小终端复用器。当前目标是先实现一个小而可靠的 MVP，支持在多个 PTY shell 窗口间切换。

支持 macOS 和 Linux 等 Unix 终端；Windows 暂不支持。

界面使用绿色方形边框，最上方以 Zellij 风格的标签列出所有窗口并高亮当前窗口；当前程序通过 OSC 设置的终端标题会显示在上边框中。

## 快捷键

默认按键采用类似 Zellij 的 mode：平时处于 `locked`，`Ctrl-b` 进入 `normal`，执行大多数动作后返回 `locked`。`normal` 和 `scroll` 等非 locked mode 会显示在活动窗口标签中。

按键兼容传统控制字符、Kitty keyboard protocol 和 xterm modified-key 编码，因此在 fish 启用扩展键盘模式后仍然有效。

- `c`：创建窗口
- `n`：下一个窗口
- `p`：上一个窗口
- `[`：进入当前窗口的历史模式
- `h`：用 `$VISUAL` 或 `$EDITOR`（默认 `vi`）打开当前窗口的完整历史
- `e`：用编辑器打开上一条命令的输出
- `y`：通过 OSC 52 把上一条命令的输出复制到系统剪贴板
- `&`：关闭当前窗口
- `d`：detach，断开当前终端；session 和其中的程序继续在后台运行
- 再按一次 `Ctrl-b`：把 `Ctrl-b` 发送给当前 shell

历史模式下可以使用：

- `↑` / `k`：向上滚动一行
- `↓` / `j`：向下滚动一行
- PageUp / `u`：向上滚动一页
- PageDown / `d`：向下滚动一页
- `g` / `G`：跳到最早记录 / 返回底部
- `q` / Esc：退出历史模式并返回实时画面

也可以直接使用鼠标滚轮：向上滚动会自动进入历史模式，向下滚动到最底部后会自动返回实时画面。

按住鼠标左键拖动可以选择当前窗口中的文本；选区会反色显示，松开左键后自动通过 OSC 52 复制到系统剪贴板。选择支持多行文本和历史模式中的当前画面。

上一条命令输出会优先使用 OSC 133 shell integration 提供的精确命令边界；fish 等现代 shell 可直接使用。没有 OSC 133 时，rustmux 会根据回车、命令回显和下一段提示符进行兼容性提取。OSC 52 剪贴板需要外层终端允许应用写入剪贴板。

## 按键与 mode 配置

rustmux 使用 TOML 配置，默认读取 `$XDG_CONFIG_HOME/rustmux/config.toml`；未设置 `XDG_CONFIG_HOME` 时读取 `~/.config/rustmux/config.toml`。可以生成一份包含所有默认绑定的配置：

```sh
mkdir -p ~/.config/rustmux
rustmux default-config > ~/.config/rustmux/config.toml
rustmux check-config
```

配置结构与 Zellij 的 mode 思路一致，但不使用 KDL：

```toml
default_mode = "locked"
clear_defaults = false

[keybinds.locked]
"Ctrl b" = [{ action = "switch-mode", mode = "normal" }]

[keybinds.normal]
c = ["new-window", { action = "switch-mode", mode = "locked" }]
n = ["next-window", { action = "switch-mode", mode = "locked" }]
"1" = [{ action = "go-to-window", index = 1 }, { action = "switch-mode", mode = "locked" }]
enter = [{ action = "switch-mode", mode = "scroll" }]
d = ["detach"]

[keybinds.scroll]
k = ["scroll-up"]
j = ["scroll-down"]
E = ["scroll-bottom", { action = "switch-mode", mode = "locked" }, "edit-history"]
y = ["copy-last-output", "scroll-bottom", { action = "switch-mode", mode = "locked" }]
esc = ["scroll-bottom", { action = "switch-mode", mode = "locked" }]
```

一个按键可以顺序执行多个动作。支持的简单动作包括 `send-prefix`、`new-window`、`next-window`、`previous-window`、`close-window`、`detach`、`show-help`、`scroll-up`、`scroll-down`、`page-up`、`page-down`、`scroll-top`、`scroll-bottom`、`edit-history`、`edit-last-output` 和 `copy-last-output`。带参数的动作包括：

```toml
key = [{ action = "switch-mode", mode = "locked" }]
key = [{ action = "go-to-window", index = 2 }]
key = [{ action = "send-key", key = "Ctrl c" }]
```

可配置普通字符、`Ctrl a` 到 `Ctrl z`、`Alt <key>`、方向键、`enter`、`tab`、`backspace`、`esc`、`pageup` 和 `pagedown`。将某个绑定设为空数组可取消默认绑定；`clear_defaults = true` 会先移除全部默认绑定。配置在创建 session 时加载，修改后需要结束并重新创建 session。

## 运行

```sh
cargo run
```

不带参数运行会连接已有的 `default` session；如果不存在则自动创建：

```sh
rustmux
```

也可以创建和管理具名 session：

```sh
# 创建（或连接）名为 work 的 session
rustmux new-session -s work

# Ctrl-b d 后重新连接
rustmux attach-session -t work

# 列出 session
rustmux list-sessions

# 结束 session 及其窗口进程
rustmux kill-session -t work
```

也可以安装到 Cargo 的二进制目录：

```sh
cargo install --path .
rustmux
```

新窗口默认启动 `fish`。如需临时使用其他 shell，可以设置：

```sh
RUSTMUX_SHELL=zsh rustmux
```

## MVP 边界

当前版本使用后台 server 保存 session。窗口内默认运行 `fish`（可通过 `RUSTMUX_SHELL` 覆盖），支持终端字符与像素尺寸同步，并为每个窗口维护独立的 VT100 屏幕状态和 1000 行回滚缓冲区。渲染器仅更新发生变化的单元格，避免 Vim 等全屏程序刷新时反复清屏闪烁；它们也可以安全地使用 alternate screen，边框和标签栏不会被覆盖。在支持 Kitty graphics protocol 的外层终端中，rustmux 会流式转发图片命令并渲染 Unicode placeholders，因此 Yazi 可以显示图片预览。尚未实现分屏、鼠标点击转发和配置文件。
