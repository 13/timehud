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
from PyQt6.QtWidgets import QSystemTrayIcon, QMenu, QStyle
from PyQt6.QtGui import QIcon, QPixmap, QColor, QPainter, QPen
from PyQt6.QtCore import Qt

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

    # ── System Tray ────────────────────────────────────────────────────────
    # ── System Tray ────────────────────────────────────────────────────────
    def polar(cx, cy, angle_deg, radius):
        import math
        rad = math.radians(angle_deg)
        from PyQt6.QtCore import QPointF
        return QPointF(
            cx + radius * math.sin(rad),
            cy - radius * math.cos(rad)
        )

    pm = QPixmap(64, 64)
    pm.fill(Qt.GlobalColor.transparent)

    p = QPainter(pm)
    p.setRenderHint(QPainter.RenderHint.Antialiasing)

    # ── Constants ─────────────────────────────────────────
    cx, cy = 32, 32
    radius = 20

    green = QColor("#00FF88")
    dark = QColor("#0F1115")

    # ── Main body ─────────────────────────────────────────
    p.setBrush(dark)
    p.setPen(QPen(green, 2))
    p.drawEllipse(cx - radius, cy - radius, radius * 2, radius * 2)

    # ── HUD outer ring ────────────────────────────────────
    p.setPen(QPen(green, 1, Qt.PenStyle.DashLine))
    p.setOpacity(0.5)
    p.drawEllipse(cx - radius - 3, cy - radius - 3,
                  (radius + 3) * 2, (radius + 3) * 2)
    p.setOpacity(1.0)

    # ── Tick marks ────────────────────────────────────────
    p.setPen(QPen(green, 1))
    for angle in range(0, 360, 30):
        inner = polar(cx, cy, angle, radius - 4)
        outer = polar(cx, cy, angle, radius)
        p.drawLine(inner, outer)

    # ── Top stem ──────────────────────────────────────────
    p.setBrush(green)
    p.setPen(Qt.PenStyle.NoPen)
    #p.drawRect(cx - 6, cy - radius - 10, 12, 6)
    #p.drawRect(cx - 4, cy - radius - 4, 8, 6)

    # ── Side buttons (2 and 10 o’clock) ───────────────────
    for angle in (65, -65):
        pos = polar(cx, cy, angle, radius + 2)

        p.save()
        p.translate(pos)
        p.rotate(angle)
        p.drawRect(-4, -2, 8, 4)
        p.restore()

    # ── Hands ─────────────────────────────────────────────
    p.setPen(QPen(QColor("#FFFFFF"), 2))
    p.drawLine(polar(cx, cy, 0, 0), polar(cx, cy, 0, radius - 6))   # up
    p.drawLine(polar(cx, cy, 0, 0), polar(cx, cy, 90, radius - 10)) # right

    # ── Center pivot ──────────────────────────────────────
    p.setBrush(QColor("#FFFFFF"))
    p.setPen(Qt.PenStyle.NoPen)
    p.drawEllipse(cx - 2, cy - 2, 4, 4)

    # ── HUD crosshair ─────────────────────────────────────
    p.setPen(QPen(green, 1))
    p.setOpacity(0.4)
    p.drawLine(cx - 6, cy, cx + 6, cy)
    p.drawLine(cx, cy - 6, cx, cy + 6)

    p.end()

    tray_icon = None
    if cfg.show_tray_icon:
        tray_icon = QSystemTrayIcon(app)
        tray_icon.setIcon(QIcon(pm))
        tray_icon.setToolTip("TimeHUD")

        tray_menu = QMenu()
        act_toggle = tray_menu.addAction("Show/Hide Overlay")
        tray_menu.addSeparator()
        act_quit = tray_menu.addAction("Quit")
        tray_icon.setContextMenu(tray_menu)
        tray_icon.show()

    # Allow Ctrl+C in the terminal to terminate cleanly
    signal.signal(signal.SIGINT, signal.SIG_DFL)

    # ── Create & show overlay ──────────────────────────────────────────────
    window = OverlayWindow(cfg)
    window.show()

    # Sync tray menu to use the identical dynamically built menu from the app
    if tray_icon is not None:
        tray_menu = window.create_context_menu()
        tray_icon.setContextMenu(tray_menu)

        def _on_tray_activated(reason):
            if reason == QSystemTrayIcon.ActivationReason.Trigger:
                window.toggle_visibility()
        tray_icon.activated.connect(_on_tray_activated)

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
