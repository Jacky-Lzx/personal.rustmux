# Troubleshooting

## Nothing appears after startup

Run the development build to surface errors:

```sh
cargo run
```

Then validate the configuration:

```sh
cargo run -- check-config
```

Running without arguments tries to attach to `default`. Use `list-sessions` to inspect existing servers.

## Esc is slow or broken inside an application

Esc can be a standalone key or the first byte of OSC, CSI, and other escape sequences. Rustmux waits for a very short decode window to see whether more bytes arrive, then releases a standalone Esc to the pane. If the problem affects only one application, also check whether it enables the Kitty keyboard protocol.

## Kitty records a notification but shows no banner

This is usually a macOS presentation setting rather than a lost protocol message. In **System Settings → Notifications → kitty**, choose Banners or Alerts and confirm that a Focus mode is not suppressing notifications.

If direct Kitty commands create a Dock badge but Rustmux commands do not, check that:

- the session is currently attached;
- shell integration provides OSC 133;
- the command reaches `command_duration_seconds`;
- its foreground application is not in `exclude_applications`.

## Exiting Yazi still triggers a long-command notification

Keep the default filter:

```toml
[notifications]
exclude_applications = ["yazi", "nvim"]
```

Rustmux inspects foreground processes seen throughout the command, rather than only the outer Fish wrapper.

## Image previews are slow

Use the built-in benchmark to separate Rustmux composition cost from external rasterization:

```sh
cargo bench --features benchmarks --bench image_preview
```

The benchmark excludes Yazi's PDF/SVG/JPEG conversion and Kitty's own image decoding and drawing.

## OSC 52 does not copy

The outer terminal must permit applications to write to the system clipboard. Rustmux sends OSC 52, but terminal security settings may reject it.

## The outer shell retains interface fragments after detach

Rustmux uses the alternate screen and restores outer-terminal state on exit or detach. If this still reproduces, record the outer terminal name and version, a minimal sequence of actions, `rustmux --version`, and relevant terminal settings.

