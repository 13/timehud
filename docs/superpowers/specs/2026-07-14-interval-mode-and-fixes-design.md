# TimeHUD: Interval Mode, Progress Bar, CI, UX Fixes

**Date:** 2026-07-14
**Scope:** User-selected batch: (1) CI test job, (2) interval training mode,
(3) countdown/interval progress bar, (5) tray-menu staleness, (6) settings
Cancel revert, (8) preset-list duration format.
Explicitly excluded: fixing the "display returns to full duration one tick
after countdown finish" quirk (user chose to keep it).

## 1. CI test job

`.github/workflows/test.yml`: on push to main + PRs; ubuntu-latest,
Python 3.12, `pip install -e ".[dev]"`, `ruff check src tests`, `pytest -q`.
Tests are Qt-free so no display/system packages needed (PyQt6 installs as a
dependency but is never imported by the suite).

## 2. Interval mode

Third `timer_mode = "interval"`: repeat `interval_work` seconds of WORK then
`interval_rest` seconds of REST for `interval_rounds` rounds. The trailing
rest of the last round is skipped (session ends on the final work phase).
`interval_rest = 0` means back-to-back work rounds.

**Config:** `interval_work: int = 40`, `interval_rest: int = 20`,
`interval_rounds: int = 8` (spinbox ranges 5–3600 / 0–3600 / 1–99).

**Engine (`timer_engine.py`, stays Qt-free):**
- New state: `_phase: str` ("work"|"rest"), `_round: int` (1-based).
  Phase countdown reuses the `_cd_remaining`/`_start_mono` mechanics.
- `TickResult` gains: `phase: str = ""` (empty outside interval mode),
  `round: int = 0`, `rounds: int = 0`, and `progress: float = -1.0`
  (fraction of the current phase/countdown remaining, 0..1; -1 = no bar).
- Transitions on phase expiry (overshoot ignored, same as auto-restart):
  work→rest emits `Beep(double=True)`; rest→work (and work→work when
  rest = 0) emits `Beep()` (long); after work of the final round: `finished`,
  state `"end"`, engine stops.
- Last-5-seconds handling inside a phase: same shorts at displayed 6..2 with
  the same dedup, but NO long beep at 1 (the transition beep covers it) and
  only when `alert_last_5_seconds` is on.
- The periodic `sound_interval` beep block is skipped entirely in interval
  mode (phase beeps replace it). The `sound_alert_before` warn-window logic
  also does not apply in interval mode.
- `is_idle()`: interval is idle when stopped at work/round 1/full work time.
- `progress`: countdown → `remaining / countdown_duration`; interval →
  `remaining / current_phase_duration`; stopwatch → -1.

**Overlay:**
- Mode cycling (button + both wheel paths) becomes a 3-cycle:
  stopwatch → countdown → interval. Button text "IV" in interval mode.
- Mode label: idle `INTERVAL {work}s/{rest}s ×{rounds}`; while running/paused
  the label shows `WORK {round}/{rounds}` or `REST {round}/{rounds}`
  (updated from tick data, only on change).
- Rest phase timer color: new `Theme.color_rest` (classic `#4FA8FF`,
  terminal `#60A5FA`, glass `#93C5FD`, mono `#B0BEC5`); work phase uses the
  normal run color. Warn/end colors unchanged.

**Settings Timer tab:** mode combo gains "interval"; three spinboxes
(Work s / Rest s / Rounds), live-updating like the rest of the tab.
Presets remain countdown-only (unchanged).

## 3. Progress bar

Thin (3 px) rounded bar between timer label and mode label, full width.
Custom `QWidget` subclass in `overlay.py` with `set_progress(fraction,
color)`; hidden when fraction < 0 (stopwatch, or timer hidden).
Fill depletes right-to-left with remaining fraction. Color: `theme.color_warn`
when state is warn/end, else `theme.color_rest` during rest, else
`theme.color_clock`. Track color `rgba(255,255,255,30)`.

## 5. Tray menu staleness

`create_context_menu` splits into a `_populate_context_menu(menu,
include_window_actions)` body + thin wrapper. `main.py` gives the tray a
persistent `QMenu` and connects its `aboutToShow` to clear + repopulate, so
preset/theme/opacity/position checkmarks are always current.

## 6. Settings Cancel revert

`_open_settings` snapshots the config (`copy.deepcopy(asdict(config))`)
before `dlg.exec()`. On reject, restore every field, run the same UI-refresh
path, and save. Live-apply behavior while the dialog is open is unchanged.

## 8. Duration format

Current `_fmt` already implements the wanted rule (MM:SS below one hour,
HH:MM:SS from one hour). Move it to `timer_engine.fmt_seconds` (Qt-free,
testable), alias in overlay, and use it in the settings presets list
(replacing the `divmod` MM:SS-only formatting that showed `1440:00`).

## Error handling / compat

- Old configs: new fields have defaults; loader filters unknown keys.
- Mode value guard: unknown `timer_mode` in config behaves as stopwatch
  (existing `modes.index` fallback pattern preserved in the 3-mode lists).
- Theme registry gains one field: update classic-guard test accordingly
  (classic look otherwise unchanged; bar + rest color are new surfaces).

## Testing

- Engine: interval transitions (work→rest→work, rest=0 skip, final-rest
  skip, finish), round counting, beep kinds per transition, no periodic
  beeps in interval mode, last-5 shorts without the long-at-1, progress
  values in all three modes, `is_idle` for interval, pause/resume mid-phase.
- `fmt_seconds`: 59 s → "00:59", 61 → "01:01", 3600 → "01:00:00",
  86399 → "23:59:59".
- Themes: `color_rest` present all four themes; classic guard updated.
- Qt-side (tray rebuild, cancel revert, bar painting): headless functional
  checks + live screenshots (controller).

## Success criteria

- CI green on push; 3 modes cycle correctly; interval session runs
  40/20×8 with correct beeps, colors, round display, bar; tray menu always
  current; Cancel restores pre-dialog state; presets tab shows 05:00-style
  durations. Classic non-interval rendering unchanged.
