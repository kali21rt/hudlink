#!/usr/bin/env bash
# Upload firmware ke ESP32. Daemon dihentikan dulu karena ia memegang port serial.
set -euo pipefail
cd "$(dirname "$0")/../firmware"
PORT="${1:-/dev/aksarahud}"
[[ -e "$PORT" ]] || PORT=/dev/ttyUSB0
systemctl --user stop aksara-hud.service 2>/dev/null || true
pio run -t upload --upload-port "$PORT"
systemctl --user start aksara-hud.service 2>/dev/null || true
