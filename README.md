# hudlink

A small external status display for Linux: ESP32 + 2.8" SPI TFT over USB.
Clock, weather, system monitor, task manager, cava visualizer, now playing and synced lyrics.
Colors follow your theme (presets, distro color, or matugen), the logo comes from your distro.

    firmware/   PlatformIO (C++, TFT_eSPI)     -> bash scripts/flash.sh
    host/       Rust daemon, no dependencies    -> cargo build --release
    system/     udev rule, systemd user service, matugen template
    scripts/    install.sh (once), flash.sh (after firmware changes)

Quick start:
    bash scripts/install.sh
    bash scripts/flash.sh
    hudlink --dry-run
    hudlink set theme forest
    hudlink next

License: GPL-3.0-or-later
