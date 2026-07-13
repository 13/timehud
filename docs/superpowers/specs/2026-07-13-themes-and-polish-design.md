# TimeHUD: Theme System + Visual Polish

**Date:** 2026-07-13
**Scope:** Idea 12 (visual polish), shaped by user decision: keep the current look
as the default theme and add the three mockup directions as selectable themes.
Mockups: https://claude.ai/code/artifact/4f8f9f25-9c20-4b56-ada5-5c75373fa4b5

## Goals

1. Four built-in themes: **Classic** (current look, unchanged, default),
   **Terminal** (A), **Glass** (B), **Mono** (C).
2. Shared polish for all themes: tabular digits, ~200 ms color fades,
   auto-hide controls, last-seconds pulse.
3. Existing users see zero change until they pick a theme (`theme: "classic"`
   default; old configs load unchanged).

## 1. Theme registry

New module `src/timehud/themes.py`, no Qt imports (testable):

```python
@dataclass(frozen=True)
class Theme:
    name: str            # registry key
    label: str           # menu text
    font_family: str     # digit font (empty = config font_family)
    color_bg: str
    bg_alpha: int        # 0-255, painted background alpha
    border_alpha: int    # 0 = no border
    top_edge_alpha: int  # Glass: lighter 1px inner top line; 0 = off
    radius: int
    color_clock: str
    clock_alpha: float   # 0-1 label opacity
    color_timer_run: str
    color_timer_pause: str
    color_warn: str      # classic #FF9900, new themes #FBBF24
    color_end: str       # #FF3333 everywhere
    show_separator: bool
    clock_scale: float   # clock px = font_size * clock_scale
    timer_scale: float   # timer px = font_size * timer_scale

THEMES: dict[str, Theme] = {...}   # classic, terminal, glass, mono
def get_theme(name: str) -> Theme  # unknown name -> classic
```

Values:

| | classic | terminal (A) | glass (B) | mono (C) |
|---|---|---|---|---|
| font_family | "" (config) | Monospace | Sans ("Noto Sans"/system) | Monospace |
| color_bg / alpha | #000000 / 185 | #0A0C0B / 199 | #101214 / 168 | #000000 / 217 |
| border_alpha / top_edge | 38 / 0 | 26 / 0 | 36 / 56 | 0 / 0 |
| radius | 13 | 14 | 18 | 10 |
| clock color / alpha | #00FF88 / 1.0 | #4ADE80 / 0.92 | #E8EDEB / 0.68 | #FFFFFF / 0.55 |
| timer run / pause | #FFFFFF / #888888 | #F4F6F5 / #6E7672 | #FFFFFF / #8A9490 | #FFFFFF / #7A7A7A |
| separator | yes | yes | no | no |
| clock_scale / timer_scale | 1.0 / 1.25 | 0.9 / 1.3 | 0.8 / 1.4 | 0.62 / 1.5 |

Warn (#FF9900→theme-agnostic amber #FBBF24 for the three new themes, classic
keeps #FF9900) and end (#FF3333) stay module constants in overlay, except:
new themes use amber `_TMR_WARN_SOFT = "#FBBF24"`. Simplest: add `color_warn`
to Theme; classic = #FF9900, others #FBBF24. `color_end` likewise (#FF3333 all).

## 2. Config and application

- `Config.theme: str = "classic"` (new field; loader compat is automatic).
- Selecting a theme **stamps** its colors into the existing config fields
  (`color_bg`, `color_clock`, `color_timer_run`, `color_timer_pause`) and sets
  `config.theme`. Structural values (radius, alphas, scales, separator, font)
  are read live from `get_theme(config.theme)` at build/paint time.
- Users can still tweak individual colors afterwards in Settings (custom
  colors sit on top of the stamped values; structure stays the theme's).
- UI: Theme selector in Settings → Display tab (combo, live-update like other
  controls) + context-menu submenu **🎨 Theme** with checkmark on active.

## 3. Shared polish (all themes)

1. **Tabular digits** — on PyQt6 ≥ 6.7 set the OpenType `tnum` feature on the
   digit fonts (`QFont.setFeatures`/`Tag("tnum")`); wrap in try/except and fall
   back silently (mono themes are naturally tabular anyway).
2. **Color fades** — timer label color transitions animate over 200 ms using a
   `QVariantAnimation` between QColors, restarted when the engine state's
   mapped color changes. Direct set (no animation) on the final warn-flash
   seconds so flashes stay crisp.
3. **Auto-hide controls** — when `show_controls` is on: controls fade to 0
   opacity (QGraphicsOpacityEffect + QPropertyAnimation, 300 ms) 2 s after the
   pointer leaves the window; fade back in on enter. Never hidden while paused
   at 00:00 (idle state keeps buttons visible for discoverability).
4. **Last-seconds pulse** — on each last-5 beep second the timer font scales
   1.0 → 1.06 → 1.0 over ~180 ms (QVariantAnimation on pixel size), synced
   with the existing color flash.

## 4. Out of scope

Real drop shadows and backdrop blur (compositor-dependent on X11); theme
import/export; per-theme sound sets.

## Error handling

- Unknown `theme` value in config → `get_theme` returns classic.
- Font missing → Qt family fallback, no crash.

## Testing

- `tests/test_themes.py` (no Qt): registry completeness (all four themes, all
  fields), `get_theme` fallback, classic theme values match current constants
  (`#00FF88`, alpha 185, radius 13, scales 1.0/1.25 — guards the "zero change
  for existing users" promise).
- Behavior polish is Qt-side: offscreen smoke run + live-display screenshot
  check (available on this machine).

## Success criteria

- Fresh run with old config → looks pixel-identical to today (classic).
- Theme switch from context menu or Settings restyles live, persists.
- `pytest` green; ruff clean.
