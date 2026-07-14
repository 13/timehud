"""
themes.py – Built-in visual themes for TimeHUD.

Pure Python, no Qt. A theme bundles default colors plus structural styling
(radius, alphas, font scales). Selecting a theme stamps its colors into the
Config color fields; structure is read live via get_theme(config.theme).
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Theme:
    name: str            # registry key
    label: str           # menu text
    font_family: str     # digit font; "" = keep config.font_family
    color_bg: str
    bg_alpha: int        # 0-255 painted background alpha
    border_alpha: int    # 0 = no border
    top_edge_alpha: int  # lighter 1px inner top line (Glass); 0 = off
    radius: int
    color_clock: str
    clock_alpha: float   # 0-1 clock label opacity
    color_timer_run: str
    color_timer_pause: str
    color_warn: str
    color_end: str
    color_rest: str
    show_separator: bool
    clock_scale: float   # clock px = font_size * clock_scale
    timer_scale: float   # timer px = font_size * timer_scale


THEMES: dict[str, Theme] = {
    "classic": Theme(
        name="classic", label="Classic",
        font_family="",
        color_bg="#000000", bg_alpha=185,
        border_alpha=38, top_edge_alpha=0, radius=13,
        color_clock="#00FF88", clock_alpha=1.0,
        color_timer_run="#FFFFFF", color_timer_pause="#888888",
        color_warn="#FF9900", color_end="#FF3333", color_rest="#4FA8FF",
        show_separator=True, clock_scale=1.0, timer_scale=1.25,
    ),
    "terminal": Theme(
        name="terminal", label="Terminal",
        font_family="Monospace",
        color_bg="#0A0C0B", bg_alpha=199,
        border_alpha=26, top_edge_alpha=0, radius=14,
        color_clock="#4ADE80", clock_alpha=0.92,
        color_timer_run="#F4F6F5", color_timer_pause="#6E7672",
        color_warn="#FBBF24", color_end="#FF3333", color_rest="#60A5FA",
        show_separator=True, clock_scale=0.9, timer_scale=1.3,
    ),
    "glass": Theme(
        name="glass", label="Glass",
        font_family="Noto Sans",
        color_bg="#101214", bg_alpha=168,
        border_alpha=36, top_edge_alpha=56, radius=18,
        color_clock="#E8EDEB", clock_alpha=0.68,
        color_timer_run="#FFFFFF", color_timer_pause="#8A9490",
        color_warn="#FBBF24", color_end="#FF3333", color_rest="#93C5FD",
        show_separator=False, clock_scale=0.8, timer_scale=1.4,
    ),
    "mono": Theme(
        name="mono", label="Mono",
        font_family="Monospace",
        color_bg="#000000", bg_alpha=217,
        border_alpha=0, top_edge_alpha=0, radius=10,
        color_clock="#FFFFFF", clock_alpha=0.55,
        color_timer_run="#FFFFFF", color_timer_pause="#7A7A7A",
        color_warn="#FBBF24", color_end="#FF3333", color_rest="#B0BEC5",
        show_separator=False, clock_scale=0.62, timer_scale=1.5,
    ),
}


def get_theme(name: str) -> Theme:
    """Look up a theme by name; unknown names fall back to classic."""
    return THEMES.get(name, THEMES["classic"])


def apply_theme(config, name: str) -> Theme:
    """Stamp a theme's colors into config and set config.theme."""
    t = get_theme(name)
    config.theme = t.name
    config.color_bg = t.color_bg
    config.color_clock = t.color_clock
    config.color_timer_run = t.color_timer_run
    config.color_timer_pause = t.color_timer_pause
    if t.font_family:
        config.font_family = t.font_family
    return t
