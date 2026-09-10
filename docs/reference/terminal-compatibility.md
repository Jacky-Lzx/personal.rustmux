# Terminal Compatibility

## Support matrix

| Capability | Protocol | Status |
| --- | --- | --- |
| Extended keyboard input | Kitty keyboard / CSI-u / xterm modified keys | Supported per pane and screen |
| Image previews | Kitty graphics + Unicode placeholders | Supported |
| File drag-and-drop | Kitty OSC 72 | Supported with Kitty 0.47+ |
| Rich clipboard | Kitty OSC 5522 | Supported with per-pane routing |
| File transfer | Kitty OSC 5113 | Supported with per-pane routing |
| Desktop notifications | Kitty OSC 99 | Supported |
| System clipboard | OSC 52 | Supported when allowed by the outer terminal |
| Shell integration | OSC 7 / OSC 133 | Supported |
| Hyperlinks | OSC 8 | Supported across pane redraws |
| Colors | OSC 4, 10/11/12, Kitty OSC 21 | Supported per pane |
| Mouse cursor shape | OSC 22 | Supported per pane |
| Focus tracking | `?1004` | Supported |

## State isolation

A terminal multiplexer must do more than forward bytes: one pane must not overwrite another pane's state. Rustmux stores screen, colors, keyboard modes, mouse modes, images, and protocol requests per pane. When focus changes, only the active pane's state is synchronized to the outer terminal.

Rustmux uses Catppuccin Mocha for its own interface without passing its cursor color into the shell. Cursor colors requested by inner applications remain isolated and are restored per pane.

## Recommended outer terminal

Kitty provides the complete protocol set. Other Unix terminals can run core window, pane, history, and session features, while graphics, drag-and-drop, notifications, and extended keyboard support depend on the protocols implemented by the outer terminal.

