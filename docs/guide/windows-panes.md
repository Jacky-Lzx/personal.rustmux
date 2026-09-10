# Window 与 Pane

## 创建与重命名 Window

在 `normal` mode 按 `c` 创建 window。按 `,` 进入重命名：输入内容会实时反映在顶部标签中；Enter 保存，Esc 取消并恢复原名。

点击顶部任一 window 标签可以直接切换。标签右侧的 `[!]` 表示其中有 pane 发出了尚未查看的 bell。

## 拆分 Pane

按 <kbd>Ctrl-b</kbd>、<kbd>Ctrl-p</kbd> 进入 pane mode：

```text
r / n   在当前 pane 右侧拆分
d       在当前 pane 下方拆分
z       当前 pane 全屏/恢复
x       关闭当前 pane
```

新 pane 会继承当前 pane 通过 OSC 7 报告的工作目录。若 shell 尚未报告目录，则使用 session server 的启动目录。

## 聚焦与调整大小

使用 `h j k l` 或方向键切换焦点。大写 `H J K L` 调整当前 pane 的边界；按住 `Alt` 使用对应方向键位，可以与最近的 pane 交换位置。

鼠标也可以完成常见操作：

- 点击 pane 内容或边框以聚焦；
- 拖动两个 pane 的共享边框，连续调整分割比例；
- 当内部程序启用了鼠标协议，locked mode 下的其他鼠标事件会传给程序。

## Bell 状态

pane 发出 BEL，或一个达到通知阈值的命令完成时，会产生未读状态：

- pane 标题右侧显示 `[!]`；
- pane 边框变为橙色；
- 对应 window 标签右侧显示 `[!]`。

聚焦到发出 bell 的 pane 后，以上提示会被清除。

