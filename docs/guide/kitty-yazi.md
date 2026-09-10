# Yazi and Kitty

Kitty is the most capable outer terminal for Rustmux. Protocol state is isolated per pane, while requests and responses are routed between the outer terminal and the pane that initiated them.

## Image previews

Rustmux supports the Kitty graphics protocol and Unicode placeholders. Yazi can display image previews inside a pane; Rustmux processes graphics chunks incrementally and redraws only terminal cells that changed.

Yazi's external tools rasterize PDF and SVG files and prepare JPEG previews. Rustmux handles the resulting graphics data and terminal-frame composition.

## File drag-and-drop

Kitty 0.47.0 or newer supports the OSC 72 drag-and-drop protocol. Through Rustmux, Yazi can:

- drag files from a pane into Finder or another GUI application;
- receive files dragged in from outside;
- route protocol responses by pane ID;
- translate outer-terminal coordinates into pane-local cell and pixel coordinates.

## Clipboard and file transfer

Rustmux supports Kitty OSC 5522 clipboard operations and OSC 5113 file transfer. Request IDs are isolated across panes, and outer-terminal responses are routed back to the pane that made the request.

## Keyboard and mouse

Rustmux maintains a Kitty keyboard protocol mode stack for every pane and for both primary and alternate screens. Progressive-enhancement flags are synchronized when focus changes, allowing modifier keys, repeat/release events, and associated text to reach the inner application.

In locked mode, applications receive the mouse protocols they enable. Rustmux retains only its own interactions, including window-label clicks, pane focus, and shared-border dragging.

