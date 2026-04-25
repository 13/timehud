#!/usr/bin/env sh

set -eu

BASE_URL="https://github.com/13/timehud/releases/latest/download"
TMP_DIR="$(mktemp -d)"
FILE="timehud-x86_64.AppImage"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

# Detect install dir
if [ -d "$HOME/bin" ]; then
  INSTALL_DIR="$HOME/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

echo "Downloading $FILE..."
curl -fsSL -o "$TMP_DIR/$FILE" "$BASE_URL/$FILE"

cd "$TMP_DIR"

echo "Installing..."
mv "$FILE" "$INSTALL_DIR/$FILE"
chmod +x "$INSTALL_DIR/$FILE"

echo "Done! Run: $FILE"