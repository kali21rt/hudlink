# Aksara HUD

HUD eksternal ESP32 + TFT 2.8" untuk Fedora: jam + pasaran, cuaca, system monitor, cava.

    firmware/   PlatformIO (C++, TFT_eSPI)  -> pio run -t upload
    host/       daemon Rust tanpa dependency -> cargo build --release
    system/     aturan udev + systemd user service
    scripts/    install.sh (sekali), flash.sh (tiap update firmware)

Cepat:
    ./scripts/flash.sh /dev/ttyUSB0   # pertama kali
    ./scripts/install.sh
    aksara-hud --dry-run              # cek data tanpa ESP32
    aksara-hud next | rotate | brightness 120   # kontrol dari terminal

Panduan lengkap ada di dokumen "Panduan Aksara HUD".
Lisensi: GPL-3.0-or-later
