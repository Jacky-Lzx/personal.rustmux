# Rustmux demo recordings

The README animations are generated from versioned [VHS](https://github.com/charmbracelet/vhs) tapes.

## Record every keyboard-driven demo

```sh
brew install go ttyd ffmpeg
./scripts/install-demo-tools.sh
./scripts/record-demos.sh
```

Record one tape while iterating:

```sh
./scripts/record-demos.sh demos/tapes/windows-and-panes.tape
```

The installer keeps a pinned VHS binary under the ignored `target/demo-tools/` directory. The recording script builds Rustmux in release mode, uses an isolated Rustmux/Fish configuration, disables Fish autosuggestions in both the VHS launcher shell and shells inside Rustmux, removes stale demo sessions before recording, verifies every output file, and cleans up its sessions on exit.

VHS 0.12.0 currently cancels its render context before starting ffmpeg on macOS, then suppresses the encoding error. The pinned 0.11.0 release avoids that regression and prevents a recording run from reporting success without producing a file.

VHS does not render Kitty graphics and cannot drive arbitrary mouse dragging. Record image previews, drag-and-drop, window-label clicks, and pane-border dragging in a real Kitty window instead of adding them to these tapes.
