# Autosave performance

Measured on 2026-09-12 with an Apple M5 Pro, 64 GiB RAM, macOS 26.6.2, and Rust 1.97.1. Rustmux was built in release mode from commit `6b533a7` plus the benchmark harness. These measurements use synthetic history and an isolated local directory; no real sessions or user configuration were changed.

## Stage measurements

Each pane has a 120-column, 24-row terminal. Histories are full, with one warm-up and 15 measured saves per scenario. Ordinary color uses three style runs per row; dense color changes the indexed foreground color at every character. Dense color is a stress case, not a typical log.

- **Capture** calls the production scrollback extraction code and builds a session snapshot. It excludes live process/working-directory lookup, renderer work, and the event loop, so it is not a complete interactive latency measurement.
- **Unchanged** repeats capture, compares it with the previous snapshot, and drops the redundant snapshot. It performs no disk write.
- **Encode** measures TOML serialization alone.
- **Background** uses the production background writer, including thread startup, TOML encoding, temporary-file writing, permissions, and rename. It excludes capture. It does not include the event loop's completion polling delay.
- **File size** is the resulting TOML snapshot. File writes use the OS cache; the implementation does not call `fsync`, so these timings do not measure durable physical-media flushes.

Times are milliseconds. P50 is the median; with 15 observations, the reported P95 is the largest sample, not a high-confidence tail estimate. Stage medians are measured separately and should not be subtracted to infer exact disk-only cost.

| History | Panes | Rows/pane | Capture P50 / P95 | Unchanged P50 | Encode P50 | Background P50 / P95 | File MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| layout | 1 | 5000 | 0.00 / 0.00 | 0.00 | 0.00 | 0.17 / 0.23 | 0.00 |
| layout | 8 | 5000 | 0.00 / 0.00 | 0.00 | 0.01 | 0.15 / 0.24 | 0.00 |
| plain | 1 | 1000 | 0.61 / 0.68 | 0.61 | 0.20 | 0.43 / 0.51 | 0.12 |
| plain | 1 | 5000 | 3.17 / 3.46 | 3.23 | 1.00 | 1.45 / 1.91 | 0.61 |
| plain | 4 | 5000 | 13.05 / 14.35 | 13.25 | 4.04 | 5.01 / 5.29 | 2.44 |
| plain | 8 | 5000 | 26.68 / 28.98 | 27.47 | 8.42 | 10.47 / 14.67 | 4.88 |
| plain | 1 | 20000 | 12.71 / 13.04 | 13.11 | 4.24 | 5.06 / 5.37 | 2.44 |
| color | 1 | 5000 | 5.77 / 6.18 | 6.04 | 2.09 | 2.63 / 2.73 | 1.01 |
| color | 4 | 5000 | 24.81 / 24.98 | 24.74 | 8.62 | 12.20 / 14.33 | 4.02 |
| color | 8 | 5000 | 49.97 / 52.68 | 50.87 | 18.07 | 20.37 / 22.90 | 8.04 |
| color | 1 | 20000 | 23.86 / 25.14 | 24.28 | 8.73 | 12.38 / 15.14 | 4.02 |
| dense-color | 1 | 5000 | 18.48 / 18.95 | 18.67 | 40.27 | 42.29 / 47.06 | 14.73 |
| dense-color | 4 | 5000 | 75.02 / 77.04 | 76.04 | 166.61 | 175.70 / 189.15 | 58.92 |
| dense-color-as-plain | 4 | 5000 | 14.08 / 17.91 | 14.44 | 4.25 | 5.00 / 5.73 | 2.44 |

## Real-session responsiveness

Seven isolated servers were tested sequentially, each with four windows/panes and 5,000 rows per pane, using a 122-column, 28-row outer terminal. A local Unix-socket status query was sent roughly every 5 ms. These queries share the event loop with input, but this is **server response latency, not end-to-end keyboard-to-screen latency**. No interactive client/rendering load was attached. Probe load and local scheduling influence the numbers.

