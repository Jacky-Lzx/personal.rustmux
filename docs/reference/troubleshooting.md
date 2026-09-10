# 故障排查

## 启动后没有任何显示

先运行开发构建查看错误：

```sh
cargo run
```

然后检查配置：

```sh
cargo run -- check-config
```

若已有后台 session，直接运行会尝试连接 `default`；也可使用 `list-sessions` 检查状态。

## Esc 在内部应用里响应慢或失效

Esc 同时可能是普通按键，也是 OSC/CSI 等转义序列的起始字节。Rustmux 使用很短的解码窗口判断后续字节是否到达；超时后会把单独 Esc 立即释放给 pane。若问题只在某个应用中出现，请同时确认该应用是否启用了 Kitty keyboard protocol。

## Kitty 通知进入通知中心但不弹横幅

这通常是 macOS 的通知展示设置，而不是协议丢失。在“系统设置 → 通知 → kitty”中将提醒样式设为“横幅”或“提醒”，并确认专注模式没有抑制通知。

如果直接在 Kitty 中有 Dock badge、Rustmux 中没有，请确认：

- session 当前没有 detach；
- shell integration 提供了 OSC 133；
- 命令达到 `command_duration_seconds`；
- 前台应用不在 `exclude_applications` 中。

## Yazi 退出后仍产生长命令通知

保留默认的：

```toml
[notifications]
exclude_applications = ["yazi", "nvim"]
```

Rustmux 会检查命令运行期间出现过的前台进程，不只检查最外层 fish 函数。

## 图片预览慢

使用项目内 benchmark 区分 Rustmux 合成开销与外部栅格化开销：

```sh
cargo bench --features benchmarks --bench image_preview
```

该测试不包含 Yazi 将 PDF/SVG/JPEG 转换为预览图片的时间，也不包含 Kitty 自身解码与绘制时间。

## OSC 52 无法复制

外层终端必须允许应用写入系统剪贴板。Rustmux 会发送 OSC 52，但终端安全设置可能拒绝该请求。

## Detach 后原 shell 留下界面残影

Rustmux 会使用备用屏幕并在退出或 detach 时恢复外层终端状态。若仍能复现，请记录外层终端名称、版本和最小操作步骤，并附上 `rustmux --version` 与终端配置。

