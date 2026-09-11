# Keybindings and Modes

Rustmux uses a mode system inspired by Zellij. The default prefix is <kbd>Ctrl-b</kbd>: press it in `locked` mode to enter `normal` mode. Most actions automatically return to `locked` mode when complete.

## Normal mode

| Key | Action |
| --- | --- |
| `c` | Create a window |
| `,` | Rename the current window; Enter confirms, Esc cancels |
| `n` / `p` | Next / previous window |
| `<` / `>` | Move the current window left / right |
| `1` … `9` | Jump to a numbered window |
| `s` | Open the Session Manager |
| `[` / `Enter` | Enter scroll mode |
| `h` | Open complete history in an editor |
| `e` | Edit the previous command's output |
| `y` | Copy the previous command's output |
| `i` | Show or hide the floating terminal |
| `Ctrl-p` | Enter pane mode |
| `&` / `x` | Close the current window |
| `d` | Detach |
| `?` | Open help for the current mode |

Press <kbd>Ctrl-b</kbd> again in normal mode to send the prefix to the application.

## Pane mode

| Key | Action |
| --- | --- |
| `r` / `n` | Create a pane to the right |
| `d` | Create a pane below |
| `h j k l` / arrow keys | Focus in a direction |
| `Tab` | Cycle focus |
| `H J K L` | Grow the pane in a direction |
| `Alt-h/j/k/l` | Swap with the nearest pane in a direction |
| `z` | Toggle pane zoom |
| `x` | Close the pane |
| `q` / `Esc` | Return to locked mode |

## Scroll mode

| Key | Action |
| --- | --- |
| `k` / `↑`, `j` / `↓` | Scroll one line |
| `u` / `PageUp`, `d` / `PageDown` | Scroll one page |
| `g` / `G` | Jump to the oldest entry / bottom |
| `/` | Search history |
| `n` / `N` | Next / previous match |
| `v` | Start or end keyboard selection |
| `h j k l` / arrow keys | Extend the selection |
| `y` | Copy the selection and exit |
| `q` / `Esc` | Return to locked mode |

Reaching the bottom with the mouse wheel does not leave scroll mode. This prevents a final scroll event from unexpectedly sending later input to a running application.

## Help overlay

Press `?` to show every visible binding for the current mode. Pressing a listed shortcut closes the overlay and runs its action immediately. Esc closes the overlay without running an action.


## Clickable bottom shortcuts

Left-click a shortcut key or its action label in the bottom bar to execute its configured action sequence, including any mode change. Hovering a clickable hint shows a pointer cursor when the outer terminal supports cursor shapes. No additional configuration is required.

- In grouped hints such as `n/p`, click `n` or `p` to choose that key. Clicking the action label, padding, or separator executes the first displayed key.
- Window-number shortcuts are displayed individually (`1/2/3`) so each number can be clicked.
- Click `MORE` or the alias ellipsis to open contextual help for additional bindings.
- The Session Manager's bottom `Esc` and `CLOSE` hints close the manager.

Only visible hints have click targets. Mode labels, blank space, and rename/search input prompts do not execute actions. Dragging or releasing after a bottom-bar press does not execute the action again or send the mouse event to an application. With `compact = true`, the bottom bar is hidden and has no click targets.
