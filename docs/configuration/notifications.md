# Notifications

When [OSC 133](https://iterm2.com/documentation-escape-codes.html) reports that a command ran past the configured threshold and completed, Rustmux sends a desktop notification with [Kitty OSC 99](https://sw.kovidgoyal.net/kitty/desktop-notifications/).

```toml
[notifications]
enabled = true
command_duration_seconds = 10
exclude_applications = ["yazi", "nvim", "lazygit"]
```

## Application filters

`exclude_applications` matches executable names case-insensitively. Rustmux records every foreground application observed during a command, so Yazi is still recognized when a Fish wrapper function launches it and changes directories afterward.

Use an empty list to disable filtering:

```toml
exclude_applications = []
```

## Bell and unread state

Long-command completion also counts as a pane bell. If the pane is not focused:

- `[!]` appears beside the window name;
- `[!]` appears beside the pane title;
- the pane border turns orange.

Focusing that pane clears the indicators.

## Requirements

- The shell must provide OSC 133 command boundaries. Rustmux does not install shell integration; [configure it in the shell](https://sw.kovidgoyal.net/kitty/shell-integration/) if needed.
- A Kitty client must be attached to the session. There is no outer terminal to receive notifications while detached.
- On macOS, notification banner behavior is controlled by system settings. An entry in Notification Center or a Dock badge does not guarantee a banner.

