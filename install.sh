#!/usr/bin/env bash
# Builds and installs the applet into the user's ~/.local prefix.
# Pass --enable to also add it to the right side of the COSMIC panel.
set -euo pipefail

APP_ID="io.github.MoisesRoig.cosmic-ext-applet-agents"
PREFIX="${PREFIX:-$HOME/.local}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cargo build --release --manifest-path "$HERE/Cargo.toml"

install -Dm755 "$HERE/target/release/cosmic-ext-applet-agents" "$PREFIX/bin/cosmic-ext-applet-agents"
install -Dm644 "$HERE/data/$APP_ID.desktop" "$PREFIX/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/data/$APP_ID.metainfo.xml" "$PREFIX/share/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "$HERE/data/icon.svg" \
    "$PREFIX/share/icons/hicolor/scalable/apps/io.github.MoisesRoig.cosmic-ext-applet-agents-symbolic.svg"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" || true
fi

echo "Installed to $PREFIX."

if [[ "${1:-}" == "--enable" ]]; then
    python3 "$HERE/scripts/enable-panel.py" "$APP_ID"
else
    echo "Add it from Settings > Desktop > Panel > Configure panel applets,"
    echo "or re-run with: ./install.sh --enable"
fi
