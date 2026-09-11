#!/usr/bin/env python3
"""Probe real server response latency during autosaves with isolated synthetic data."""
import argparse
import csv
import json
import os
from pathlib import Path
import shlex
import socket
import statistics
import subprocess
import tempfile
import time


def percentile(values, fraction):
    return sorted(values)[min(len(values) - 1, int(len(values) * fraction))]


def status(path):
    started = time.perf_counter()
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(5)
        stream.connect(str(path))
        stream.sendall(b'S')
        stream.recv(512)
    return (time.perf_counter() - started) * 1000


def wait_for(predicate, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (FileNotFoundError, ConnectionError):
            pass
        time.sleep(0.02)
    raise RuntimeError('server or fixture did not become ready')


def fixture(path, density, lines):
    with path.open('wb') as output:
        for row in range(lines + 24):
            text = []
            for column in range(120):
                if density == 2 or (density == 1 and column % 40 == 0):
                    text.append(f'\x1b[38;5;{(row + column) % 216 + 16}m')
                text.append(chr(97 + (row + column) % 26))
            output.write((''.join(text) + '\x1b[0m\n').encode())


def run(binary, case, output_dir):
    name, history, colors, density, interval, changing, duration = case
    with tempfile.TemporaryDirectory(prefix='rm-save-', dir='/tmp') as temporary:
        root = Path(temporary)
        config = root / 'config/rustmux/config.toml'
        config.parent.mkdir(parents=True)
        settings = f'scrollback_lines = 5000\nsave_scrollback = {str(history).lower()}\nsave_scrollback_colors = {str(colors).lower()}\n'
        config.write_text(settings + 'autosave_interval_seconds = 0\n')
        source = root / 'fixture.txt'
        fixture(source, density, 5000)
        env = dict(os.environ, TMPDIR=str(root), XDG_CONFIG_HOME=str(root / 'config'), XDG_STATE_HOME=str(root / 'state'), RUSTMUX_SHELL='/bin/sh', ENV='/dev/null')
        env.pop('RUSTMUX', None)
        socket_path = root / f'rustmux-{os.getuid()}/work.sock'
        error_file = (output_dir / f'{name}.stderr').open('w')
        server = subprocess.Popen([str(binary), '--server', str(socket_path), '122', '28', '0', '0'], env=env, cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=error_file)
        def cli(*args):
            return subprocess.check_output([str(binary), *args], env=env, cwd=root, stderr=subprocess.PIPE).decode()
        try:
            wait_for(lambda: socket_path.exists() and status(socket_path) >= 0)
            for index in range(4):
                pane = '1' if index == 0 else cli('new-window', '-s', 'work').strip()
                command = f'cat {shlex.quote(str(source))}; printf "ready-{index}\\n"'
                cli('send-keys', '-s', 'work', '-p', pane, '--literal', '--enter', command)
                wait_for(lambda: f'ready-{index}' in cli('capture-pane', '-s', 'work', '-p', pane))
                # Capture can see the echoed command first; use a separate marker file.
                marker = root / f'ready-{index}'
                cli('send-keys', '-s', 'work', '-p', pane, '--literal', '--enter', f'touch {shlex.quote(str(marker))}')
                wait_for(marker.exists)
                if changing:
                    cli('send-keys', '-s', 'work', '-p', pane, '--literal', '--enter', 'while :; do printf "tick\\n"; sleep 0.2; done')
            config.write_text(settings + f'autosave_interval_seconds = {interval}\n')
            samples = []
            snapshot = root / 'state/rustmux/sessions/work.toml'
            mtimes = set()
            started = time.monotonic()
            while time.monotonic() - started < duration:
                at = time.monotonic() - started
                samples.append((at, status(socket_path)))
                if snapshot.exists():
                    mtimes.add(snapshot.stat().st_mtime_ns)
                time.sleep(0.005)
            with (output_dir / f'{name}-latency.csv').open('w') as file:
                writer = csv.writer(file)
                writer.writerow(['elapsed_seconds', 'status_latency_ms'])
                writer.writerows(samples)
            values = [row[1] for row in samples]
            return dict(case=name, panes=4, lines_per_pane=5000, interval=interval, changing=changing, duration=duration, samples=len(values), p50_ms=statistics.median(values), p95_ms=percentile(values,.95), p99_ms=percentile(values,.99), max_ms=max(values), over_16ms=sum(v > 16 for v in values), writes_observed=len(mtimes), snapshot_bytes=snapshot.stat().st_size if snapshot.exists() else 0)
        finally:
            try:
                with socket.socket(socket.AF_UNIX) as stream:
                    stream.connect(str(socket_path)); stream.sendall(b'X')
                server.wait(timeout=15)
            except (OSError, subprocess.TimeoutExpired):
                server.kill(); server.wait()
            error_file.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=Path('target/release/rustmux'))
    parser.add_argument('--output', type=Path, default=Path('target/autosave-bench/live'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    cases = [
        ('disabled', True, True, 2, 0, False, 6),
        ('layout-1s', False, False, 0, 1, False, 6),
        ('plain-1s', True, False, 0, 1, False, 6),
        ('color-1s', True, True, 1, 1, False, 6),
        ('dense-1s-static', True, True, 2, 1, False, 6),
        ('dense-1s-changing', True, True, 2, 1, True, 6),
        ('dense-30s-changing', True, True, 2, 30, True, 33),
    ]
    results = []
    for case in cases:
        result = run(args.binary.resolve(), case, args.output)
        results.append(result)
        print(json.dumps(result), flush=True)
        (args.output / 'summary.json').write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    main()