Static cases had no new output during measurement. Changing cases printed a short line every 200 ms in each pane. All histories were filled before enabling automatic saving; writes observed during setup/shutdown are excluded. Each case used a fresh server and isolated config/state. One run per case was made, so maximum latency is an observed value, not a worst-case guarantee.

| Case | Interval | Observation | Response P50 | Response P99 | Maximum | Queries >16 ms | Writes observed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| disabled | 0 s | 6 s | 0.15 ms | 0.40 ms | 0.70 ms | 0 | 0 |
| layout-1s | 1 s | 6 s | 0.14 ms | 0.38 ms | 0.59 ms | 0 | 1 |
| plain-1s | 1 s | 6 s | 0.13 ms | 0.46 ms | 20.79 ms | 4 | 1 |
| color-1s | 1 s | 6 s | 0.11 ms | 0.42 ms | 31.52 ms | 6 | 1 |
| dense-1s-static | 1 s | 6 s | 0.13 ms | 0.50 ms | 94.68 ms | 6 | 1 |
| dense-1s-changing | 1 s | 6 s | 0.09 ms | 0.56 ms | 89.40 ms | 6 | 6 |
| dense-30s-changing | 30 s | 33 s | 0.15 ms | 0.42 ms | 96.86 ms | 1 | 1 |

Static ordinary-color history produced only one disk write, but six responses exceeded 16 ms in six seconds. The repeated capture/check work remains even when nothing is written. Dense changing output produced six writes and six slow responses at a one-second interval. At a 30-second interval it produced one write and one slow response during 33 seconds, with a similar maximum delay. Longer intervals reduce frequency, not single-save latency.

The dense four-pane snapshot was approximately 59 MiB, versus 4 MiB for ordinary color and 2.4 MiB for plain text. Per-character style changes and escaped ANSI sequences in TOML make the stress case much larger.

Raw results are generated locally at `target/autosave-bench/results.csv` and `target/autosave-bench/live/summary.json`. Per-query samples are retained in `target/autosave-bench/live/*-latency.csv`. These generated files are not versioned; reproduce them with the commands below.

## Interpretation

Background writes remove serialization and filesystem work from the terminal event loop, but history capture remains synchronous. Four panes with 5,000 rows each took about 13 ms to capture as plain text, 25 ms with ordinary color, and 75 ms in the dense-color stress case. Eight ordinarily colored panes took about 50 ms.

Unchanged sessions still incur nearly the same capture cost: the implementation constructs the full snapshot before comparing it with the last saved one. Avoiding the disk write alone does not eliminate the periodic pause.

Intervals change frequency, not the duration of an individual pause. For example, a 25 ms capture every second occupies roughly 2.5% of one event-loop thread's time, compared with 0.083% at a 30-second interval; the individual capture still takes about 25 ms. This is arithmetic from the measured capture cost, not a measured whole-process CPU percentage.

The largest reduction would come from detecting unchanged pane content before copying/extracting history, and moving history formatting off the event loop or capturing it incrementally. Increasing the save interval reduces how often work happens; reducing history capacity or pane count reduces each capture's cost.

## Reproduce

Run stages without other benchmarks competing for CPU:

```sh
mkdir -p target/autosave-bench
XDG_STATE_HOME="$PWD/target/autosave-bench/state" \
  RUSTMUX_BENCH_STATE_ISOLATED=1 RUSTMUX_BENCH_ITERATIONS=15 \
  cargo bench --features benchmarks --bench session_save \
  > target/autosave-bench/results.csv
```

For real-session response probes (requires local PTYs and Unix sockets):

```sh
cargo build --release --locked
python3 scripts/bench-session-save.py
```

The probe uses isolated temporary directories and stops its own servers afterward. It writes raw latency samples and a JSON summary under `target/autosave-bench/live/`.
