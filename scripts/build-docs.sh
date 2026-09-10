#!/usr/bin/env bash
set -euo pipefail

docs_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$docs_root"

mdbook build
MDBOOK_BOOK__TITLE="Rustmux 文档" \
MDBOOK_BOOK__DESCRIPTION="Rustmux 使用指南、配置参考与终端协议兼容性文档" \
MDBOOK_BOOK__LANGUAGE="zh-CN" \
MDBOOK_BOOK__SRC="docs-zh" \
MDBOOK_BUILD__BUILD_DIR="dist/zh" \
mdbook build

test -f dist/index.html
test -f dist/zh/index.html
find dist -maxdepth 1 -name 'searchindex-*.js' -print -quit | grep -q .
find dist/zh -maxdepth 1 -name 'searchindex-*.js' -print -quit | grep -q .

echo "Built English documentation in dist/ and Chinese documentation in dist/zh/."
