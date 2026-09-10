# Session 管理

## 命令行管理

```sh
# 创建或连接具名 session
rustmux -s work

# 重新连接
rustmux attach work

# 列出 session
rustmux ls

# 结束 session 及其进程
rustmux k work
```

这些短写遵循 Zellij 的 CLI 习惯：`a` 是 `attach` 的别名，`ls` 是 `list-sessions` 的别名，`k` 是 `kill-session` 的别名。`rustmux attach -c work` 会在 session 不存在时创建它。原有的 `new-session -s work`、`attach-session -t work` 和 `kill-session -t work` 写法仍然可用。

在 Rustmux 内按 <kbd>Ctrl-b</kbd>、<kbd>d</kbd> detach。server 与其中的程序会继续运行。

## Session Manager

在 `normal` mode 按 `s` 打开居中的 Session Manager。

- 输入文字：按名称过滤 session；
- `↑` / `↓`：选择；
- `Tab`：补全所选名称；
- `Enter`：进入所选 session；若无匹配，以输入内容创建新 session；
- `Ctrl-a`：保存当前 session 布局；
- `Ctrl-r`：重命名；
- `Ctrl-x`：断开所选 session 的其他客户端；
- `Delete`：删除 session 或已保存快照；
- `Esc`：关闭。

列表会显示 window/pane 数、连接状态、保存状态和创建时间。

## 保存与恢复

快照默认保存在：

```text
$XDG_STATE_HOME/rustmux/sessions
~/.local/state/rustmux/sessions   # 未设置 XDG_STATE_HOME 时
```

快照包含 window 名称、pane 分割布局与比例、活动 pane、各 pane 工作目录和 floating terminal 状态。进程和终端内容不会序列化。

server 停止后再次创建同名 session，Rustmux 会在保存的目录中启动新 shell 并重建布局。已保存但未运行的 session 也会出现在 Session Manager 中。
