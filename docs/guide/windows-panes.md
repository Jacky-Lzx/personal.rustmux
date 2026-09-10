# Windows and Panes

## Create and rename windows

Press `c` in normal mode to create a window. Press `,` to rename it: the top label updates while you type. Enter saves the new name; Esc restores the original.

Click any window label to switch directly. A `[!]` on the right means a pane in that window has emitted a bell that you have not viewed yet.

## Split panes

Press <kbd>Ctrl-b</kbd>, <kbd>Ctrl-p</kbd> to enter pane mode:

```text
r / n   split to the right
d       split below
z       zoom or restore the current pane
x       close the current pane
```

New panes inherit the working directory reported by the active pane through OSC 7. Before the shell reports a directory, Rustmux uses the session server's startup directory.

## Focus and resize

Use `h j k l` or the arrow keys to move focus. Uppercase `H J K L` moves the current pane boundary. Add `Alt` to the lowercase directional keys to swap the pane with its nearest neighbor while preserving split ratios.

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

