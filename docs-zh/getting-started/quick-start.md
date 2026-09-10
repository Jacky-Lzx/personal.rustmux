# 安装与快速开始

## 从源码运行

Rustmux 需要稳定版 Rust toolchain，以及 macOS 或 Linux 环境。

```sh
git clone https://github.com/Jacky-Lzx/personal.rustmux.git
cd personal.rustmux
cargo run --release
```

不带参数运行时，Rustmux 会连接已有的 `default` session；如果不存在则自动创建。

## 安装二进制

```sh
cargo install --path .
rustmux
```

新 window 默认启动 `fish`。可为单次启动覆盖 shell：

```sh
RUSTMUX_SHELL=zsh rustmux
```

## 第一个工作区

进入 Rustmux 后，你通常处于 `locked` mode，输入会直接交给 shell。

1. 按 <kbd>Ctrl-b</kbd> 进入 `normal` mode。
2. 按 <kbd>c</kbd> 创建一个 window。
3. 再按 <kbd>Ctrl-b</kbd>，然后按 <kbd>Ctrl-p</kbd> 进入 pane mode。
4. 按 <kbd>r</kbd> 向右拆分 pane，或按 <kbd>d</kbd> 向下拆分。
5. 按 <kbd>Ctrl-b</kbd>、<kbd>d</kbd> detach；再次运行 `rustmux` 即可连接回来。

## 生成配置

```sh
mkdir -p ~/.config/rustmux
rustmux default-config > ~/.config/rustmux/config.toml
rustmux check-config
```

对应的 Zellij 风格写法是 `rustmux setup --dump-config` 和 `rustmux setup --check`。

配置文件保存后会在约 500ms 内热重载。无效配置不会替换上一份有效配置。

## 下一步

- 阅读[快捷键与模式](../guide/keybindings.md)，理解输入如何在 Rustmux 与内部程序之间传递。
- 在[配置文件](../configuration/index.md)中调整快捷键和状态栏显示方式。
- 如果使用 Kitty 或 Yazi，查看[Yazi 与 Kitty](../guide/kitty-yazi.md)。
