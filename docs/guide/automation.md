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
