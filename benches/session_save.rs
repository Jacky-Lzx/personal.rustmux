use rustmux::benchmarking::SessionSaveRunner;

fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
}

fn main() {
    // Set XDG_STATE_HOME in the launcher to isolate writes from real sessions.
    assert!(std::env::var_os("RUSTMUX_BENCH_STATE_ISOLATED").is_some());
    assert!(std::env::var_os("XDG_STATE_HOME").is_some());
    let iterations: usize = std::env::var("RUSTMUX_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15)
        .max(3);
    println!(
        "case,panes,lines_per_pane,density,capture_p50_ms,capture_p95_ms,unchanged_p50_ms,unchanged_p95_ms,encode_p50_ms,background_p50_ms,background_p95_ms,file_bytes,baseline_capture_p50_ms,one_changed_p50_ms"
    );
    for (name, panes, lines, density, history, colors) in [
        ("layout", 1, 5000, 0, false, false),
        ("layout", 8, 5000, 0, false, false),
        ("plain", 1, 1000, 0, true, false),
        ("plain", 1, 5000, 0, true, false),
        ("plain", 4, 5000, 0, true, false),
        ("plain", 8, 5000, 0, true, false),
        ("plain", 1, 20000, 0, true, false),
        ("color", 1, 5000, 1, true, true),
        ("color", 4, 5000, 1, true, true),
        ("color", 8, 5000, 1, true, true),
        ("color", 1, 20000, 1, true, true),
        ("dense-color", 1, 5000, 2, true, true),
        ("dense-color", 4, 5000, 2, true, true),
        ("dense-color-as-plain", 4, 5000, 2, true, false),
    ] {
        let mut runner = SessionSaveRunner::new(panes, lines, density, history, colors);
        runner.measure(); // warm caches and allocator
        let samples: Vec<_> = (0..iterations).map(|_| runner.measure()).collect();
        let mut baseline: Vec<_> = samples.iter().map(|s| s.baseline_capture_ms).collect();
        let mut one_changed: Vec<_> = samples.iter().map(|s| s.one_changed_ms).collect();
        let mut capture: Vec<_> = samples.iter().map(|s| s.capture_ms).collect();
        let mut unchanged: Vec<_> = samples.iter().map(|s| s.unchanged_ms).collect();
        let mut encode: Vec<_> = samples.iter().map(|s| s.encode_ms).collect();
        let mut background: Vec<_> = samples.iter().map(|s| s.background_ms).collect();
        println!(
            "{name},{panes},{lines},{density},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3}",
            percentile(&mut capture, 0.5),
            percentile(&mut capture, 0.95),
            percentile(&mut unchanged, 0.5),
            percentile(&mut unchanged, 0.95),
            percentile(&mut encode, 0.5),
            percentile(&mut background, 0.5),
            percentile(&mut background, 0.95),
            samples[0].bytes,
            percentile(&mut baseline, 0.5),
            percentile(&mut one_changed, 0.5)
        );
    }
}
