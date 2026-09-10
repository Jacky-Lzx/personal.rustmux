# 快捷键与模式

Rustmux 采用类似 Zellij 的 mode 设计。默认前缀是 <kbd>Ctrl-b</kbd>：在 `locked` mode 按下前缀进入 `normal`，多数动作执行后会自动返回 `locked`。

## Normal mode

| 按键 | 动作 |
| --- | --- |
| `c` | 创建 window |
| `,` | 重命名当前 window；Enter 确认，Esc 取消 |
| `n` / `p` | 下一个 / 上一个 window |
| `<` / `>` | 将当前 window 左移 / 右移 |
| `1` … `9` | 跳到对应 window |
| `s` | 打开 Session Manager |
| `[` / `Enter` | 进入 scroll mode |
| `h` | 在编辑器中打开完整历史 |
| `e` | 编辑上一条命令输出 |
| `y` | 复制上一条命令输出 |
| `i` | 显示或隐藏 floating terminal |
| `Ctrl-p` | 进入 pane mode |
| `&` / `x` | 关闭当前 window |
| `d` | Detach |
| `?` | 打开当前 mode 的完整帮助 |

在 `normal` mode 再按一次 <kbd>Ctrl-b</kbd>，前缀会发送给内部程序。

## Pane mode

| 按键 | 动作 |
| --- | --- |
| `r` / `n` | 在右侧创建 pane |
| `d` | 在下方创建 pane |
| `h j k l` / 方向键 | 向对应方向聚焦 |
| `Tab` | 循环聚焦 |
| `H J K L` | 向对应方向扩展 pane |
| `Alt-h/j/k/l` | 与对应方向最近的 pane 交换 |
| `z` | 切换 pane 全屏 |
| `x` | 关闭 pane |
| `q` / `Esc` | 返回 locked mode |

## Scroll mode

| 按键 | 动作 |
| --- | --- |
| `k` / `↑`、`j` / `↓` | 上下滚动一行 |
| `u` / `PageUp`、`d` / `PageDown` | 上下滚动一页 |
| `g` / `G` | 跳到最早记录 / 底部 |
| `/` | 搜索历史 |
| `n` / `N` | 下一个 / 上一个匹配 |
| `v` | 开始或结束键盘选择 |
| `h j k l` / 方向键 | 扩展选区 |
| `y` | 复制选区并退出 |
| `q` / `Esc` | 返回 locked mode |

滚轮到达底部不会自动退出 scroll mode。这样可以避免一次滚动意外把后续输入送进正在运行的应用。

## 帮助浮窗

按 `?` 会显示当前 mode 的完整按键表。按浮窗中的任意快捷键，会关闭浮窗并立即执行对应动作；按 Esc 只关闭浮窗。

