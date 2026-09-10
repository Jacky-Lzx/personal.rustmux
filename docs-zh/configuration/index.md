# 配置文件

Rustmux 默认读取 `$XDG_CONFIG_HOME/rustmux/config.toml`；未设置 `XDG_CONFIG_HOME` 时读取 `~/.config/rustmux/config.toml`。

## 创建与校验

```sh
mkdir -p ~/.config/rustmux
rustmux default-config > ~/.config/rustmux/config.toml
rustmux check-config
```

## 基础选项

```toml
default_mode = "locked"
clear_defaults = false
compact = false
scrollback_lines = 1000
```

| 选项 | 作用 |
| --- | --- |
| `default_mode` | 启动时的 mode |
| `clear_defaults` | 配置自定义按键前，是否移除所有默认绑定 |
| `compact` | 隐藏底部状态栏，将 mode 放到顶部右侧 |
| `scrollback_lines` | 新 pane 的历史行数，范围 1–1,000,000 |

## 配置快捷键

一个快捷键可以顺序执行多个动作：

```toml
[keybinds.normal]
c = ["new-window", { action = "switch-mode", mode = "locked" }]
"," = ["rename-window"]
"Ctrl p" = [{ action = "switch-mode", mode = "pane" }]
"1" = [{ action = "go-to-window", index = 1 }, { action = "switch-mode", mode = "locked" }]
```

带参数的动作使用对象形式：

```toml
key = [{ action = "switch-mode", mode = "locked" }]
key = [{ action = "go-to-window", index = 2 }]
key = [{ action = "send-key", key = "Ctrl c" }]
```

将绑定设为空数组可取消默认绑定：

```toml
[keybinds.normal]
x = []
```

## 控制提示显示

详细写法可控制绑定出现在哪里：

```toml
[keybinds.normal]
c = { display = "help" }
d = { display = "hidden" }
z = { actions = ["new-window"], display = "always" }
```

| 值 | 状态栏 | 帮助浮窗 |
| --- | --- | --- |
| `always` | 显示 | 显示 |
| `help` | 隐藏 | 显示 |
| `hidden` | 隐藏 | 隐藏 |

覆盖已有绑定时可以只写 `display`；新增绑定时必须同时提供 `actions`。

## 支持的动作

Window 与 session：`new-window`、`rename-window`、`next-window`、`previous-window`、`move-window-left`、`move-window-right`、`go-to-window`、`switch-session`、`close-window`、`detach`。

Pane：`new-pane-right`、`new-pane-down`、`focus-left/right/up/down`、`focus-next-pane`、`move-pane-left/right/up/down`、`resize-pane-left/right/up/down`、`toggle-pane-zoom`、`close-pane`、`toggle-floating-terminal`。

历史：`scroll-up/down`、`page-up/down`、`scroll-top/bottom`、`search-history`、`next-search-match`、`previous-search-match`、`toggle-history-selection`、`selection-left/right`、`copy-selection`、`edit-history`、`edit-last-output`、`copy-last-output`。

通用：`switch-mode`、`send-prefix`、`send-key`、`show-help`。

## 热重载

server 每 500ms 检查一次配置变化。有效修改会自动应用；无效配置保留上一份有效设置，修正后自动恢复。删除配置文件会恢复内置默认值。

`scrollback_lines` 只影响之后创建的 pane；通知与按键设置会立即应用。

