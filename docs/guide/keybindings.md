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

