# rustmux

一个用 Rust 编写的最小终端复用器。当前目标是先实现一个小而可靠的 MVP，支持在多个 PTY shell 窗口间切换。

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

