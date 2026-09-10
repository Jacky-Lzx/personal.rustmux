# 核心概念

## Session

Session 是由后台 server 持有的长期工作区。关闭或 detach 客户端不会结束其中的 shell。一个 session 包含多个 window，并拥有独立名称。

## Window

Window 类似标签页。顶部状态栏显示当前 session 和全部 window；活动 window 使用绿色标签，后台 window 使用白色标签。若后台 window 中的 pane 发出 bell，window 标签右侧会显示 `[!]`。

点击 window 名称可以直接切换。`normal` mode 下也可用 `n`、`p` 和数字键导航，用 `<`、`>` 重排。

## Pane

Pane 是一个带独立 PTY、屏幕缓冲区和终端状态的区域。每个 pane 会分别保存：

- 主屏幕与备用屏幕内容；
- Kitty keyboard protocol mode stack；
- 默认前景、背景与应用请求的光标颜色；
- 鼠标协议、鼠标形状与 focus tracking；
- Kitty graphics、DnD 与 IPC 请求状态。

聚焦 pane 的边框为绿色；未读 bell 的 pane 为橙色。点击 pane 或边框可聚焦，拖动共享边框可调整比例。

## Mode

Mode 是 Rustmux 的输入上下文：

- `locked`：几乎所有输入传给内部程序。
- `normal`：window、session、历史和浮动终端操作。
- `pane`：拆分、聚焦、缩放、移动和关闭 pane。
- `scroll`：浏览、搜索和复制历史。

底部状态栏只展示当前 mode 中标记为常用的快捷键。显示不下时会出现 `? MORE (+N)`；按 `?` 打开完整帮助浮窗。

## Floating terminal

Floating terminal 是一个居中的独立 PTY。按 `normal` mode 下的 `i` 显示或隐藏；打开后它获得焦点，底层 pane 自动转为未聚焦样式。

