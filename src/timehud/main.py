#!/usr/bin/env python3
"""
main.py – TimeHUD entry point.

Usage:
    python main.py
    python main.py --position bottom-right
    python main.py --reset-config

Global hotkeys (requires pynput):
    Ctrl+Shift+Space  – start / pause timer
    Ctrl+Shift+R      – reset timer
    Ctrl+Shift+H      – toggle overlay visibility

Local keyboard shortcuts (overlay window focused):
    Space  – start / pause timer
    R      – reset timer
    Escape – hide overlay
    Ctrl+Q – quit
"""

import argparse
import os
import signal
import sys

# ── Force X11 backend so the overlay works on both pure X11 and XWayland ──
# On a Wayland session Qt would normally pick the Wayland backend which
# does NOT support X11BypassWindowManagerHint.  XWayland provides full X11
# compatibility.  Pass --wayland to disable this if you know what you're doing.
if "--wayland" not in sys.argv:
    os.environ.setdefault("QT_QPA_PLATFORM", "xcb")

from PyQt6.QtWidgets import QApplication  # noqa: E402 (after env var)
from PyQt6.QtWidgets import QSystemTrayIcon
from PyQt6.QtGui import QIcon

from timehud.config import Config           # noqa: E402
from timehud.overlay import OverlayWindow   # noqa: E402


# ── Global hotkeys ─────────────────────────────────────────────────────────

def _setup_hotkeys(window: OverlayWindow):
    """Register system-wide hotkeys via pynput (optional dependency)."""
    try:
        from pynput import keyboard  # type: ignore

        hotkeys = keyboard.GlobalHotKeys({
            "<ctrl>+<shift>+<space>": window.toggle_timer,
            "<ctrl>+<shift>+r":     window.reset_timer,
            "<ctrl>+<shift>+h":     window.toggle_visibility,
        })
        hotkeys.start()
        return hotkeys
    except ImportError:
        print(
            "[hotkeys] pynput not installed – global hotkeys disabled.\n"
            "          Install with:  pip install pynput"
        )
        return None
    except Exception as exc:
        print(f"[hotkeys] Could not register hotkeys: {exc}")
        return None


# ── Main ───────────────────────────────────────────────────────────────────

def main() -> None:
    from timehud import __version__
    parser = argparse.ArgumentParser(
        description="TimeHUD – Lightweight Linux HUD overlay"
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"TimeHUD {__version__}"
    )
    parser.add_argument(
        "--position",
        choices=[
            "top-left", "top-right",
            "bottom-left", "bottom-right",
            "top-center", "bottom-center",
        ],
        help="Override window position preset",
    )
    parser.add_argument(
        "--reset-config",
        action="store_true",
        help="Reset all settings to defaults and exit",
    )
    parser.add_argument(
        "--wayland",
        action="store_true",
        help="Use native Wayland backend (experimental, overlay may not appear above fullscreen)",
    )
    parser.add_argument(
        "--no-tray",
        action="store_true",
        help="Disable system tray icon (overrides config)",
    )
    args = parser.parse_args()

    if args.reset_config:
        cfg = Config()
        cfg.save()
        print(f"Config reset → {Config.__module__}")
        return

    cfg = Config.load()
    if args.position:
        cfg.position  = args.position
        cfg.custom_x  = -1
        cfg.custom_y  = -1
    if args.no_tray:
        cfg.show_tray_icon = False

    # ── Qt application ─────────────────────────────────────────────────────
    app = QApplication(sys.argv)
    app.setApplicationName("TimeHUD")
    # Keep running even when the settings dialog is the only open window
    app.setQuitOnLastWindowClosed(False)

    # Allow Ctrl+C in the terminal to terminate cleanly
    signal.signal(signal.SIGINT, signal.SIG_DFL)

    # ── Create & show overlay ──────────────────────────────────────────────
    tray_icon: QSystemTrayIcon | None = None

    def _set_tray_visibility(enabled: bool) -> None:
        nonlocal tray_icon

        # --no-tray is a runtime override for this process.
        if args.no_tray:
            enabled = False

        if enabled:
            if tray_icon is None:
                tray_icon = QSystemTrayIcon(app)
                icon_path = os.path.join(os.path.dirname(__file__), "timehud.svg")
                tray_icon.setIcon(QIcon(icon_path))
                tray_icon.setToolTip("TimeHUD")
                tray_icon.activated.connect(_on_tray_activated)

            tray_icon.setContextMenu(window.create_context_menu())
            tray_icon.show()
        elif tray_icon is not None:
            tray_icon.hide()

    window = OverlayWindow(cfg, on_tray_icon_toggle=_set_tray_visibility)
    window.show()

    def _on_tray_activated(reason):
        if reason == QSystemTrayIcon.ActivationReason.Trigger:
            window.toggle_visibility()

    _set_tray_visibility(cfg.show_tray_icon)

    # ── Optional global hotkeys ────────────────────────────────────────────
    hotkey_handler = _setup_hotkeys(window)

    # ── Event loop ─────────────────────────────────────────────────────────
    exit_code = app.exec()

    # ── Cleanup ────────────────────────────────────────────────────────────
    if hotkey_handler:
        hotkey_handler.stop()
    window.sound.cleanup()
    cfg.save()

    sys.exit(exit_code)


if __name__ == "__main__":
    main()
