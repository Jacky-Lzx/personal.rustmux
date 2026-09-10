#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tool_root="$project_root/target/demo-tools"

if ! command -v go >/dev/null 2>&1; then
  echo "Go is required to install the pinned VHS recorder." >&2
  exit 1
fi

mkdir -p "$tool_root/bin" "$tool_root/mod" "$tool_root/gopath"

# VHS 0.12.0 cancels the render context before starting ffmpeg on macOS.
# Pin 0.11.0 until an upstream release contains the context-lifetime fix.
GOBIN="$tool_root/bin" \
GOMODCACHE="$tool_root/mod" \
GOPATH="$tool_root/gopath" \
go install github.com/charmbracelet/vhs@v0.11.0

"$tool_root/bin/vhs" --version
