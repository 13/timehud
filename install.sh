#!/usr/bin/env bash
# install.sh – Set up TimeHUD in a virtual environment
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV="$SCRIPT_DIR/.venv"

echo "==> Creating virtual environment at $VENV …"
python3 -m venv "$VENV"

echo "==> Installing dependencies …"
"$VENV/bin/pip" install --upgrade pip -q
"$VENV/bin/pip" install -r "$SCRIPT_DIR/requirements.txt"

# Create launcher script
LAUNCHER="$SCRIPT_DIR/timehud"
cat > "$LAUNCHER" << 'EOF'
#!/usr/bin/env bash
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PYTHONPATH="$DIR/src:$PYTHONPATH"
exec "$DIR/.venv/bin/python" -m timehud.main "$@"
EOF
chmod +x "$LAUNCHER"

# Install application icon
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$ICON_DIR"
cp "$SCRIPT_DIR/src/timehud/timehud.svg" "$ICON_DIR/timehud.svg"
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache "$HOME/.local/share/icons/hicolor" -t || true
fi

# Install desktop entry
APP_DIR="$HOME/.local/share/applications"
mkdir -p "$APP_DIR"
# Substitute the absolute path to the launcher in the Exec line
sed "s|Exec=timehud|Exec=$LAUNCHER|g" "$SCRIPT_DIR/app/timehud.desktop" > "$APP_DIR/timehud.desktop"

echo ""
echo "✅  Done!  Run the overlay with:"
echo "   $LAUNCHER"
echo ""
echo "   Optional flags:"
echo "   --position top-left|top-right|bottom-left|bottom-right|top-center|bottom-center"
echo "   --reset-config"
