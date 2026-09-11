# Windows and Panes

## Create and rename windows

Press `c` in normal mode to create a window. Press `,` to rename it: the top label updates while you type. Enter saves the new name; Esc restores the original.

Click any window label to switch directly. A `[!]` on the right means a pane in that window has emitted a bell that you have not viewed yet.

## Split panes

Press <kbd>Ctrl-b</kbd>, <kbd>Ctrl-p</kbd> to enter pane mode:

```text
r / n   split to the right
d       split below
f       zoom or restore the current pane
x       close the current pane
```

New windows, splits, and floating terminals inherit the active pane's working directory. Rustmux prefers a valid OSC 7 directory, then falls back to the foreground process, shell process, and original pane directory. When Yazi is the foreground application, its process directory takes precedence over a stale OSC 7 directory. If no valid directory is available, the session server's working directory is used.

## Focus and resize

In pane mode, use `h j k l` or the arrow keys repeatedly to move focus. From normal mode, press `r` to enter resize mode or `Ctrl-m` to enter move mode, then use the same directional keys to resize or swap panes. Press Esc to return to locked mode.

In pane mode, `b` moves the pane into a new window; `[` and `]` move it into the previous or next existing window, wrapping at the ends. These actions preserve the running process and return to locked mode.

Mouse controls are also available:

- click pane content or a border to focus it;
- drag a shared border to resize adjacent panes continuously;
- when an inner application enables a mouse protocol, other mouse events pass through in locked mode.

## Bell state

A BEL from a pane—or completion of a command that reaches the notification threshold—creates unread state:

- `[!]` appears beside the pane title;
- the pane border turns orange;
- `[!]` appears beside the window label.

Focusing the pane that emitted the bell clears these indicators.

