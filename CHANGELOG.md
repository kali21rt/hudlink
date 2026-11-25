# Changelog

## 2.1.0 (2026-11-25)

First public release.

- `hudlink screenshot`: reads the panel memory back over the SDO/MISO wire and saves a PNG
- `hudlink status`, `hudlink doctor` (checks the whole setup and suggests fixes), `hudlink setup` (first-run wizard)
- screen follows the laptop: backlight off while the session is locked, dimmer at night
- installer for dnf, apt, pacman, zypper and xbps; XDG autostart when there is no systemd
- every config value is validated before it is saved
- CI builds the daemon and the firmware on every push

## 2.0.0 (2026-11-24)

Renamed from Aksara HUD to hudlink and made distro-agnostic. UI, config and CLI in English.

- title and logo follow the distro; themes (presets, distro color, matugen, cava gradient)
- clock styles, 12/24 h, custom date lines (including the Javanese pasaran)
- pages and home layouts are configurable
- system page with CPU and network graphs, process page (top 8)
- now playing (any MPRIS player) and synced lyrics (local .lrc or LRCLIB)
- live configuration with `hudlink set`

## 1.0.0 (2026-11-23)

Aksara HUD, made for my own Fedora laptop and the Aksara OS look.

- ESP32 + 2.8" ILI9341 firmware: clock with pasaran, weather, system monitor, cava
- Rust daemon without external crates, binary serial protocol
- starts automatically when the display is plugged in (udev + systemd user service)
