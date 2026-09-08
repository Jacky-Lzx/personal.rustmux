# rustmux

一个用 Rust 编写的最小终端复用器。当前目标是先实现一个小而可靠的 MVP，支持在多个 PTY shell 窗口间切换。

支持 macOS 和 Linux 等 Unix 终端；Windows 暂不支持。

界面使用绿色边框，顶部标签栏列出所有窗口，并高亮当前窗口。

## 快捷键

所有命令均以 `Ctrl-b` 为前缀：

前缀兼容传统控制字符、Kitty keyboard protocol 和 xterm modified-key 编码，因此在 fish 启用扩展键盘模式后仍然有效。

- `c`：创建窗口
- `n`：下一个窗口
- `p`：上一个窗口
- `[`：进入当前窗口的历史模式
- `&`：关闭当前窗口
- `d`：退出 rustmux（子进程也会被清理）
- 再按一次 `Ctrl-b`：把 `Ctrl-b` 发送给当前 shell

历史模式下可以使用：

- `↑` / `k`：向上滚动一行
- `↓` / `j`：向下滚动一行
- PageUp / `u`：向上滚动一页
- PageDown / `d`：向下滚动一页
- `g` / `G`：跳到最早记录 / 返回底部
- `q` / Esc：退出历史模式并返回实时画面

## 运行

```sh
cargo run
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

当前版本是单进程复用器，窗口内默认运行 `fish`（可通过 `RUSTMUX_SHELL` 覆盖），支持终端字符尺寸同步，并为每个窗口维护独立的 VT100 屏幕状态和 1000 行回滚缓冲区。渲染器仅更新发生变化的单元格，避免 Vim 等全屏程序刷新时反复清屏闪烁；它们也可以安全地使用 alternate screen，边框和标签栏不会被覆盖。在支持 Kitty graphics protocol 的外层终端中，rustmux 会流式转发图片命令并渲染 Unicode placeholders，因此 Yazi 可以显示图片预览。退出 rustmux 会结束其 shell。尚未实现 tmux 的后台 server/session 持久化、分屏、鼠标和配置文件。
