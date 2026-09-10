# Development and Testing

## Routine checks

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

CI runs formatting, the complete test suite, and Clippy on macOS and Linux.

## Fuzz testing

Nightly Rust compiles every libFuzzer target. Linux CI also runs short smoke tests.

```sh
cargo install cargo-fuzz --locked
cargo fuzz list
cargo fuzz run input-decode
cargo fuzz run kitty-graphics
cargo fuzz run kitty-dnd
cargo fuzz run osc-terminal
cargo fuzz run terminal-render
```

The targets cover keyboard and mouse decoding, Kitty graphics, Kitty drag-and-drop, OSC and terminal responses, and complete terminal-frame rendering.

## Image-preview benchmark

```sh
cargo bench --features benchmarks --bench image_preview
```

The default run normalizes JPG, PNG, PDF, and SVG fixtures to a 1 MiB payload, performs 20 iterations, and reports median, mean, and MiB/s.

```sh
RUSTMUX_BENCH_PAYLOAD_MIB=4 \
RUSTMUX_BENCH_ITERATIONS=50 \
cargo bench --features benchmarks --bench image_preview
```

Fixtures are generated under `target/image-preview-bench-fixtures/` and are not committed.

## Build this manual

The documentation uses mdBook:

```sh
./scripts/build-docs.sh
mdbook serve --open
```

The build script generates the documentation in `dist/`. `mdbook serve` opens it with live reload.
