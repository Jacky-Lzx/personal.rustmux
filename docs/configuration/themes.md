# Themes

Rustmux supports two interface presets and individual color overrides. Add a theme to `config.toml`:

```toml
[theme]
preset = "light"
```

`mocha` is the default, preserving the original Catppuccin Mocha interface. `light` is a light-background palette with darker accents. Preset names are lowercase.

## Customize colors

Override only the colors you need. Omitted colors inherit the selected preset; if `preset` is omitted, they inherit `mocha`.

```toml
[theme]
preset = "mocha"

[theme.colors]
background = "#1e1e2e"
foreground = "#cdd6f4"
accent = "#89b4fa"
border = "#6c7086"
warning = "#f9e2af"
```

Colors must be quoted `#RRGGBB` strings. Uppercase and lowercase hexadecimal digits are accepted. Short hex, named colors, alpha channels, unknown presets, and unknown color keys are rejected by `rustmux check-config`.

| Color | Used for | Mocha default |
| --- | --- | --- |
| `background` | Status bars and overlay backgrounds | `#1e1e2e` |
| `foreground` | Main text and inactive window tabs | `#cdd6f4` |
| `badge_text` | Text inside colored powerline segments | `#11111b` |
| `surface` | Selected session rows | `#313244` |
| `surface_highlight` | Session table separators | `#45475a` |
| `border` | Inactive pane borders and detached indicators | `#6c7086` |
| `muted` | Table headings and secondary metadata | `#a6adc8` |
| `accent` | Active borders, active tabs, and overlay headings | `#a6e3a1` |
| `error` | Locked-mode badge | `#f38ba8` |
| `orange` | Attention borders and attached-session indicators | `#fab387` |
| `warning` | Notifications and search/rename prompts | `#f9e2af` |
| `key` | Shortcut labels and input cursor markers | `#f5c2e7` |
| `secondary` | Alternating action labels and completion hints | `#b4befe` |
| `blue` | Alternating action labels | `#89b4fa` |
| `purple` | Saved-session indicators | `#cba6f7` |
| `teal` | Other session names | `#94e2d5` |

## Live updates

The running server checks configuration every 500ms. A valid theme change repaints the interface, including pane borders, the floating terminal border, Session Manager, contextual help, and notifications. Invalid updates retain the complete last valid configuration and display an error. Fixing the file applies the new theme; removing `[theme]` or deleting the configuration file restores Mocha.

Themes affect Rustmux's interface. Shells, editors, terminal ANSI colors, and application cursor colors retain their own settings. Text selections continue to invert the selected application's colors. Command-line `--help` retains its built-in styling.
