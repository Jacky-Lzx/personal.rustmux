# rustmux

一个用 Rust 编写的最小终端复用器。当前目标是先实现一个小而可靠的 MVP，支持在多个 PTY shell 窗口间切换。

支持 macOS 和 Linux 等 Unix 终端；Windows 暂不支持。

界面使用 Catppuccin Mocha 配色。最上方先显示当前 session 名称，再以 Zellij 风格的 powerline 箭头标签列出所有窗口并高亮当前窗口；底部使用同样的箭头分段显示 mode 与快捷键。两条状态行都使用暗色背景。当前程序通过 OSC 设置的终端标题会显示在上边框中。

## 快捷键

默认按键采用类似 Zellij 的 mode：平时处于 `locked`，`Ctrl-b` 进入 `normal`，执行大多数动作后返回 `locked`。`normal` 和 `scroll` 等非 locked mode 会显示在活动窗口标签中。

按键兼容传统控制字符、Kitty keyboard protocol 和 xterm modified-key 编码，因此在 fish 启用扩展键盘模式后仍然有效。

- `c`：创建窗口
- `,`：重命名当前窗口；输入时顶部标签会实时预览，Enter 确认，Esc 取消并恢复原名
- `n`：下一个窗口
- `p`：上一个窗口
- `s`：打开 Session Manager；输入名称搜索，使用 `↑` / `↓` 选择、Tab 补全、Enter 进入、Esc 取消。没有匹配项时，Enter 会以当前输入创建新 session
- `[`：进入当前窗口的历史模式
- `h`：用 `$VISUAL` 或 `$EDITOR`（默认 `vi`）打开当前窗口的完整历史
- `e`：用编辑器打开上一条命令的输出
- `y`：通过 OSC 52 把上一条命令的输出复制到系统剪贴板
- `i`：显示或隐藏居中的浮动 terminal；首次使用时创建一个独立 PTY shell
- `Ctrl-p`：进入 pane mode
- `&`：关闭当前窗口
- `d`：detach，断开当前终端；session 和其中的程序继续在后台运行
- 再按一次 `Ctrl-b`：把 `Ctrl-b` 发送给当前 shell

Pane mode 下可以使用：

- `r` / `n`：在当前 pane 右侧创建 pane
- `d`：在当前 pane 下方创建 pane
- `h` / `j` / `k` / `l` 或方向键：向对应方向切换焦点
- Tab：循环切换 pane
- `H` / `J` / `K` / `L`：向左 / 下 / 上 / 右扩展当前 pane
- `z`：切换当前 pane 的全屏显示
- `x`：关闭当前 pane；若它是 tab 内最后一个 pane，则关闭整个 tab
- `q` / Esc：退出 pane mode

历史模式下可以使用：

- `↑` / `k`：向上滚动一行
- `↓` / `j`：向下滚动一行
- PageUp / `u`：向上滚动一页
- PageDown / `d`：向下滚动一页
- `g` / `G`：跳到最早记录 / 返回底部
- `/`：输入文本搜索历史，Enter 确认；之后用 `n` / `N` 跳到下一个 / 上一个匹配行
- `v`：从当前光标开始键盘选择；使用 `h` / `l` 或左右方向键横向扩展，`j` / `k` 或上下方向键纵向扩展
- `y`：复制键盘选区并退出 history 模式
- `q` / Esc：退出历史模式并返回实时画面

在 history 模式中也可以使用鼠标滚轮浏览；向下滚动到最底部后会自动返回实时画面。

进入 history（`scroll`）模式后，按住鼠标左键拖动可以选择当前窗口中的文本；选区会反色显示，松开左键后自动通过 OSC 52 复制到系统剪贴板，并在左下角边框短暂显示 `copied to system clipboard`。在默认的 `locked` 模式下，鼠标事件会根据内部程序启用的鼠标协议传入当前 pane，不会触发 rustmux 的文本选择。在包含多个 pane 的 tab 中，点击任意 pane 的内容或边框会先切换焦点；如果其中的程序启用了鼠标协议，同一次点击也会继续传给该程序。

在 Kitty 0.47.0 或更高版本中，rustmux 会转发 OSC 72 drag-and-drop protocol。Yazi 可以从当前 pane 向 Finder 等 GUI 应用拖出文件，也可以接收拖入的文件；协议响应会根据 pane ID 路由，并自动换算 pane 内的单元格和像素坐标。

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
compact = false
scrollback_lines = 1000

[notifications]
enabled = true
command_duration_seconds = 10
exclude_applications = ["yazi", "nvim"]

[keybinds.locked]
"Ctrl b" = [{ action = "switch-mode", mode = "normal" }]

[keybinds.normal]
c = ["new-window", { action = "switch-mode", mode = "locked" }]
"," = ["rename-window"]
n = ["next-window", { action = "switch-mode", mode = "locked" }]
s = ["switch-session"]
"1" = [{ action = "go-to-window", index = 1 }, { action = "switch-mode", mode = "locked" }]
i = [{ action = "switch-mode", mode = "locked" }, "toggle-floating-terminal"]
"Ctrl p" = [{ action = "switch-mode", mode = "pane" }]
enter = [{ action = "switch-mode", mode = "scroll" }]
d = ["detach"]

