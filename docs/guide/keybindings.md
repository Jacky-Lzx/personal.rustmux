# Keybindings and Modes

Press <kbd>Ctrl-b</kbd> in `locked` mode to enter `normal` mode. Window creation, splits, and many other one-shot actions return to `locked`; navigation within pane, tab, resize, and move modes stays in that mode for repeated actions.

## Normal mode

| Key | Action |
| --- | --- |
| `c` / `x` | Create / close a window |
| `,` | Rename the window; Enter confirms, Esc cancels |
| `n` / `p` or `Tab` | Next / previous window |
| `<` / `>` | Move the window left / right |
| `1` … `9` | Jump to a numbered window |
| `h j k l` | Focus a pane, then return to locked mode |
| Arrow keys | Focus a pane and stay in normal mode |
| `i` | Toggle the floating terminal |
| `Ctrl-w` | Open the Session Manager |
| `Enter` / `s` | Enter scroll mode |
| `Ctrl-p` | Enter pane mode |
| `Ctrl-t` | Enter tab mode |
| `Ctrl-m` | Enter move mode |
| `Ctrl-o` | Enter session mode |
| `r` | Enter resize mode |
| `?` | Show normal-mode help, then return to locked mode |
| `Esc` / `Ctrl-g` | Return to locked mode |

## Pane mode

| Key | Action |
| --- | --- |
| `r` / `n`, `d` | Split right / below, then return to locked mode |
| `h j k l` / arrow keys | Focus a pane and stay in pane mode |
| `Tab` | Cycle focus |
| `f` | Toggle pane zoom and return to locked mode |
| `w` | Toggle the floating terminal and return to locked mode |
| `b` | Move the pane into a new window |
| `[` / `]` | Move the pane into the previous / next existing window |
| `x` | Close the pane and return to locked mode |
| `p` | Return to normal mode |

Moving a pane between windows preserves its running process and returns to locked mode. `[` and `]` wrap at the ends of the window list.

## Tab mode

| Key | Action |
| --- | --- |
| `h` / `Left` / `Tab` | Previous window |
| `l` / `Right` | Next window |
| `<` / `>` | Move the window left / right |
| `1` … `9` | Jump to a numbered window |
| `n` / `x` | Create / close a window and return to locked mode |
| `r` | Rename the window |

## Resize and move modes

Use `h j k l` or arrow keys repeatedly to resize in `resize` mode or swap panes in `move` mode. Press `r` from resize mode, or `m` from move mode, to return to normal mode.

## Scroll mode

| Key | Action |
| --- | --- |
| `k` / `Up`, `j` / `Down` | Scroll one line |
| `Ctrl-b` / `Ctrl-u` | Scroll one page up |
| `Ctrl-f` / `Ctrl-d` | Scroll one page down |
| `g` / `G` | Jump to the oldest entry / bottom |
| `/` | Search history |
| `n` / `N` | Next / previous match |
| `E` | Open complete history in an editor |
| `e` | Edit the previous command's output |
| `y` | Copy the previous command's output |
| `q` / `Esc` | Scroll to the bottom and return to locked mode |

`E`, `e`, and `y` return to the live screen and locked mode. Drag with the mouse to select and copy history text. Keyboard selection actions remain configurable but are not bound by default.

Reaching the bottom with the mouse wheel does not leave scroll mode.

## Session mode

| Key | Action |
| --- | --- |
| `d` | Detach |
| `w` | Open the Session Manager and return to locked mode |
| `o` | Return to normal mode |

In pane, tab, resize, move, scroll, and session modes, `Enter`, `Esc`, or `Ctrl-g` returns to locked mode, and `?` opens contextual help. The help overlay shows the available shortcuts for switching directly between modes.

## Help overlay

Press `?` to show every visible binding for the current mode. Pressing a listed shortcut closes the overlay and runs its action immediately. Esc closes the overlay without running an action.


## Clickable bottom shortcuts

Left-click a shortcut key or its action label in the bottom bar to execute its configured action sequence, including any mode change. Set `mouse_hover_cursor = true` to show a pointer cursor over clickable hints when the outer terminal supports cursor shapes; this is disabled by default.

- In grouped hints such as `n/p`, click `n` or `p` to choose that key. Clicking the action label, padding, or separator executes the first displayed key.
- Window-number shortcuts are hidden by default. If configured with `display = "always"`, each displayed number can be clicked.
- Click `MORE` or the alias ellipsis to open contextual help for additional bindings.
- The Session Manager's bottom `Esc` and `CLOSE` hints close the manager.

Only visible hints have click targets. Mode labels, blank space, and rename/search input prompts do not execute actions. Dragging or releasing after a bottom-bar press does not execute the action again or send the mouse event to an application. With `compact = true`, the bottom bar is hidden and has no click targets.
