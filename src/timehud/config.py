"""
config.py – Persistent settings for TimeHUD.
Stored as JSON at ~/.config/timehud/config.json
"""

import json
import os
from dataclasses import dataclass, asdict, field

CONFIG_PATH = os.path.expanduser("~/.config/timehud/config.json")


@dataclass
class Config:
    # ── Window position ────────────────────────────────────────────────────
    # Preset: top-left | top-right | bottom-left | bottom-right |
    #         top-center | bottom-center
    position: str = "top-right"
    custom_x: int = -1   # -1 = use preset
    custom_y: int = -1
    margin: int = 15     # pixels from screen edge

    # ── Display ────────────────────────────────────────────────────────────
    opacity: float = 0.88
    font_size: int = 30        # px, applied to clock/timer labels
    font_family: str = "Monospace"
    color_bg: str = "#000000"
    color_clock: str = "#00FF88"
    color_timer_run: str = "#FFFFFF"
    color_timer_pause: str = "#888888"
    show_clock: bool = True    # system time widget
    show_timer: bool = True    # stopwatch / countdown widget

    # ── Timer ──────────────────────────────────────────────────────────────
    # "stopwatch" | "countdown"
    timer_mode: str = "stopwatch"
    countdown_duration: int = 300   # seconds (default 5 min)

    # ── Sound ──────────────────────────────────────────────────────────────
    sound_enabled: bool = True
    sound_interval: int = 60   # play alert every N seconds of running time
    sound_alert_before: int = 0  # play short beep N seconds before main alert, 0 to disable
    sound_file: str = ""       # empty = built-in generated beep

    # ── Behaviour ──────────────────────────────────────────────────────────
    show_tray_icon: bool = True   # display icon in system tray
    show_controls: bool = True    # display overlay control buttons [▶] [↺] [SW]
    click_through: bool = False   # window ignores mouse input when True
    auto_restart_countdown: bool = False
    alert_last_5_seconds: bool = False

    # ──────────────────────────────────────────────────────────────────────

    def save(self) -> None:
        os.makedirs(os.path.dirname(CONFIG_PATH), exist_ok=True)
        with open(CONFIG_PATH, "w") as fh:
            json.dump(asdict(self), fh, indent=2)

    @classmethod
    def load(cls) -> "Config":
        """Load from disk; fall back to defaults on any error."""
        try:
            with open(CONFIG_PATH) as fh:
                data = json.load(fh)
            # Only pass keys that exist in the dataclass (forward-compat)
            valid_keys = cls.__dataclass_fields__.keys()
            return cls(**{k: v for k, v in data.items() if k in valid_keys})
        except Exception:
            return cls()
