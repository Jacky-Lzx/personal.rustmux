# Core Concepts

## Session

A session is a long-lived workspace owned by a background server. Closing or detaching a client does not stop its shells. Each named session contains one or more windows.

## Window

A window behaves like a tab. The top status line shows the active session and every window. The active window uses a green label; background windows use white labels. A background window displays `[!]` on the right when one of its panes has an unread bell.

Click a window label to switch directly. In `normal` mode, use `n`, `p`, or number keys to navigate, and `<` or `>` to reorder windows.

## Pane

A pane is a region with its own PTY, screen buffer, and terminal state. Rustmux stores the following independently for every pane:

- primary and alternate screen contents;
- Kitty keyboard protocol mode stacks;
- default foreground, background, and application-requested cursor colors;
- mouse protocol, cursor shape, and focus tracking;
- Kitty graphics, drag-and-drop, and IPC request state.

The focused pane has a green border. A pane with an unread bell has an orange border. Click a pane or its border to focus it, and drag a shared border to resize adjacent panes.

## Mode

Modes provide distinct input contexts:

- `locked`: almost all input reaches the application inside the pane.
- `normal`: window, session, history, and floating-terminal actions.
- `pane`: split, focus, resize, move, zoom, or close panes.
- `scroll`: browse, search, select, and copy terminal history.

The bottom status line only shows frequently used bindings for the current mode. When space runs out, Rustmux displays `? MORE (+N)`; press `?` for the complete help overlay.

## Floating terminal

The floating terminal is an independent PTY shown in a centered overlay. Press `i` in normal mode to show or hide it. When opened, it receives focus and the pane below switches to its unfocused border style.

