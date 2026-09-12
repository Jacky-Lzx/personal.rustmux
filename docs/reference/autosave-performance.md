# Autosave performance

## Incremental history capture

Measured locally on 2026-09-12 with macOS 26.6.2 and Rust 1.97.1, using a release build based on `1213a17` plus the incremental-save changes. Synthetic histories and isolated state directories were used. No real sessions or user configuration were changed.

The event loop now copies a terminal screen only when that pane has changed. Unchanged panes reuse an immutable capture by reference. The save worker extracts rows, formats text and colors, and caches those results before encoding and writing the snapshot. Layout metadata, directories, focus, and floating state are still collected at each save check.

New terminal output and pane size changes invalidate a capture. The history limit and color option are part of its cache key. History browsing does not invalidate it. Output invalidation is conservative, including control sequences that may leave saved text unchanged. The on-disk format remains a complete TOML snapshot, not a delta log.

## Stage measurements

Each pane has a 120-column, 24-row terminal. Histories are full, with one warm-up and 15 measured saves per scenario. Ordinary color uses three style runs per row; dense color changes the indexed foreground color at every character. Dense color is a stress case, not a typical log.

- **Previous capture** runs the original synchronous screen-copy and history-formatting path in the same benchmark iteration, before the new path. The resulting snapshot is checked against the worker's output for equality.
- **All changed** builds layout metadata and captures changed terminal screens using the production cache. Every pane receives output before this measurement.
- **One changed** repeats preparation with only the first pane changed, reusing all other panes.
- **Unchanged** repeats preparation and compares capture identities and layout metadata without extracting or comparing history text.
- **Background** includes thread startup, history extraction and formatting, snapshot assembly, TOML encoding, temporary-file writing, permissions, and rename. It excludes event-loop capture and completion polling. Unlike the original benchmark's background stage, it now includes history formatting.

These stages exclude live process/directory lookup, renderer work, and interactive client load. They are not end-to-end keyboard-to-screen latency. Writes use the OS cache without `fsync`; timings do not measure durable physical-media flushes. Cached history and frozen screens require additional transient memory. Screens are released by the worker after formatting, while reusable formatted history remains cached.

Times below are median milliseconds. With 15 observations, P95 is the largest sample, not a high-confidence tail estimate. Stages are measured independently; subtracting medians does not isolate disk cost.

| History | Panes | Rows/pane | Previous capture | All changed | One changed | Unchanged | Background |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| layout | 1 | 5000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.136 |
| layout | 8 | 5000 | 0.000 | 0.000 | 0.000 | 0.001 | 0.175 |
| plain | 1 | 1000 | 0.593 | 0.102 | 0.111 | 0.000 | 0.951 |
| plain | 1 | 5000 | 3.078 | 0.532 | 0.579 | 0.001 | 4.113 |
| plain | 4 | 5000 | 13.769 | 2.376 | 0.737 | 0.002 | 15.379 |
| plain | 8 | 5000 | 25.351 | 4.351 | 0.635 | 0.004 | 30.529 |
| plain | 1 | 20000 | 12.397 | 2.298 | 2.133 | 0.002 | 15.505 |
| color | 1 | 5000 | 6.476 | 0.747 | 0.720 | 0.001 | 8.517 |
| color | 4 | 5000 | 25.205 | 2.271 | 0.672 | 0.002 | 35.014 |
| color | 8 | 5000 | 51.679 | 4.586 | 0.832 | 0.003 | 64.506 |
| color | 1 | 20000 | 24.300 | 2.464 | 2.403 | 0.002 | 32.500 |
| dense-color | 1 | 5000 | 18.487 | 0.848 | 0.721 | 0.001 | 60.327 |
| dense-color | 4 | 5000 | 74.204 | 2.840 | 0.723 | 0.002 | 250.735 |
| dense-color-as-plain | 4 | 5000 | 13.185 | 2.508 | 0.734 | 0.002 | 15.524 |

Four ordinarily colored panes dropped from 25.205 ms of synchronous capture/formatting to 2.271 ms of event-loop preparation (about 91% less). Eight dropped from 51.679 ms to 4.586 ms. Four dense-color panes dropped from 74.204 ms to 2.840 ms. An unchanged four-pane ordinary-color check took 0.002 ms in this stage benchmark, while changing only one pane took 0.672 ms.

The remaining synchronous work still grows with changed pane count and history size: a one-pane, 20,000-row color capture took 2.464 ms. This is a measured improvement, not a fixed latency bound. Formatting and full-file encoding still cost CPU on the worker, and the dense four-pane snapshot remains about 59 MiB. Reducing autosave frequency can still reduce total work when output changes continuously.

## Real-session responsiveness

Seven isolated servers were tested sequentially, each with four panes and 5,000 rows per pane, at a 122-column, 28-row outer size. A Unix-socket status query was sent roughly every 5 ms. This measures server response latency, not keyboard-to-screen latency; no interactive client/rendering load was attached. Static cases received no new output, while changing cases printed a line every 200 ms per pane. Each case was run once, so maxima are observations, not worst-case bounds.

| Case | Interval | Observation | Response P50 | Response P99 | Maximum | Queries >16 ms | Writes observed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| disabled | 0 s | 6 s | 0.14 ms | 0.35 ms | 0.51 ms | 0 | 0 |
| layout-1s | 1 s | 6 s | 0.16 ms | 0.41 ms | 1.01 ms | 0 | 1 |
| plain-1s | 1 s | 6 s | 0.10 ms | 0.31 ms | 7.11 ms | 0 | 1 |
| color-1s | 1 s | 6 s | 0.16 ms | 0.41 ms | 10.31 ms | 0 | 1 |
| dense-1s-static | 1 s | 6 s | 0.09 ms | 0.30 ms | 2.36 ms | 0 | 1 |
| dense-1s-changing | 1 s | 6 s | 0.09 ms | 0.25 ms | 7.81 ms | 0 | 6 |
| dense-30s-changing | 30 s | 33 s | 0.11 ms | 0.32 ms | 12.34 ms | 0 | 1 |

No status query exceeded 16 ms in these runs. Static histories produced only one write, while dense changing output still produced six writes with a one-second interval. The previous implementation's report recorded 31.52 ms maximum latency for static ordinary color and 89.40 ms for dense changing output at a one-second interval; the corresponding observations here were 10.31 ms and 7.81 ms. Those earlier runs were separate measurements, not a controlled paired experiment.

Cache reuse removes repeated history scans on static saves. Changed-screen copying, process/directory lookup, allocation, scheduling, and renderer work can still delay the event loop. These results do not establish a universal latency limit or measure whole-process CPU consumption.

## Reproduce

Run stages without other benchmarks competing for CPU:

```sh
mkdir -p target/autosave-bench
XDG_STATE_HOME="$PWD/target/autosave-bench/state" \
  RUSTMUX_BENCH_STATE_ISOLATED=1 RUSTMUX_BENCH_ITERATIONS=15 \
  cargo bench --features benchmarks --bench session_save --locked \
  > target/autosave-bench/incremental-results.csv
```

The CSV also contains P95 capture/background times, TOML encoding time, and file size. The previous synchronous path is retained only for this benchmark comparison and unit tests.

For real-session response probes (requires local PTYs and Unix sockets):

```sh
cargo build --release --locked
python3 scripts/bench-session-save.py --output target/autosave-bench/incremental-live
```

The probe uses isolated temporary directories and stops its own servers afterward. Raw per-query CSV files and `summary.json` are generated in the output directory and are not versioned.
