# 开发与测试

## 日常检查

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

macOS 与 Linux CI 都会运行格式检查、完整测试和 Clippy。

## Fuzz test

nightly Rust 会编译全部 libFuzzer target；Linux CI 还会执行短时 smoke test。

```sh
cargo install cargo-fuzz --locked
cargo fuzz list
cargo fuzz run input-decode
cargo fuzz run kitty-graphics
cargo fuzz run kitty-dnd
cargo fuzz run osc-terminal
cargo fuzz run terminal-render
```

这些 target 覆盖输入与鼠标解码、Kitty graphics、Kitty DnD、OSC/终端响应和完整终端帧渲染。

## 图片预览 benchmark

```sh
cargo bench --features benchmarks --bench image_preview
```

默认把 JPG、PNG、PDF 和 SVG 测试素材归一化为 1 MiB payload，运行 20 次并输出 median、mean 和 MiB/s。

```sh
RUSTMUX_BENCH_PAYLOAD_MIB=4 \
RUSTMUX_BENCH_ITERATIONS=50 \
cargo bench --features benchmarks --bench image_preview
```

测试素材生成在 `target/image-preview-bench-fixtures/`，不会加入 Git。

## 构建本手册

文档使用 mdBook：

```sh
./scripts/build-docs.sh
mdbook serve --open
```

构建脚本会把英文站点写入 `dist/`，并把中文版写入 `dist/zh/`。`mdbook serve` 默认实时预览英文源码。
