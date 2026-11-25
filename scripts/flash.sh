#!/usr/bin/env bash
# Upload the firmware. The daemon is stopped first because it holds the serial port.
#   bash scripts/flash.sh [port]
set -euo pipefail
cd "$(dirname "$0")/../firmware"
if ! command -v pio >/dev/null 2>&1; then
  echo "PlatformIO is missing. Install it with:  pipx install platformio   (then open a new terminal)"
  exit 1
fi
PORT="${1:-}"
if [[ -z "$PORT" ]]; then
  for p in /dev/hudlink /dev/aksarahud /dev/ttyUSB0 /dev/ttyACM0; do
    [[ -e "$p" ]] && { PORT="$p"; break; }
  done
fi
[[ -n "$PORT" ]] || { echo "No ESP32 found. Plug it in, or pass the port: bash scripts/flash.sh /dev/ttyUSB0"; exit 1; }

STOPPED=""
if command -v systemctl >/dev/null 2>&1 && systemctl --user is-active --quiet hudlink.service 2>/dev/null; then
  systemctl --user stop hudlink.service; STOPPED=systemd
elif pgrep -x hudlink >/dev/null 2>&1; then
  pkill -x hudlink; STOPPED=process
fi
echo "Flashing $PORT (hold BOOT if it hangs at 'Connecting...')"
pio run -t upload --upload-port "$PORT"
case "$STOPPED" in
  systemd) systemctl --user start hudlink.service ;;
  process) (nohup "$HOME/.local/bin/hudlink" >/dev/null 2>&1 &) ;;
esac
echo "Done."
