#!/usr/bin/env bash
# Собирает всё: сервер, консоль и окно. Бинарники — в target/release.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release
echo
echo "Готово:"
echo "  target/release/krug-server  — сервер"
echo "  target/release/krug         — консольный клиент"
echo "  target/release/krug-gui     — нативное окно"
