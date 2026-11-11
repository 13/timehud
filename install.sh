#!/usr/bin/env bash
# install.sh - build TimeHUD and install it under $PREFIX (default ~/.local)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/timehud"
ICON_ROOT="$PREFIX/share/icons/hicolor"
APP_DIR="$PREFIX/share/applications"

echo "==> Building (release) ..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

echo "==> Installing to $PREFIX ..."
install -Dm755 "$SCRIPT_DIR/target/release/timehud" "$BIN"
install -Dm644 "$SCRIPT_DIR/assets/icons/timehud.svg" "$ICON_ROOT/scalable/apps/timehud.svg"
install -Dm644 "$SCRIPT_DIR/assets/timehud.desktop" "$APP_DIR/timehud.desktop"
# Point Exec= at the absolute binary path
sed -i "s|^Exec=.*|Exec=$BIN|" "$APP_DIR/timehud.desktop"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache "$ICON_ROOT" -t 2>/dev/null || true
fi

echo ""
echo "Done. Run the overlay with:"
echo "   $BIN"
echo ""
echo "   Optional flags: --position top-left|top-right|bottom-left|bottom-right|top-center|bottom-center"
echo "                   --reset-config  --no-tray  --version"
echo "   Control a running instance: timehud toggle | reset | show | hide | quit"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "   Note: $PREFIX/bin is not on your PATH." ;;
esac
