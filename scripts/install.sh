#!/usr/bin/env bash
# hudlink installer. Run from the hudlink/ folder:
#   bash scripts/install.sh            install / update
#   bash scripts/install.sh --uninstall
# Supports dnf (Fedora), apt (Debian, Ubuntu, Devuan), pacman (Arch), zypper (openSUSE), xbps (Void).
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="$HOME/.local/bin/hudlink"
UNIT="$HOME/.config/systemd/user/hudlink.service"
AUTOSTART="$HOME/.config/autostart/hudlink.desktop"
RULE=/etc/udev/rules.d/70-hudlink.rules

have() { command -v "$1" >/dev/null 2>&1; }
user_systemd() { have systemctl && systemctl --user show-environment >/dev/null 2>&1; }

if [[ "${1:-}" == "--uninstall" ]]; then
  echo "==> Removing hudlink"
  if user_systemd; then systemctl --user disable --now hudlink.service 2>/dev/null || true; fi
  pkill -x hudlink 2>/dev/null || true
  rm -f "$BIN" "$UNIT" "$AUTOSTART"
  sudo rm -f "$RULE"
  sudo udevadm control --reload-rules 2>/dev/null || true
  echo "Done. Your config is kept in ~/.config/hudlink (delete it yourself if you want)."
  exit 0
fi

echo "==> 1/6 Packages"
if have dnf; then
  sudo dnf install -y cava curl usbutils playerctl ImageMagick
elif have apt-get; then
  sudo apt-get update && sudo apt-get install -y cava curl usbutils playerctl imagemagick
elif have pacman; then
  sudo pacman -S --needed --noconfirm cava curl usbutils playerctl imagemagick
elif have zypper; then
  sudo zypper install -y cava curl usbutils playerctl ImageMagick
elif have xbps-install; then
  sudo xbps-install -Sy cava curl usbutils playerctl ImageMagick
else
  echo "   Unknown package manager. Install these yourself: cava curl usbutils playerctl imagemagick"
fi
if ! have cargo; then
  echo "   Rust (cargo) is missing. Install it with your package manager or from https://rustup.rs, then run this again."
  exit 1
fi

echo "==> 2/6 Remove the old aksara-hud (if present)"
if [[ -e "$HOME/.local/bin/aksara-hud" || -e /etc/udev/rules.d/70-aksara-hud.rules ]]; then
  if user_systemd; then systemctl --user disable --now aksara-hud.service 2>/dev/null || true; fi
  rm -f "$HOME/.config/systemd/user/aksara-hud.service" "$HOME/.local/bin/aksara-hud"
  sudo rm -f /etc/udev/rules.d/70-aksara-hud.rules
  echo "   removed"
else
  echo "   nothing to remove"
fi

echo "==> 3/6 Build the Rust daemon"
( cd host && cargo build --release )
install -Dm755 host/target/release/hudlink "$BIN"

echo "==> 4/6 udev rule (keep the ESP32 plugged in)"
VIDPID=""
if have lsusb; then
  for id in 1a86:7523 10c4:ea60 1a86:55d4; do
    if lsusb -d "$id" >/dev/null 2>&1; then VIDPID="$id"; break; fi
  done
fi
if [[ -z "$VIDPID" ]]; then
  echo "   USB chip not detected automatically. Check 'lsusb' and type VID:PID (e.g. 1a86:7523):"
  read -r VIDPID
fi
VID="${VIDPID%%:*}"; PID="${VIDPID##*:}"
echo "   using $VID:$PID"
sed -e "s/__VID__/$VID/" -e "s/__PID__/$PID/" system/70-hudlink.rules | sudo tee "$RULE" >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=tty --action=add

echo "==> 5/6 Autostart"
if user_systemd; then
  install -Dm644 system/hudlink.service "$UNIT"
  systemctl --user daemon-reload
  systemctl --user enable hudlink.service 2>/dev/null
  sleep 1
  if [[ -e /dev/hudlink ]]; then systemctl --user restart hudlink.service; else echo "   re-plug the display to start it"; fi
  echo "   systemd user service: starts when the display is plugged in"
else
  # no systemd (e.g. OpenRC, runit): run the daemon at login; it waits for the display by itself
  sed "s|__BIN__|$BIN|" system/hudlink.desktop > /tmp/hudlink.desktop
  install -Dm644 /tmp/hudlink.desktop "$AUTOSTART"
  rm -f /tmp/hudlink.desktop
  echo "   XDG autostart entry installed (works in most desktops)."
  echo "   Window managers without XDG autostart need one line, for example:"
  echo "     niri:  spawn-at-startup \"$BIN\""
  echo "     i3:    exec --no-startup-id $BIN"
  pgrep -x hudlink >/dev/null || (nohup "$BIN" >/dev/null 2>&1 &)
fi

echo "==> 6/6 matugen template (only if matugen is installed)"
if have matugen; then
  install -Dm644 system/matugen/hudlink-colors "$HOME/.config/matugen/templates/hudlink-colors"
  echo "   template copied. Add the block from system/matugen/config-snippet.toml to ~/.config/matugen/config.toml"
else
  echo "   matugen not found, skipped"
fi

case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo; echo "Note: add ~/.local/bin to your PATH to use the hudlink command.";; esac
echo
echo "Done. Next:"
echo "  bash scripts/flash.sh     upload the firmware (first time, or after updates)"
echo "  hudlink setup             city, language, clock, theme"
echo "  hudlink doctor            check everything"
