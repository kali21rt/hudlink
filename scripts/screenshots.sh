#!/usr/bin/env bash
# Capture every page (and a few themes) into docs/images for the README.
# The display must be plugged in and the daemon running.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=docs/images
mkdir -p "$OUT"
theme_before=$(hudlink get theme | grep '=' | sed 's/.*= *//' || true)
for page in home system process cava lyrics; do
  hudlink page "$page" >/dev/null
  sleep 3   # let graphs, cava and lyrics fill in
  hudlink screenshot "$OUT/$page.png"
done
hudlink page home >/dev/null
for theme in ocean forest rose amber; do
  hudlink set theme "$theme" >/dev/null
  sleep 2
  hudlink screenshot "$OUT/theme-$theme.png"
done
hudlink set theme "${theme_before:-auto}" >/dev/null
echo "Saved to $OUT. Play a song first if you want cava and lyrics in the pictures."
