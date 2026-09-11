# Development and Testing

## Routine checks

```sh
cargo fmt --all --check
cargo fmt --manifest-path fuzz/Cargo.toml --all --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo check --features benchmarks --bench image_preview --locked
```

CI runs formatting, the complete test suite, and Clippy on macOS and Linux.

## Fuzz testing

[Nightly Rust](https://rust-lang.github.io/rustup/concepts/channels.html) compiles every [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) target. Linux CI also runs short smoke tests.

```sh
cargo install cargo-fuzz --locked
cargo fuzz list
cargo +nightly fuzz run input-decode
cargo +nightly fuzz run kitty-graphics
cargo +nightly fuzz run kitty-dnd
cargo +nightly fuzz run osc-terminal
cargo +nightly fuzz run terminal-render
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

The documentation uses [mdBook](https://rust-lang.github.io/mdBook/):

```sh
./scripts/build-docs.sh
mdbook serve --open
```

The build script generates the documentation in `dist/`. `mdbook serve` opens it with live reload.
