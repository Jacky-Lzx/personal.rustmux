use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustmux::benchmarking::{ImagePreviewRunner, image_preview_stream};

const PNG_1PX: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const JPEG_1PX: &str = "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////2wBDAf//////////////////////////////////////////////////////////////////////////////////////wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAH/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/9oADAMBAAIQAxAAAAF//8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABBQJ//8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAgBAwEBPwF//8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAgBAgEBPwF//8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAGPwJ//8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPyF//9oADAMBAAIAAwAAABD/xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oACAEDAQE/EB//xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oACAECAQE/EB//xAAUEAEAAAAAAAAAAAAAAAAAAAAA/9oACAEBAAE/EB//2Q==";
const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="600" viewBox="0 0 800 600"><defs><linearGradient id="g"><stop stop-color="#89b4fa"/><stop offset="1" stop-color="#cba6f7"/></linearGradient></defs><rect width="800" height="600" fill="url(#g)"/><circle cx="400" cy="300" r="180" fill="#1e1e2e"/><text x="400" y="320" text-anchor="middle" font-size="64" fill="#cdd6f4">rustmux</text></svg>"##;
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 800 600]/Contents 4 0 R>>endobj\n4 0 obj<</Length 45>>stream\n0.54 0.71 0.98 rg 0 0 800 600 re f\nendstream\nendobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";

struct Fixture {
    name: &'static str,
    path: PathBuf,
    source: Vec<u8>,
    stream: Vec<u8>,
}

fn main() {
    let iterations = environment_usize("RUSTMUX_BENCH_ITERATIONS", 20).max(3);
    let payload_mib = environment_usize("RUSTMUX_BENCH_PAYLOAD_MIB", 1).max(1);
    let payload_bytes = payload_mib * 1024 * 1024;
    let fixtures = fixtures(payload_bytes);
    let mut runner = ImagePreviewRunner::new();

    println!(
        "Rustmux image preview: {payload_mib} MiB normalized payload, {iterations} iterations"
    );
    println!(
        "{:<6} {:>10} {:>12} {:>12} {:>12} {:>12}",
        "format", "source", "wire", "median", "mean", "throughput"
    );
    for fixture in fixtures {
        for _ in 0..3 {
            black_box(runner.run(black_box(&fixture.stream)));
        }
        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            black_box(runner.run(black_box(&fixture.stream)));
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        let mean = samples.iter().sum::<Duration>() / samples.len() as u32;
        let throughput = fixture.stream.len() as f64 / median.as_secs_f64() / (1024.0 * 1024.0);
        println!(
            "{:<6} {:>7.1} KiB {:>8.2} MiB {:>9.3} ms {:>9.3} ms {:>8.1} MiB/s",
            fixture.name,
            fixture.source.len() as f64 / 1024.0,
            fixture.stream.len() as f64 / (1024.0 * 1024.0),
            milliseconds(median),
            milliseconds(mean),
            throughput,
        );
        black_box(fixture.path);
    }
}

fn fixtures(payload_bytes: usize) -> Vec<Fixture> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("image-preview-bench-fixtures");
    fs::create_dir_all(&directory).expect("create benchmark fixture directory");
    [
        ("jpg", base64_decode(JPEG_1PX)),
        ("png", base64_decode(PNG_1PX)),
        ("pdf", PDF.to_vec()),
        ("svg", SVG.to_vec()),
    ]
    .into_iter()
    .map(|(name, source)| {
        let path = directory.join(format!("preview.{name}"));
        fs::write(&path, &source).expect("write benchmark fixture");
        let source = fs::read(&path).expect("read benchmark fixture");
        let stream = image_preview_stream(&source, payload_bytes);
        Fixture {
            name,
            path,
            source,
            stream,
        }
    })
    .collect()
}

fn environment_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn base64_decode(value: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for byte in value.bytes().filter(|byte| *byte != b'=') {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        accumulator = (accumulator << 6) | u32::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1_u32 << bits) - 1;
        }
    }
    output
}
