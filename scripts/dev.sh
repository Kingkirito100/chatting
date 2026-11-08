#!/usr/bin/env bash
# Локальный сервер: TCP 7777 и WebSocket 8080, данные рядом с проектом.
set -euo pipefail
cd "$(dirname "$0")/.."
export KRUG_INVITE="${KRUG_INVITE:-свои}"
exec cargo run -p krug-server -- \
  --addr 0.0.0.0:7777 \
  --ws-addr 0.0.0.0:8080 \
  --data ./data \
  --invite "$KRUG_INVITE" \
  --name "Круг" \
  --motd "Свои люди, свои игры." \
  "$@"
