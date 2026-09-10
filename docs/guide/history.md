# History, Search, and Copy

## Browse history

Press <kbd>Ctrl-b</kbd>, <kbd>[</kbd> to enter scroll mode. Browse with the keyboard or mouse wheel. Reaching the bottom does not exit; press `q` or Esc to return to the live screen.

Each new pane keeps 1,000 lines by default. Set `scrollback_lines` from 1 to 1,000,000 to change the limit.

## Search

Press `/` in scroll mode, enter a query, and press Enter. Use `n` for the next result and `N` for the previous result.

## Copy text

Rustmux supports two selection styles:

1. Press `v`, extend with arrow keys or `h j k l`, then press `y`.
2. Hold the left mouse button and drag. Releasing copies through OSC 52 and shows a brief confirmation.

Mouse selection is active only in scroll mode. In locked mode, mouse input goes to applications such as Yazi and Neovim.

## Edit history and command output

In normal mode:

- `h` opens complete history with `$VISUAL` or `$EDITOR`;
- `e` opens the previous command's output;
- `y` copies the previous command's output through OSC 52.

Rustmux prefers exact command boundaries from OSC 133 shell integration. Without OSC 133, it falls back to command echo and prompt detection.

