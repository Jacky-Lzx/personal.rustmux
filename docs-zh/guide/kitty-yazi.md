# Yazi 与 Kitty

Kitty 是 Rustmux 功能最完整的外层终端。Rustmux 会在 pane 之间隔离协议状态，并在外层终端与当前 pane 之间转发请求和响应。

## 图片预览

Rustmux 支持 Kitty graphics protocol 与 Unicode placeholder。Yazi 可在 pane 内显示图片预览，Rustmux 会流式处理 graphics 分块，并只重绘发生变化的终端单元格。

PDF 与 SVG 的栅格化由 Yazi 调用的外部工具完成；Rustmux 负责后续 graphics 数据与终端帧合成。

## 文件拖放

Kitty 0.47.0 或更新版本支持 OSC 72 drag-and-drop protocol。通过 Rustmux，Yazi 可以：

- 从当前 pane 拖文件到 Finder 等 GUI 应用；
- 接收从外部拖入的文件；
- 按 pane ID 路由协议响应；
- 把外层坐标换算成 pane 内的单元格与像素坐标。

## Clipboard 与文件传输

Rustmux 支持 Kitty OSC 5522 clipboard protocol 与 OSC 5113 file transfer，并为多个 pane 隔离请求 ID。外层终端返回结果时，会路由到发起请求的 pane。

## Keyboard 与鼠标

Rustmux 为每个 pane、主屏幕和备用屏幕维护 Kitty keyboard protocol mode stack。切换 pane 时同步 progressive-enhancement flags；修饰键、重复/释放事件和 associated text 可以到达内部程序。

locked mode 下，内部程序启用的鼠标协议会正常收到事件。Rustmux 只保留 window 标签点击、pane 聚焦和共享边框拖动等自身交互。

