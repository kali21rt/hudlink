#!/usr/bin/env bash
# Upload the firmware. The daemon is stopped first because it holds the serial port.
set -euo pipefail
cd "$(dirname "$0")/../firmware"
PORT="${1:-/dev/hudlink}"
[[ -e "$PORT" ]] || PORT=/dev/aksarahud
[[ -e "$PORT" ]] || PORT=/dev/ttyUSB0
systemctl --user stop hudlink.service aksara-hud.service 2>/dev/null || true
pio run -t upload --upload-port "$PORT"
systemctl --user start hudlink.service 2>/dev/null || true
