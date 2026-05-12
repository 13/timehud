#!/usr/bin/env sh

set -eu

REPO="13/timehud"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

if [ -d "$HOME/bin" ]; then
  INSTALL_DIR="$HOME/bin"
else
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

echo "Fetching latest release..."

ASSET_URL="$(curl -fsSL \
  "https://api.github.com/repos/$REPO/releases/latest" \
  | grep browser_download_url \
  | grep AppImage \
  | cut -d '"' -f 4)"

echo "Downloading..."
curl -fsSL -o "$TMP_DIR/timehud.AppImage" "$ASSET_URL"

install -m 755 "$TMP_DIR/timehud.AppImage" "$INSTALL_DIR/timehud"

echo "Done! Run: timehud"
