# Script Control

Control a running session without attaching a terminal. Every command accepts `-s SESSION` (default: `default`). Pane commands accept `-p ID`; omission selects the active pane. IDs remain stable while that server is running; enumerate them again after restoring a session.

```sh
rustmux list-panes -s work
rustmux list-panes -s work --toml
rustmux new-window -s work --name logs
rustmux split-pane -s work -p 1 --down
rustmux send-keys -s work -p 1 --literal --enter 'printf "hello\n"'
rustmux send-keys -s work -p 1 'Ctrl c'
rustmux capture-pane -s work -p 1 --history
rustmux save-session -s work
```

`new-window` and `split-pane` print the new pane ID. New windows and splits receive focus. Sending input and capturing output do not change focus. Splits go right by default; `--down` splits below. Too-small panes and invalid IDs return a nonzero exit status.

`list-panes` prints tab-separated pane ID, one-based window number, title, and directory. `--toml` adds window names, focus, and floating state in a machine-readable `panes` array.

`send-keys` accepts the same named keys as configuration bindings. Use `--literal` for text, and `--enter` to submit it. A request is limited to 4096 input bytes. `capture-pane` returns plain text, using the visible screen by default or full scrollback with `--history`.

Control messages use a separate, bounded request/response channel on the existing private session socket. Errors are returned to the calling command without stopping the server. Captures are limited to 16 MiB including response metadata.

## Project layouts

Create a session in the background, optionally using a project layout:

```sh
rustmux new-session work --detached
rustmux new-session project --detached --layout ./rustmux-project.toml
rustmux attach project
```

A detached session starts at 120 columns by 40 rows and resizes on attach. Creating an already running session without `--layout` succeeds without changing it. Applying a layout to a running session is rejected. An explicit layout takes precedence over the saved snapshot for that name.

```toml
[[windows]]
name = "development"

[[windows.panes]]
cwd = "."
command = "cargo watch -x check"

[[windows.panes]]
cwd = "."
split = "down"

[[windows]]
name = "editor"

[[windows.panes]]
cwd = "."
command = "nvim"
```

Directories are relative to the layout file and must exist. Each pane after the first splits the preceding pane; `split` is `right` by default or `down`. Layouts support at most 128 panes. Unknown fields, empty windows, and invalid directories fail before launching the server.

Commands execute through the configured shell with `-c`. A pane closes when its command exits; append `; exec /bin/sh` when a command should leave an interactive shell behind. Panes without commands open the configured interactive shell. Explicit startup commands are included in saved snapshots and rerun when restoring the session. Existing process memory and terminal contents are not restored.

## Move panes between windows

```sh
# Move pane 1 below pane 2; remove the source window if it becomes empty.
rustmux join-pane -s work -p 1 --to-pane 2 --down

# Give pane 1 its own window.
rustmux break-pane -s work -p 1 --name editor
```

Both commands preserve the pane ID, running process, terminal contents, and working directory, and focus the moved pane. Floating panes and temporary history editor panes cannot be moved this way. Invalid destinations leave the layout unchanged.

The corresponding interactive actions are `break-pane`, `move-pane-next-window`, and `move-pane-previous-window`. The latter two split the first pane in the adjacent window to the right, wrapping around the window list. They have no default keys; configure them as needed:

```toml
[keybinds.pane]
"!" = ["break-pane"]
">" = ["move-pane-next-window"]
"<" = ["move-pane-previous-window"]
```
