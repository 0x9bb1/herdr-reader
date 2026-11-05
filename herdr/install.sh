#!/usr/bin/env bash
# herdr's `[[build]]` step: build the release binary and put it in bin/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  for d in "$HOME/.cargo/bin"; do
    [ -x "$d/cargo" ] && PATH="$d:$PATH"
  done
fi

echo "reader: cargo build --release"
cargo build --release --quiet
mkdir -p bin
install -m 0755 target/release/herdr-reader bin/herdr-reader
echo "reader: installed $ROOT/bin/herdr-reader"
