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

    # ── Interval mode ──────────────────────────────────────────────────────
    interval_work: int = 40     # seconds of work per round
    interval_rest: int = 20     # seconds of rest (0 = back-to-back rounds)
    interval_rounds: int = 8

    # ── Cycling stopwatch (0 = plain stopwatch) ────────────────────────────
    stopwatch_work: int = 0   # seconds of work per cycle while counting up
    stopwatch_rest: int = 0   # seconds of rest per cycle
    phase_beeps: bool = True   # long beep when a work/rest phase ends
    halfway_beep: bool = False # fast double beep at half of each work phase

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

    # ── Presets ────────────────────────────────────────────────────────────
    # Countdown presets shown in the right-click menu.
    presets: list = field(default_factory=lambda: [
        {"name": "1 min", "duration": 60},
        {"name": "5 min", "duration": 300},
    ])
    active_preset: str = ""   # name of the applied preset, "" = none

    # ── Theme ──────────────────────────────────────────────────────────────
    theme: str = "classic"   # built-in theme name; see timehud/themes.py
    progress_style: str = "line"   # countdown/interval progress: line | border | off
    padding: int = 12        # space between window border and content, px
    padding_top: int = -1    # top padding override, px; -1 = same as padding

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


def _is_int(value, minimum: int = 0) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= minimum


def valid_presets(presets: list) -> list:
    """Filter out malformed preset entries (defensive against hand-edited config).

    Three shapes:
      countdown: {"name": str, "duration": int > 0}
      interval:  {"name": str, "type": "interval",
                  "work": int > 0, "rest": int >= 0, "total": int >= work}
      stopwatch: {"name": str, "type": "stopwatch",
                  "work": int > 0, "rest": int >= 0}
                 — cycles work/rest like interval, but counts upward forever

    All types may carry optional sound rules, applied to the config when the
    preset is selected: "last5" (bool → alert_last_5_seconds), "boundary"
    (bool → phase_beeps, long beep at phase end, default on) and "halfway"
    (bool → halfway_beep, fast double beep at half of each work phase).
    Legacy "every"/"before" keys are ignored.
    """
    out = []
    for p in presets:
        if not isinstance(p, dict) or not isinstance(p.get("name"), str):
            continue
        if "last5" in p and not isinstance(p["last5"], bool):
            continue
        if "boundary" in p and not isinstance(p["boundary"], bool):
            continue
        if "halfway" in p and not isinstance(p["halfway"], bool):
            continue
        if p.get("type") == "stopwatch":
            if _is_int(p.get("work"), 1) and _is_int(p.get("rest"), 0):
                out.append(p)
        elif p.get("type") == "interval":
            if (
                _is_int(p.get("work"), 1)
                and _is_int(p.get("rest"), 0)
                and _is_int(p.get("total"), 1)
                and p["total"] >= p["work"]
            ):
                out.append(p)
        elif _is_int(p.get("duration"), 1):
            out.append(p)
    return out


def interval_preset_rounds(preset: dict) -> int:
    """Rounds that fit into the preset's total time (at least one)."""
    cycle = preset["work"] + preset["rest"]
    return max(1, preset["total"] // cycle)
