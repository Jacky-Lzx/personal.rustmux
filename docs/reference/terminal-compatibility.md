# Terminal Compatibility

## Support matrix

Protocol names below link to their specifications. For OSC (Operating System Command), CSI, and other escape-sequence conventions, see the [xterm control-sequence reference](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html).

| Capability | Protocol | Status |
| --- | --- | --- |
| Extended keyboard input | [Kitty keyboard](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) / [CSI-u](https://www.leonerd.org.uk/hacks/fixterms/) / [xterm modified keys](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html) | Supported per pane and screen |
| Image previews | [Kitty graphics](https://sw.kovidgoyal.net/kitty/graphics-protocol/) + [Unicode placeholders](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders) | Supported |
| File drag-and-drop | [Kitty OSC 72](https://sw.kovidgoyal.net/kitty/dnd-protocol/) | Supported with Kitty 0.47+ |
| Rich clipboard | [Kitty OSC 5522](https://sw.kovidgoyal.net/kitty/clipboard/) | Supported with per-pane routing |
| File transfer | [Kitty OSC 5113](https://sw.kovidgoyal.net/kitty/file-transfer-protocol/) | Supported with per-pane routing |
| Desktop notifications | [Kitty OSC 99](https://sw.kovidgoyal.net/kitty/desktop-notifications/) | Supported |
| System clipboard | [OSC 52](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html) | Supported when allowed by the outer terminal |
| Shell integration | [OSC 7](https://sw.kovidgoyal.net/kitty/shell-integration/#notes-for-shell-developers) / [OSC 133](https://iterm2.com/documentation-escape-codes.html) | Supported |
| Hyperlinks | [OSC 8](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda) | Supported across pane redraws |
| Colors | [OSC 4, 10/11/12](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html), [Kitty OSC 21](https://sw.kovidgoyal.net/kitty/color-stack/#setting-and-querying-colors) | Supported per pane |
| Mouse cursor shape | [OSC 22](https://sw.kovidgoyal.net/kitty/pointer-shapes/) | Supported per pane |
| Focus tracking | [`?1004`](https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html) | Supported |

## State isolation

A terminal multiplexer must do more than forward bytes: one pane must not overwrite another pane's state. Rustmux stores screen, colors, keyboard modes, mouse modes, images, and protocol requests per pane. When focus changes, only the active pane's state is synchronized to the outer terminal.

Rustmux uses [Catppuccin Mocha](https://catppuccin.com/palette/) for its own interface without passing its cursor color into the shell. Cursor colors requested by inner applications remain isolated and are restored per pane.

## Recommended outer terminal

Kitty provides the complete protocol set. Other Unix terminals can run core window, pane, history, and session features, while graphics, drag-and-drop, notifications, and extended keyboard support depend on the protocols implemented by the outer terminal.

