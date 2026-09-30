#!/usr/bin/env bash
# Рендерит окно RustBox в PNG без реального экрана (egui_kittest + wgpu).
# Реальные настройки и профили не трогаются: XDG-каталоги подменяются на временные.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")/.."

out=${1:-target/screenshots}
mkdir -p "$out"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

XDG_CONFIG_HOME=$tmp/config XDG_DATA_HOME=$tmp/data XDG_RUNTIME_DIR=$tmp/run \
RUSTBOX_SCREENSHOT_DIR=$PWD/$out \
  cargo test -q -p rustbox-gui screenshots -- --ignored --nocapture
