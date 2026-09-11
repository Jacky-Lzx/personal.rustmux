# Session Management

## Command-line management

```sh
# Create or attach to a named session
rustmux -s work

# Reattach
rustmux attach work

# List sessions
rustmux ls

# Stop a session and its processes
rustmux k work

# Stop every running session without prompting
rustmux ka -y
```

The short forms follow Zellij's CLI conventions: `a` aliases `attach`, `ls` aliases `list-sessions`, `k` aliases `kill-session`, and `ka` aliases `kill-all-sessions`. Without `-y` or `--yes`, `kill-all-sessions` asks for confirmation. It stops running sessions but preserves saved snapshots. `rustmux attach -c work` creates the session when it does not exist. The earlier `new-session -s work`, `attach-session -t work`, and `kill-session -t work` forms remain supported.

Press <kbd>Ctrl-b</kbd>, <kbd>d</kbd> inside Rustmux to detach. The server and its programs keep running.

A session accepts one interactive client at a time. If another client is already attached, a second attach exits with a warning and leaves the original client connected, with its mode and terminal size unchanged. Detach the original client before attaching elsewhere. Script control commands remain available while a client is attached.

Rustmux sessions cannot be nested. Starting or attaching to Rustmux from a shell that is already inside Rustmux prints a warning and leaves the current session unchanged. Session management commands such as `list-sessions` and `kill-session` remain available inside a session.

## Session Manager

Press `s` in normal mode to open the centered Session Manager.

- The first session is selected when the manager opens; the search field stays hidden.
- `j` / `k` (or `↓` / `↑`) selects a session.
- `/` enters search mode. Type to filter sessions by name; `j` and `k` enter text in this mode. Use the arrow keys to select a search result.
- `Tab` completes the selected name.
- `Enter` enters the selected session; with no match, it creates a session from the query.
- `Ctrl-a` saves the current layout.
- `Ctrl-r` renames a session.
- `Ctrl-x` disconnects other clients from the selected session.
- `Delete` deletes a session or saved snapshot.
- `Esc` leaves search mode and restores the full list; press it again to close the manager. While renaming, it cancels the rename.

The list includes window and pane counts, connection state, saved state, and creation time.

All manager shortcuts can be customized in `config.toml`. Each action's array replaces its default keys; use `[]` to disable an action. Omitted actions keep their defaults, independently of the top-level `clear_defaults` setting. Keys must not be assigned to multiple manager actions. The displayed hints follow the configured keys.

```toml
[session_manager]
up = ["k", "up"]
down = ["j", "down"]
search = ["/"]
complete = ["tab"]
open = ["enter"]
rename = ["Ctrl r"]
save = ["Ctrl a"]
delete = ["delete"]
disconnect = ["Ctrl x"]
cancel = ["esc"]
backspace = ["backspace"]
```

`open`, `cancel`, and `backspace` also control rename input. While editing a search or name, printable keys assigned to other actions are treated as text. Use modified keys (such as `Ctrl n`) or arrows if you want those shortcuts available while typing.

## Save and restore

Changed layouts are saved automatically every 30 seconds and when detaching or stopping a running server. Set `autosave_interval_seconds = 0` to keep manual saving only. Manual saving remains available through `Ctrl-a` in the Session Manager. Closing every pane retains the last saved snapshot; deleting a session removes its snapshot without recreating it during shutdown.

Snapshots are stored under:

```text
$XDG_STATE_HOME/rustmux/sessions
~/.local/state/rustmux/sessions   # when XDG_STATE_HOME is unset
```

A snapshot contains window names, pane split trees and ratios, the active pane, working directories, and floating-terminal state. Explicit startup commands from project layouts are also saved. Processes and terminal contents are not serialized.

After the server stops, creating the same session again starts fresh shells (or reruns explicit project startup commands) in the saved directories and rebuilds the layout. Saved sessions that are not running remain visible in the Session Manager.

Working directories prefer OSC 7 shell integration and fall back to the foreground process, shell process, and original pane directory. Yazi’s process directory takes precedence over a stale shell directory. Missing directories fall back to the server’s working directory when restoring.

Temporary history editor windows are excluded from saved layouts.
