# rustmux

一个用 Rust 编写的最小终端复用器。当前目标是先实现一个小而可靠的 MVP，支持在多个 PTY shell 窗口间切换。

支持 macOS 和 Linux 等 Unix 终端；Windows 暂不支持。

## 快捷键

所有命令均以 `Ctrl-b` 为前缀：

- `c`：创建窗口
- `n`：下一个窗口
- `p`：上一个窗口
- `&`：关闭当前窗口
- `d`：退出 rustmux（子进程也会被清理）
- 再按一次 `Ctrl-b`：把 `Ctrl-b` 发送给当前 shell

## 运行

```sh
cargo run
```

也可以安装到 Cargo 的二进制目录：

```sh
cargo install --path .
rustmux
```

## MVP 边界

当前版本是单进程复用器，窗口内运行 `$SHELL`，支持终端尺寸同步，并为每个窗口保留最多 1 MiB 输出用于切换时重放。退出 rustmux 会结束其 shell。尚未实现 tmux 的后台 server/session 持久化、分屏、鼠标、配置文件和完整终端状态模拟。
