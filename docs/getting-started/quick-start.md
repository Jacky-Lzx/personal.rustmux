# Installation and Quick Start

## Run from source

Rustmux requires [a stable Rust toolchain](https://www.rust-lang.org/tools/install) and macOS or Linux.

```sh
git clone https://github.com/Jacky-Lzx/Rustmux.git
cd Rustmux
cargo run --release
```

Running without arguments attaches to the existing `default` session, or creates it when it does not exist.

## Install the binary

```sh
cargo install --path .
rustmux
```

New windows use `RUSTMUX_SHELL`, then the `shell` configuration option, then `$SHELL`, and finally `/bin/sh`. Explicit overrides must name an executable; invalid overrides report an error. Override the shell for one launch with:

```sh
RUSTMUX_SHELL=zsh rustmux
```

## Build your first workspace

Rustmux normally starts in `locked` mode, where input goes directly to the shell.

1. Press <kbd>Ctrl-b</kbd> to enter `normal` mode.
2. Press <kbd>c</kbd> to create a window.
3. Press <kbd>Ctrl-b</kbd>, then <kbd>Ctrl-p</kbd> to enter pane mode.
4. Press <kbd>r</kbd> to split right, or <kbd>d</kbd> to split down.
5. Press <kbd>Ctrl-b</kbd>, <kbd>Ctrl-o</kbd>, <kbd>d</kbd> to detach. Run `rustmux` again to reconnect.

## Generate a configuration

```sh
mkdir -p ~/.config/rustmux
rustmux default-config > ~/.config/rustmux/config.toml
rustmux check-config
```

The Zellij-style equivalents are `rustmux setup --dump-config` and `rustmux setup --check`.

Saved changes hot-reload in about 500ms. Invalid changes never replace the last valid configuration.

## Next steps

- Read [Keybindings and Modes](../guide/keybindings.md) to understand how input moves between Rustmux and terminal applications.
- Customize bindings and status-line visibility in [Configuration File](../configuration/index.md).
- If you use Kitty or Yazi, continue with [Yazi and Kitty](../guide/kitty-yazi.md).
