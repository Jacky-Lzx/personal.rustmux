# 终端兼容性

## 支持矩阵

| 能力 | 协议 | 状态 |
| --- | --- | --- |
| 扩展键盘输入 | Kitty keyboard / CSI-u / xterm modified keys | 支持，按 pane 与 screen 隔离 |
| 图片预览 | Kitty graphics + Unicode placeholder | 支持 |
| 文件拖放 | Kitty OSC 72 | 支持，Kitty 0.47+ |
| 富剪贴板 | Kitty OSC 5522 | 支持，请求按 pane 路由 |
| 文件传输 | Kitty OSC 5113 | 支持，请求按 pane 路由 |
| 桌面通知 | Kitty OSC 99 | 支持 |
| 系统剪贴板 | OSC 52 | 支持，取决于外层终端权限 |
| Shell integration | OSC 7 / OSC 133 | 支持 |
| 超链接 | OSC 8 | 支持，随 pane 重绘 |
| 颜色 | OSC 4、10/11/12、Kitty OSC 21 | 支持，按 pane 隔离 |
| 鼠标形状 | OSC 22 | 支持，按 pane 隔离 |
| Focus tracking | `?1004` | 支持 |

## 状态隔离

终端复用器不仅要转发字节，还要防止一个 pane 修改另一个 pane 的状态。Rustmux 分别保存各 pane 的屏幕、颜色、键盘模式、鼠标模式、图像与协议请求；焦点切换时只把当前 pane 的状态同步到外层终端。

Rustmux 自身界面使用 Catppuccin Mocha，但不会把自身的光标颜色设置传入 shell。内部应用请求的光标颜色仍会按 pane 保存和恢复。

## 外层终端建议

Kitty 提供最完整的协议组合。其他 Unix 终端仍可运行基础 window、pane、历史与 session 功能，但图片、拖放、通知或扩展键盘能力取决于外层终端是否实现相应协议。

