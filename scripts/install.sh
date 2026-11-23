#!/usr/bin/env bash
# Aksara HUD installer untuk Fedora. Jalankan dari folder aksara-hud/:  ./scripts/install.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 1/5 Paket yang dibutuhkan"
sudo dnf install -y cava curl usbutils

echo "==> 2/5 Build daemon Rust"
( cd host && cargo build --release )
install -Dm755 host/target/release/aksara-hud "$HOME/.local/bin/aksara-hud"

echo "==> 3/5 Deteksi chip USB ESP32 (pastikan ESP32 sedang dicolok)"
VIDPID=""
for id in 1a86:7523 10c4:ea60 1a86:55d4; do
  if lsusb -d "$id" >/dev/null 2>&1; then VIDPID="$id"; break; fi
done
if [[ -z "$VIDPID" ]]; then
  echo "   Chip tidak dikenali otomatis. Lihat 'lsusb', lalu ketik VID:PID (contoh 1a86:7523):"
  read -r VIDPID
fi
VID="${VIDPID%%:*}"; PID="${VIDPID##*:}"
echo "   memakai $VID:$PID"
sed -e "s/__VID__/$VID/" -e "s/__PID__/$PID/" system/70-aksara-hud.rules \
  | sudo tee /etc/udev/rules.d/70-aksara-hud.rules >/dev/null

echo "==> 4/5 Muat ulang udev"
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=tty

echo "==> 5/5 Pasang systemd user service"
install -Dm644 system/aksara-hud.service "$HOME/.config/systemd/user/aksara-hud.service"
systemctl --user daemon-reload
systemctl --user enable aksara-hud.service
[[ -e /dev/aksarahud ]] && systemctl --user restart aksara-hud.service || true

echo
echo "Selesai. Edit kota di ~/.config/aksara-hud/config lalu:"
echo "  systemctl --user restart aksara-hud"
echo "Log:  journalctl --user -u aksara-hud -f"
