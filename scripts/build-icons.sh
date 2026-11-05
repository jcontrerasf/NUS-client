#!/bin/sh
# Genera los íconos rasterizados a partir de assets/nus-client.svg:
# - assets/nus-client-<N>.png: íconos hicolor del .deb (y 64 px para la ventana, main.rs);
# - assets/nus-client.ico: varios tamaños, incrustado en el .exe de Windows (build.rs).
# Requiere Inkscape e ImageMagick. Volver a ejecutarlo tras editar el SVG.
set -e
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for size in 16 24 32 48 64 128 256 512; do
    inkscape assets/nus-client.svg -o "$tmp/$size.png" -w "$size" -h "$size" >/dev/null 2>&1
done
for size in 16 32 48 64 128 256 512; do
    # PNG32: RGBA de 8 bits, el formato que espera el ícono de la ventana.
    convert "$tmp/$size.png" "PNG32:assets/nus-client-$size.png"
done
convert "$tmp/16.png" "$tmp/24.png" "$tmp/32.png" "$tmp/48.png" "$tmp/64.png" "$tmp/128.png" "$tmp/256.png" \
    assets/nus-client.ico
