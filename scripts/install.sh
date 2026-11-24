#!/usr/bin/env bash
# hudlink installer (Fedora). Run from the hudlink/ folder:  bash scripts/install.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 1/6 Packages"
sudo dnf install -y cava curl usbutils playerctl ImageMagick

echo "==> 2/6 Remove the old aksara-hud (if present)"
systemctl --user disable --now aksara-hud.service 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/aksara-hud.service" "$HOME/.local/bin/aksara-hud"
sudo rm -f /etc/udev/rules.d/70-aksara-hud.rules

echo "==> 3/6 Build the Rust daemon"
( cd host && cargo build --release )
install -Dm755 host/target/release/hudlink "$HOME/.local/bin/hudlink"

echo "==> 4/6 Detect the ESP32 USB chip (keep it plugged in)"
VIDPID=""
for id in 1a86:7523 10c4:ea60 1a86:55d4; do
  if lsusb -d "$id" >/dev/null 2>&1; then VIDPID="$id"; break; fi
done
if [[ -z "$VIDPID" ]]; then
  echo "   Not detected automatically. Check 'lsusb' and type VID:PID (e.g. 1a86:7523):"
  read -r VIDPID
fi
VID="${VIDPID%%:*}"; PID="${VIDPID##*:}"
echo "   using $VID:$PID"
sed -e "s/__VID__/$VID/" -e "s/__PID__/$PID/" system/70-hudlink.rules \
  | sudo tee /etc/udev/rules.d/70-hudlink.rules >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=tty --action=add

echo "==> 5/6 systemd user service"
install -Dm644 system/hudlink.service "$HOME/.config/systemd/user/hudlink.service"
systemctl --user daemon-reload
systemctl --user enable hudlink.service 2>/dev/null
sleep 1
[[ -e /dev/hudlink ]] && systemctl --user restart hudlink.service || echo "   (re-plug the display if it does not start)"

echo "==> 6/6 matugen template (only if matugen is installed)"
if command -v matugen >/dev/null 2>&1; then
  install -Dm644 system/matugen/hudlink-colors "$HOME/.config/matugen/templates/hudlink-colors"
  echo "   template copied. Add the block from system/matugen/config-snippet.toml to ~/.config/matugen/config.toml"
else
  echo "   matugen not found, skipped (theme falls back to the distro color)"
fi

echo
echo "Done. Try:  hudlink --dry-run   |   hudlink set theme forest   |   hudlink next"
echo "Logs:       journalctl --user -u hudlink -f"
