#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

vhs_bin="$project_root/target/demo-tools/bin/vhs"
if [[ ! -x "$vhs_bin" ]]; then
  echo "The pinned VHS recorder is missing. Run: ./scripts/install-demo-tools.sh" >&2
  exit 1
fi

for dependency in ttyd ffmpeg fish; do
  if ! command -v "$dependency" >/dev/null 2>&1; then
    echo "$dependency is required. On macOS run: brew install ttyd ffmpeg fish" >&2
    exit 1
  fi
done

cargo build --release --locked
mkdir -p demos/assets target/demo-state
export XDG_CONFIG_HOME="$project_root/demos/config"
export XDG_STATE_HOME="$project_root/target/demo-state"
export RUSTMUX_SHELL="fish"

demo_sessions=(
  rustmux-readme-layout
  rustmux-readme-main
  rustmux-readme-work
  rustmux-readme-history
)

cleanup_sessions() {
  for session in "${demo_sessions[@]}"; do
    ./target/release/rustmux kill-session -t "$session" >/dev/null 2>&1 || true
  done
}

cleanup_sessions
trap cleanup_sessions EXIT

if (( $# > 0 )); then
  tapes=("$@")
else
  tapes=(
    demos/tapes/windows-and-panes.tape
    demos/tapes/session-manager.tape
    demos/tapes/history-and-help.tape
  )
fi

for tape in "${tapes[@]}"; do
  cleanup_sessions
  demo_output="$(sed -n 's/^Output //p' "$tape" | head -n 1)"
  case "$demo_output" in
    demos/assets/*.gif) ;;
    *)
      echo "Tape output must be a GIF under demos/assets/: $tape" >&2
      exit 1
      ;;
  esac
  rm -f "$demo_output"
  echo "Recording $tape"
  "$vhs_bin" "$tape"
  if [[ ! -s "$demo_output" ]]; then
    echo "VHS did not create the expected output for $tape" >&2
    exit 1
  fi
  cleanup_sessions
done

echo "Generated demos in demos/assets/."
