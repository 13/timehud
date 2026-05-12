#!/usr/bin/env bash
set -euo pipefail

APPDIR="${1:-app}"
APP_NAME="${APP_NAME:-timehud}"
DESKTOP_FILE="$APPDIR/usr/share/applications/${APP_NAME}.desktop"
ICON_DIR="$APPDIR/usr/share/icons/hicolor/256x256/apps"

mkdir -p "$ICON_DIR"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/bin"

# Handle entrypoint.sh → AppRun conversion (python-appimage requirement)
if [ -f "$APPDIR/entrypoint.sh" ]; then
  chmod +x "$APPDIR/entrypoint.sh"
elif [ -f "$APPDIR/AppRun" ]; then
  # python-appimage consumes entrypoint.* to generate AppDir/AppRun.
  cp "$APPDIR/AppRun" "$APPDIR/entrypoint.sh"
  chmod +x "$APPDIR/entrypoint.sh"
else
  echo "ERROR: missing $APPDIR/entrypoint.sh (or $APPDIR/AppRun)" >&2
  exit 1
fi
chmod +x "$APPDIR/AppRun" 2>/dev/null || true

cp "$APPDIR/timehud.desktop" "$DESKTOP_FILE"
sed -i 's/^Exec=.*/Exec=AppRun/' "$DESKTOP_FILE"
sed -i 's/^Icon=.*/Icon=timehud/' "$DESKTOP_FILE"

cp "$APPDIR/timehud.svg" "$ICON_DIR/timehud.svg"

if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w 256 -h 256 \
    "$APPDIR/timehud.svg" \
    -o "$ICON_DIR/timehud.png"
fi

# Keep icon at AppDir root so .DirIcon can point to it.
ln -sf timehud.svg "$APPDIR/.DirIcon"
ln -sf ../AppRun "$APPDIR/usr/bin/timehud"

