# Development and Testing

## Routine checks

```sh
cargo fmt --all --check
cargo fmt --manifest-path fuzz/Cargo.toml --all --check
cargo test --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo check --features benchmarks --bench image_preview --locked
```

CI runs formatting, the complete test suite, and Clippy on macOS and Linux.
It runs on pushes to `main` and pull requests. Changes confined to `docs/`,
`README.md`, `CONTRIBUTING.md`, `demos/`, `book.toml`, the documentation build scripts, or the Pages
workflow skip Rust CI. Mixed code and documentation changes still run the full
checks, including fuzz jobs. A new CI run cancels an unfinished run for the same
branch or pull request; different pull requests run independently. Documentation
deployment uses a separate concurrency group and is not canceled by CI.

To run CI manually, open **Actions → CI → Run workflow**, select the branch,
and click **Run workflow**. Manual runs execute all check and fuzz jobs regardless
of changed paths. The selected branch must contain the manual trigger; the
workflow must also be present on the default branch for the button to appear.

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

Use mdBook 0.5.4 and Python 3.12 or newer. The unified build keeps `main` at the
site root and places `main-human` under `main-human/`, with a branch selector.
Each branch has its own content, search index and edit links. Shared presentation
assets come from `main`.

From the `main` checkout, build with the local `main-human` Git ref:

```sh
./scripts/build-docs.sh
python3 -m http.server 8000 --directory dist
```

For an unmerged documentation candidate or local human edits, pass its checkout:

```sh
./scripts/build-docs.sh --human-source ../Rustmux-human
```

Alternatively, use `--human-ref codex/human-docs` to build a committed candidate.
Use `--output /tmp/rustmux-docs-preview` for an isolated preview directory if
a separate `mdbook serve` process is rebuilding `dist/`. The script does not fetch Git refs; refresh the reference yourself when needed.
It fails if human documentation is absent and leaves the previous artifact in
place when either book fails to build. For a single-book editing preview,
`mdbook serve --open` remains available, without the branch selector.

The selector opens the same chapter on the other branch when available. If the
chapter is absent, it opens that branch's home page with a notice. Search remains
within the selected branch. The shared feature acceptance ledger stays on `main`.

## Publish to GitHub Pages

The `Deploy documentation` workflow on both branches checks out `main` and
`main-human`, builds both books, and deploys one combined artifact. A documentation
push to either branch triggers the workflow. All runs share one concurrency group;
neither branch deploys a partial site that could remove the other branch's pages.
Both source branch tips are resolved at checkout time, including on a manual run.

For the first combined publication, merge the human documentation candidate into
`main-human` and publish both branches' documentation changes before running the
workflow. The workflow requires `book.toml` and `docs/` on both remote branches.
Keep the small orchestration workflow identical on both branches; the build code
and shared theme are maintained only on `main`.

In repository **Settings → Pages**, choose **GitHub Actions** as the source.
Then use **Actions → Deploy documentation → Run workflow** on `main` (or
`main-human`) to publish manually. Pages settings are separate from these files;
local builds do not deploy or change repository settings.

The default project URLs are:

- `https://jacky-lzx.github.io/Rustmux/` for `main`;
- `https://jacky-lzx.github.io/Rustmux/main-human/` for `main-human`.

The workflow sets the Pages base path for both books. To validate the same
subdirectory layout locally:

```sh
MDBOOK_OUTPUT__HTML__SITE_URL=/Rustmux/ ./scripts/build-docs.sh
```

Serve that artifact at `/Rustmux/` when previewing this configuration. Both local
root builds and project-path builds retain the branch-specific navigation,
assets, search and edit URLs. No additional hosting provider or token is needed.
