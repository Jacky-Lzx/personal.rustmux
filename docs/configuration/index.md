# Configuration File

Rustmux reads `$XDG_CONFIG_HOME/rustmux/config.toml`, or `~/.config/rustmux/config.toml` when `XDG_CONFIG_HOME` is unset.

## Create and validate

```sh
mkdir -p ~/.config/rustmux
rustmux default-config > ~/.config/rustmux/config.toml
rustmux check-config
```

## General options

```toml
default_mode = "locked"
clear_defaults = false
compact = false
scrollback_lines = 1000
autosave_interval_seconds = 30
# shell = "/bin/zsh"
```

| Option | Purpose |
| --- | --- |
| `default_mode` | Mode used at startup |
| `clear_defaults` | Remove built-in bindings before applying custom bindings |
| `compact` | Hide the bottom status line and show the mode at the top right |
| `autosave_interval_seconds` | Save changed layouts periodically and on detach/server shutdown; `0` disables automatic saving |
| `scrollback_lines` | History capacity for new panes, from 1 to 1,000,000 |

## Configure keybindings

A key can run multiple actions in sequence:

```toml
[keybinds.normal]
c = ["new-window", { action = "switch-mode", mode = "locked" }]
"," = ["rename-window"]
"Ctrl p" = [{ action = "switch-mode", mode = "pane" }]
"1" = [{ action = "go-to-window", index = 1 }, { action = "switch-mode", mode = "locked" }]
```

Parameterized actions use object syntax:

```toml
key = [{ action = "switch-mode", mode = "locked" }]
key = [{ action = "go-to-window", index = 2 }]
key = [{ action = "send-key", key = "Ctrl c" }]
```

Set a binding to an empty array to remove its default:

```toml
[keybinds.normal]
x = []
```

## Control hint visibility

The expanded form controls where a binding appears:

```toml
[keybinds.normal]
c = { display = "help" }
d = { display = "hidden" }
z = { actions = ["new-window"], display = "always" }
```

| Value | Status line | Help overlay |
| --- | --- | --- |
| `always` | Shown | Shown |
| `help` | Hidden | Shown |
| `hidden` | Hidden | Hidden |

When overriding an existing binding, `display` can be provided alone. New bindings must also provide `actions`.

## Supported actions

Windows and sessions: `new-window`, `rename-window`, `next-window`, `previous-window`, `move-window-left`, `move-window-right`, `go-to-window`, `switch-session`, `close-window`, `detach`.

Panes: `new-pane-right`, `new-pane-down`, `focus-left/right/up/down`, `focus-next-pane`, `move-pane-left/right/up/down`, `resize-pane-left/right/up/down`, `toggle-pane-zoom`, `break-pane`, `move-pane-next-window`, `move-pane-previous-window`, `close-pane`, `toggle-floating-terminal`.

History: `scroll-up/down`, `page-up/down`, `scroll-top/bottom`, `search-history`, `next-search-match`, `previous-search-match`, `toggle-history-selection`, `selection-left/right`, `copy-selection`, `edit-history`, `edit-last-output`, `copy-last-output`.

General: `switch-mode`, `send-prefix`, `send-key`, `show-help`.

## Hot reload

The server checks for configuration changes every 500ms. Valid updates apply automatically. Invalid files leave the previous valid settings active and apply after correction. Removing the file restores built-in defaults.

`scrollback_lines` affects only panes created afterward; bindings and notification settings apply immediately.


Set `shell` to an executable name or path (without arguments). `RUSTMUX_SHELL` takes precedence; otherwise the default is `$SHELL`, falling back to `/bin/sh`. Reloaded shell settings apply to new panes.

## Interface theme

Choose `mocha` (default) or `light`, then optionally override individual colors. Themes reload automatically. See [Themes](themes.md) for all supported color keys.

```toml
[theme]
preset = "mocha"

[theme.colors]
accent = "#89b4fa"
```
