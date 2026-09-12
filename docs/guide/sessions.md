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

Session names contain 1–64 ASCII letters, digits, hyphens, or underscores.

Press <kbd>Ctrl-b</kbd>, <kbd>Ctrl-o</kbd>, <kbd>d</kbd> inside Rustmux to detach. The server and its programs keep running.

A session accepts one interactive client at a time. If another client is already attached, a second attach exits with a warning and leaves the original client connected, with its mode and terminal size unchanged. Detach the original client before attaching elsewhere. Script control commands remain available while a client is attached.

Rustmux sessions cannot be nested. Starting or attaching to Rustmux from a shell that is already inside Rustmux prints a warning and leaves the current session unchanged. Session management commands such as `list-sessions` and `kill-session` remain available inside a session.

## Session Manager

Press `Ctrl-w` in normal mode, or `w` in session mode, to open the centered Session Manager.

- The current session stays at the top, followed by other connected sessions, then disconnected sessions. Each group is ordered by last connection time, newest first.
- The most recently connected session that is currently disconnected is selected when the manager opens. If none exists, the current session is selected. The search field stays hidden.
- `j` / `k` (or `↓` / `↑`) selects a session.
- `/` enters search mode. Type to filter sessions by a case-insensitive substring; `j` and `k` enter text in this mode. Use the arrow keys to select a search result.
- `Tab` completes the selected name while searching.
- `Enter` enters the selected session; with no match, it creates a session from the query.
- `Ctrl-a` saves the current layout.
- `Ctrl-r` renames a session.
- `Ctrl-x` disconnects other clients from the selected session.
- `Delete` stops the selected session and its processes and removes its saved snapshot, or removes the snapshot alone if the session is not running. It does not ask for confirmation.
- `Esc` leaves search mode and restores the full list; press it again to close the manager. While renaming, it cancels the rename.

The list includes window and pane counts, connection state, saved state, creation time, and a `LAST CONNECTED` column showing `Now` for the current and attached sessions, or how long ago the last connection occurred for disconnected sessions. Time columns are hidden when space is limited; `—` means no connection time has been recorded.

Connection times are recorded on each successful attachment, independently of automatic layout saving, and survive server restarts. Renaming a session carries its connection time with it; deleting a session removes it. Existing sessions gain a time record on their next connection using the updated server.

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

Automatic saving is disabled by default (`autosave_interval_seconds = 0`). Set a positive interval, such as `30`, to save changed layouts periodically and when detaching or stopping a running server. Manual saving remains available through `Ctrl-a` in the Session Manager. Closing every pane retains the last saved snapshot; deleting a session removes its snapshot without recreating it during shutdown.

Snapshot capture and comparison happen in the terminal event loop; TOML encoding and file writes run on a background thread. One write runs at a time, and pending saves are combined into the latest snapshot. The Session Manager confirms success after the write completes, and `rustmux save-session` waits for that result while terminal input continues. Normal shutdown, session renaming, and deletion wait for outstanding writes to finish so they cannot restore an old filename or recreate a deleted snapshot. Capturing large histories still takes time in the event loop.

Snapshots are stored under:

```text
$XDG_STATE_HOME/rustmux/sessions
~/.local/state/rustmux/sessions   # when XDG_STATE_HOME is unset
```

A snapshot contains window names, pane split trees and ratios, the active pane, working directories, and floating-terminal state. Explicit startup commands from project layouts are also saved. Processes are not serialized. Terminal text is included only when `save_scrollback = true`.

### Save scrollback

```toml
save_scrollback = true
save_scrollback_colors = true # Optional; false saves plain text.
scrollback_lines = 5000
autosave_interval_seconds = 30 # Optional; keep 0 for manual saving only.
```

With `save_scrollback` enabled, manual and automatic saves include the most recent rows from each pane's main terminal buffer, including visible text and floating-terminal history. Up to `scrollback_lines` rows are retained per pane. Full-screen applications' alternate buffers, images, and terminal modes are not saved.

History is plain text by default. Set `save_scrollback_colors = true` to retain indexed and RGB foreground/background colors, bold, dim, italic, underline, and inverse styles. Consecutive cells with the same style share [ANSI SGR sequences](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html). Default and indexed colors use the palette available when restored; custom OSC palette changes are not saved. Color formatting is captured with the snapshot; TOML encoding and writes continue to run in the background.

The color option supports hot reload and affects future saves. Each snapshot records its own format, so existing colored snapshots restore with colors even if the option is subsequently disabled. Old plain-text snapshots remain readable. Restoration accepts only text and SGR styling, and resets styles before starting the fresh live screen.

On restoration, the saved text becomes scrollback above a fresh live screen. Enter scroll mode to browse, search, or copy it. Restoring at a narrower width may wrap lines and reduce how much history fits within the current limit. Applications start anew; saved output is not executed.

This option supports hot reload and does not enable automatic saving by itself. Disabling it omits history from future snapshots and skips history when restoring an existing snapshot. Previously saved text remains on disk until that snapshot is overwritten or deleted. Older layout-only snapshots remain compatible.

After the server stops, creating the same session again starts fresh shells (or reruns explicit project startup commands) in the saved directories and rebuilds the layout. Saved sessions that are not running remain visible in the Session Manager.

Working directories prefer [OSC 7](https://sw.kovidgoyal.net/kitty/shell-integration/#notes-for-shell-developers) shell integration and fall back to the foreground process, shell process, and original pane directory. Yazi’s process directory takes precedence over a stale shell directory. Missing directories fall back to the server’s working directory when restoring.

Temporary history editor windows are excluded from saved layouts.
