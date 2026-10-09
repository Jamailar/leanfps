#!/usr/bin/env bash
# Regenerates the .ico files and the window icon from the SVG sources.
# Needs rsvg-convert (librsvg2-bin) and ImageMagick. Run from this folder.
set -euo pipefail
tmp=$(mktemp -d)
for n in 16 24 32; do rsvg-convert -w $n -h $n app-small.svg -o "$tmp/a$n.png"; done
for n in 48 64 128 256; do rsvg-convert -w $n -h $n app.svg -o "$tmp/a$n.png"; done
convert "$tmp"/a16.png "$tmp"/a24.png "$tmp"/a32.png "$tmp"/a48.png "$tmp"/a64.png "$tmp"/a128.png "$tmp"/a256.png ../leanfps.ico
for s in active idle; do
  for n in 16 20 24 32; do rsvg-convert -w $n -h $n tray-$s.svg -o "$tmp/t$s$n.png"; done
  convert "$tmp"/t${s}16.png "$tmp"/t${s}20.png "$tmp"/t${s}24.png "$tmp"/t${s}32.png ../tray-$s.ico
done
# 64x64 straight RGBA for the window/taskbar icon (no PNG decoder in the binary).
convert "$tmp/a64.png" -depth 8 RGBA:../window-64.rgba
rm -rf "$tmp"
