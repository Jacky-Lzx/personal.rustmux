# rustmux

一个用 Rust 编写的最小终端复用器。当前目标是先实现一个小而可靠的 MVP，支持在多个 PTY shell 窗口间切换。

支持 macOS 和 Linux 等 Unix 终端；Windows 暂不支持。

界面使用绿色方形边框，最上方以 Zellij 风格的标签列出所有窗口并高亮当前窗口；当前程序通过 OSC 设置的终端标题会显示在上边框中。

## 快捷键

所有命令均以 `Ctrl-b` 为前缀：

前缀兼容传统控制字符、Kitty keyboard protocol 和 xterm modified-key 编码，因此在 fish 启用扩展键盘模式后仍然有效。

- `c`：创建窗口
- `n`：下一个窗口
- `p`：上一个窗口
- `[`：进入当前窗口的历史模式
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
