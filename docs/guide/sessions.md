# Session Management

## Command-line management

```sh
# Create or attach to a named session
rustmux new-session -s work

# Reattach
rustmux attach-session -t work

# List sessions
rustmux list-sessions

# Stop a session and its processes
rustmux kill-session -t work
```

Press <kbd>Ctrl-b</kbd>, <kbd>d</kbd> inside Rustmux to detach. The server and its programs keep running.

## Session Manager

Press `s` in normal mode to open the centered Session Manager.

- Type to filter sessions by name.
- `↑` / `↓` selects a result.
- `Tab` completes the selected name.
- `Enter` enters the selected session; with no match, it creates a session from the query.
- `Ctrl-a` saves the current layout.
- `Ctrl-r` renames a session.
- `Ctrl-x` disconnects other clients from the selected session.
- `Delete` deletes a session or saved snapshot.
- `Esc` closes the manager.

The list includes window and pane counts, connection state, saved state, and creation time.

## Save and restore

Snapshots are stored under:

```text
$XDG_STATE_HOME/rustmux/sessions
~/.local/state/rustmux/sessions   # when XDG_STATE_HOME is unset
```

A snapshot contains window names, pane split trees and ratios, the active pane, working directories, and floating-terminal state. Processes and terminal contents are not serialized.

After the server stops, creating the same session again starts fresh shells in the saved directories and rebuilds the layout. Saved sessions that are not running remain visible in the Session Manager.

