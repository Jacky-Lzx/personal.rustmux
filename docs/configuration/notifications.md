# 通知

通过 OSC 133 检测到一条命令达到时长阈值并完成后，Rustmux 使用 Kitty OSC 99 发送桌面通知。

```toml
[notifications]
enabled = true
command_duration_seconds = 10
exclude_applications = ["yazi", "nvim"]
```

## 应用过滤

`exclude_applications` 按可执行文件名匹配，不区分大小写。Rustmux 会记录整条命令期间出现过的所有前台应用，因此即使 Yazi 外面包了一层负责切换目录的 fish 函数，也能识别并过滤。

设置为空数组可取消过滤：

```toml
exclude_applications = []
```

## Bell 与未读状态

长命令完成也会被视为一次 pane bell。若 pane 不在焦点：

- window 名称右侧出现 `[!]`；
- pane 标题右侧出现 `[!]`；
- pane 边框变为橙色。

聚焦对应 pane 后清除。

## 前置条件

- shell 需要输出 OSC 133 命令边界；默认 fish 可直接使用；
- session 必须连接着 Kitty 客户端；detach 时没有外层终端可接收通知；
- macOS 的横幅显示方式由系统通知设置控制，通知中心记录与 Dock badge 不代表一定出现横幅。

