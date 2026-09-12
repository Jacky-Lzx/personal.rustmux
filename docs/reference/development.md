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
It runs on pushes to `main` and pull requests. Changes confined to `docs/`,
`README.md`, `demos/`, `book.toml`, the documentation build script, or the Pages
workflow skip Rust CI. Mixed code and documentation changes still run the full
checks, including fuzz jobs. A new CI run cancels an unfinished run for the same
branch or pull request; different pull requests run independently. Documentation
deployment uses a separate concurrency group and is not canceled by CI.

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

## Publish to GitHub Pages

The `Deploy documentation` workflow builds with mdBook 0.5.4 and publishes `dist/`.
It runs when documentation, `book.toml`, the build script, or the workflow changes
on `main`. It can also be started manually from the Actions tab on `main`.

For initial setup:

1. In the repository, open **Settings → Pages** and select **GitHub Actions** as
   the build and deployment source.
2. Push the workflow to `main`.
3. Open **Actions → Deploy documentation** to monitor the run. If needed, choose
   **Run workflow**, select `main`, and start it manually.
4. After both jobs succeed, open the deployment URL or **Settings → Pages → Visit site**.

The default project URL is <https://jacky-lzx.github.io/Rustmux/>. Pages must be
enabled before the workflow runs. No personal access token or custom secret is
required; deployment uses the workflow's `GITHUB_TOKEN` and OIDC permissions.

The workflow obtains the base path from GitHub Pages and overrides mdBook's
`site-url` only during deployment, so local and Sites builds retain their existing
root path. To check a project-path build locally:

```sh
MDBOOK_OUTPUT__HTML__SITE_URL=/Rustmux/ ./scripts/build-docs.sh
```
