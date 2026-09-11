# History, Search, and Copy

## Browse history

Press <kbd>Ctrl-b</kbd>, <kbd>Enter</kbd> to enter scroll mode. Browse with the keyboard or mouse wheel. Reaching the bottom does not exit; press `q` or Esc to return to the live screen.

Each new pane keeps 5,000 lines by default. Set `scrollback_lines` from 1 to 1,000,000 to change the limit.

Set `save_scrollback = true` to include history in manual and automatic session saves. Add `save_scrollback_colors = true` to preserve colors and text styles; otherwise history is saved as plain text. Restored history remains available for scrolling, searching, and copying above the new shell. See [Save scrollback](sessions.md#save-scrollback) for details; both options are disabled by default.

## Search

Press `/` in scroll mode, enter a query, and press Enter. Use `n` for the next result and `N` for the previous result.

## Copy text

Hold the left mouse button and drag in scroll mode. Releasing copies through [OSC 52](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html) and shows a brief confirmation.

Keyboard selection is available through custom bindings for `toggle-history-selection`, `selection-left/right`, `scroll-up/down`, and `copy-selection`; these selection shortcuts are not assigned by default.

Mouse selection is active only in scroll mode. In locked mode, mouse input goes to applications such as Yazi and Neovim.

## Edit history and command output

In scroll mode:

- `E` opens complete history with `$VISUAL`, then `$EDITOR`, falling back to `vi`;
- `e` opens the previous command's output;
- `y` copies the previous command's output through OSC 52.

These actions return to the live screen and locked mode.

Rustmux prefers exact command boundaries from [OSC 133](https://iterm2.com/documentation-escape-codes.html) shell integration. Without OSC 133, it falls back to command echo and prompt detection.
