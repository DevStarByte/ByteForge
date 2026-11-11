#!/usr/bin/env bash
# Renders dmg-background.svg to the PNGs the .dmg uses (1x and @2x for Retina). Needs rsvg-convert and the
# IDE's fonts installed: Figtree and JetBrains Mono. Run it after editing the SVG and commit the PNGs.
set -euo pipefail
cd "$(dirname "$0")"
rsvg-convert -w 640 -h 400 dmg-background.svg -o dmg-background.png
rsvg-convert -w 1280 -h 800 dmg-background.svg -o dmg-background@2x.png
echo "Rendered dmg-background.png and dmg-background@2x.png"
