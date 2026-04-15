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

echo ""
echo "✅  Done!  Run the overlay with:"
echo "   $LAUNCHER"
echo ""
echo "   Optional flags:"
echo "   --position top-left|top-right|bottom-left|bottom-right|top-center|bottom-center"
echo "   --reset-config"
