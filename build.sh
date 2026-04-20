#!/usr/bin/env bash
set -euo pipefail

APP_NAME="timehud"
APPDIR="app"
VERSION="${VERSION:-$(git describe --tags --abbrev=0 2>/dev/null || echo "$(git rev-parse --short HEAD)")}"
ARCH="$(uname -m)"
BUILD_VENV="${APPIMAGE_VENV:-.venv-appimage}"
VENV_PYTHON="$BUILD_VENV/bin/python"

echo "==> Preparing local build environment ($BUILD_VENV)"
if [ ! -x "$VENV_PYTHON" ]; then
  python3 -m venv "$BUILD_VENV"
fi

"$VENV_PYTHON" -m pip install --upgrade pip
"$VENV_PYTHON" -m pip install python-appimage
if [ -f requirements.txt ]; then
  "$VENV_PYTHON" -m pip install -r requirements.txt
fi

echo "==> Preparing AppDir"

mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/bin"

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

sed -i 's/^Exec=.*/Exec=AppRun/' "$APPDIR/timehud.desktop"
sed -i 's/^Icon=.*/Icon=timehud/' "$APPDIR/timehud.desktop"

cp "$APPDIR/timehud.svg" "$APPDIR/usr/share/icons/hicolor/256x256/apps/timehud.svg"

if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w 256 -h 256 \
    "$APPDIR/timehud.svg" \
    -o "$APPDIR/usr/share/icons/hicolor/256x256/apps/timehud.png"
fi

# Keep icon at AppDir root so .DirIcon can point to it.
ln -sf timehud.svg "$APPDIR/.DirIcon"
ln -sf ../AppRun "$APPDIR/usr/bin/timehud"

echo "==> Building AppImage"

PYTHONPATH=src "$VENV_PYTHON" -m python_appimage build app -n "$APP_NAME" "$APPDIR"

OUTPUT="${APP_NAME}-${VERSION}-${ARCH}.AppImage"

LATEST_APPIMAGE="$(/bin/ls -1t ./*.AppImage | head -n 1)"
if [ -z "$LATEST_APPIMAGE" ]; then
  echo "ERROR: no AppImage artifact found" >&2
  exit 1
fi

if [ "$LATEST_APPIMAGE" != "./$OUTPUT" ] && [ "$LATEST_APPIMAGE" != "$OUTPUT" ]; then
  mv -f "$LATEST_APPIMAGE" "$OUTPUT"
fi

echo "==> Verifying"
chmod +x "$OUTPUT"
./"$OUTPUT" --appimage-version || true

echo "==> Done: $OUTPUT"
