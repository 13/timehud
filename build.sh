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

APPDIR="$APPDIR" APP_NAME="$APP_NAME" bash ./scripts/prepare_appdir.sh "$APPDIR"

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
