#!/usr/bin/env bash
# Мгновенный публичный адрес без аккаунта: Cloudflare Quick Tunnel.
# Компьютер должен быть включён, пока играете.
set -euo pipefail
if ! command -v cloudflared >/dev/null 2>&1; then
  echo "Нужен cloudflared (бесплатно):"
  echo "  macOS:  brew install cloudflared"
  echo "  Linux:  https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
  exit 1
fi
echo "Запусти в другом окне scripts/dev.sh, затем сюда."
echo "Ниже появится адрес https://… — клиенту указывай wss://…"
exec cloudflared tunnel --url http://localhost:8080