[keybinds.scroll]
"/" = ["search-history"]
n = ["next-search-match"]
N = ["previous-search-match"]
v = ["toggle-history-selection"]
h = ["selection-left"]
l = ["selection-right"]
k = ["scroll-up"]
j = ["scroll-down"]
E = ["scroll-bottom", { action = "switch-mode", mode = "locked" }, "edit-history"]
y = ["copy-selection"]
esc = ["scroll-bottom", { action = "switch-mode", mode = "locked" }]

[keybinds.pane]
r = ["new-pane-right", { action = "switch-mode", mode = "locked" }]
d = ["new-pane-down", { action = "switch-mode", mode = "locked" }]
h = ["focus-left", { action = "switch-mode", mode = "locked" }]
j = ["focus-down", { action = "switch-mode", mode = "locked" }]
k = ["focus-up", { action = "switch-mode", mode = "locked" }]
l = ["focus-right", { action = "switch-mode", mode = "locked" }]
H = ["resize-pane-left"]
J = ["resize-pane-down"]
K = ["resize-pane-up"]
L = ["resize-pane-right"]
z = ["toggle-pane-zoom"]
x = ["close-pane", { action = "switch-mode", mode = "locked" }]
```

一个按键可以顺序执行多个动作。支持的简单动作包括 `send-prefix`、`new-window`、`rename-window`、`next-window`、`previous-window`、`switch-session`、`toggle-floating-terminal`、`new-pane-right`、`new-pane-down`、`focus-left`、`focus-right`、`focus-up`、`focus-down`、`focus-next-pane`、`resize-pane-left`、`resize-pane-right`、`resize-pane-up`、`resize-pane-down`、`toggle-pane-zoom`、`close-pane`、`close-window`、`detach`、`show-help`、`scroll-up`、`scroll-down`、`page-up`、`page-down`、`scroll-top`、`scroll-bottom`、`search-history`、`next-search-match`、`previous-search-match`、`toggle-history-selection`、`selection-left`、`selection-right`、`copy-selection`、`edit-history`、`edit-last-output` 和 `copy-last-output`。带参数的动作包括：

```toml
key = [{ action = "switch-mode", mode = "locked" }]
key = [{ action = "go-to-window", index = 2 }]
key = [{ action = "send-key", key = "Ctrl c" }]
```

默认布局会在最下面一行显示当前 mode 以及该 mode 的快捷键提示。设置 `compact = true` 后不会保留底部状态栏，pane 会使用腾出的空间，当前 mode 则显示在顶部标签栏右侧：

```toml
compact = true
```

可配置普通字符、`Ctrl a` 到 `Ctrl z`、`Alt <key>`、方向键、`enter`、`tab`、`backspace`、`esc`、`pageup` 和 `pagedown`。将某个绑定设为空数组可取消默认绑定；`clear_defaults = true` 会先移除全部默认绑定。server 会每 500ms 检查一次配置变化并自动热重载；无效配置不会替换上一份有效配置，修正后会自动恢复，删除配置文件则恢复内置默认值。

`scrollback_lines` 控制新建 pane 保存的历史行数，范围为 1 到 1,000,000；热重载后会应用于之后创建的 pane。

## 长命令完成通知

默认情况下，通过 OSC 133 检测到一条命令运行至少 10 秒并完成后，rustmux 会使用 Kitty 的 OSC 99 协议发送桌面通知。通知包含窗口编号、标题和实际运行时间；后台窗口中的命令也会触发。可以修改阈值或完全关闭：

```toml
[notifications]
enabled = true
command_duration_seconds = 10
exclude_applications = ["yazi", "nvim"]
```

`exclude_applications` 按可执行文件名过滤通知，不区分大小写；即使这些应用运行时间超过阈值也不会通知。rustmux 会检查整条命令运行期间出现过的所有前台应用，因此 `yazi` 外面包有负责切换目录的 fish 函数时也能正确过滤。默认过滤 `yazi` 和 `nvim`，可以加入其他应用；设置为 `[]` 可取消过滤。该列表也支持热重载。

该功能依赖 shell integration 提供的命令边界，默认的 fish 可以直接使用。通知需要 session 当前连接着一个 Kitty 客户端；detach 期间没有终端可接收通知。设置 `enabled = false` 可以关闭。

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

rustmux 会通过 OSC 7 shell integration 跟踪每个 pane 的当前工作目录。新建 window、pane 或浮动 terminal 时会继承当前 pane 的目录；如果 shell 尚未报告目录，则使用 session server 的启动目录。

## MVP 边界

当前版本使用后台 server 保存 session。窗口内默认运行 `fish`（可通过 `RUSTMUX_SHELL` 覆盖），支持 tab 内的水平/垂直 pane、pane resize、全屏与鼠标点击聚焦、终端字符与像素尺寸同步，并为每个 pane 维护独立的 VT100 屏幕状态和 1000 行回滚缓冲区。渲染器仅更新发生变化的单元格，避免 Vim 等全屏程序刷新时反复清屏闪烁；它们也可以安全地使用 alternate screen，边框和标签栏不会被覆盖。在支持 Kitty graphics protocol 的外层终端中，rustmux 会流式转发图片命令并渲染 Unicode placeholders，因此 Yazi 可以显示图片预览。尚未实现 pane 移动。
