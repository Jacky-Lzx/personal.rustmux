#!/usr/bin/env bash
set -euo pipefail

docs_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$docs_root"

mdbook build

test -f dist/index.html
find dist -maxdepth 1 -name 'searchindex-*.js' -print -quit | grep -q .

echo "Built documentation in dist/."
