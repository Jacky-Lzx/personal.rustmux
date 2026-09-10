# 历史、搜索与复制

## 浏览历史

按 <kbd>Ctrl-b</kbd>、<kbd>[</kbd> 进入 scroll mode。可以使用键盘或滚轮浏览；滚动到底部后仍会留在 scroll mode，直到按 `q` 或 Esc。

每个新 pane 默认保留 1000 行，可通过 `scrollback_lines` 调整为 1 到 1,000,000。

## 搜索

在 scroll mode 按 `/` 输入搜索词，Enter 确认。使用 `n` 跳到下一处，`N` 回到上一处。

## 复制文本

有两种选择方式：

1. 按 `v` 开始键盘选择，用方向键或 `h j k l` 扩展，再按 `y` 复制。
2. 按住鼠标左键拖动；松开后自动通过 OSC 52 复制，并显示短暂提示。

鼠标复制只在 scroll mode 生效。locked mode 下鼠标输入会传给 Yazi、Neovim 等内部程序。

## 编辑历史与命令输出

在 normal mode：

- `h` 使用 `$VISUAL` 或 `$EDITOR` 打开完整历史；
- `e` 打开上一条命令的输出；
- `y` 通过 OSC 52 复制上一条命令输出。

命令输出优先使用 OSC 133 shell integration 提供的精确边界；没有 OSC 133 时会使用命令回显和提示符进行兼容性提取。

