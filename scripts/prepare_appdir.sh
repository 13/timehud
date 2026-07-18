#!/usr/bin/env bash
set -euo pipefail

APPDIR="${1:-app}"
APP_NAME="${APP_NAME:-timehud}"
DESKTOP_FILE="$APPDIR/usr/share/applications/${APP_NAME}.desktop"
HICOLOR="$APPDIR/usr/share/icons/hicolor"
ICON_DIR="$HICOLOR/256x256/apps"

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

# Scalable source of truth for desktop themes.
mkdir -p "$HICOLOR/scalable/apps"
cp "$APPDIR/timehud.svg" "$HICOLOR/scalable/apps/timehud.svg"
cp "$APPDIR/timehud.svg" "$ICON_DIR/timehud.svg"

if command -v rsvg-convert >/dev/null 2>&1; then
  # Rasterize the full hicolor size set so docks/panels get a crisp match.
  for s in 16 24 32 48 64 128 256; do
    d="$HICOLOR/${s}x${s}/apps"
    mkdir -p "$d"
    rsvg-convert -w "$s" -h "$s" "$APPDIR/timehud.svg" -o "$d/timehud.png"
  done
fi

# Keep icon at AppDir root so .DirIcon can point to it.
ln -sf timehud.svg "$APPDIR/.DirIcon"
ln -sf ../AppRun "$APPDIR/usr/bin/timehud"

