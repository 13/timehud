# TimeHUD: Countdown Presets + Engine Split, Packaging, Tests

**Date:** 2026-07-13
**Scope:** Ideas 7 (presets), 9 (timer engine extraction), 10 (pyproject packaging), 11 (test suite).
Explicitly out of scope: interval/round mode, visual redesign (progress ring, typography).
Mockup: https://claude.ai/code/artifact/30edcc50-cbcc-45d3-8f58-7d3c661db9b4

## Goals

1. Switch countdown durations mid-workout in one click (no settings dialog).
2. Make timer logic unit-testable and shrink `overlay.py` (713 lines → UI only).
3. Standard Python packaging: `pip install -e .`, console entry point, ruff, pytest.

## 1. Presets (idea 7)

### Data model

`Config` gains one field (forward-compatible loading already filters unknown keys,
so old configs keep working):

```python
presets: list = field(default_factory=lambda: [
    {"name": "1 min", "duration": 60},
    {"name": "5 min", "duration": 300},
])
```

A preset is `{"name": str, "duration": int_seconds}`. Countdown-only — a
stopwatch has no parameters, so the menu offers a plain "Stopwatch" switch
instead of stopwatch presets. `Config` also gains `active_preset: str = ""`
(name of the currently applied preset; cleared when the user changes the
countdown duration by other means).

### UI

Context menu (window right-click and tray share `create_context_menu`) gets a
**Presets** submenu between Settings and Click-Through:

- **Stopwatch** — switches `timer_mode` to stopwatch, resets timer.
- One item per preset, formatted `"{name}   MM:SS"`, checkmark on the active
  one. Selecting: sets `timer_mode="countdown"`, `countdown_duration`,
  `active_preset`, resets the timer, saves config. Timer does **not**
  auto-start (matches existing behavior; single click/Space starts).
- **Save current as preset…** — `QInputDialog.getText` for the name; saves the
  current countdown duration. Only enabled in countdown mode.
- **Manage presets…** — opens the settings dialog on a new **Presets** tab:
  list of presets with Add / Remove / Rename / duration spinbox, same live-
  apply pattern as the other tabs.

Mode label under the timer shows the active preset name when set:
`"5 MIN · COUNTDOWN"` instead of `"COUNTDOWN 05:00"`.

## 2. Timer engine extraction (idea 9)

New module `src/timehud/timer_engine.py`, **no Qt imports**. It owns all state
currently spread through `OverlayWindow`: `_running`, `_elapsed`,
`_cd_remaining`, `_start_mono`, `_sound_beats`, `_sound_alert_before_beats`,
`_last_short_beep_sec`.

```python
class TimerEngine:
    def __init__(self, config, clock=time.monotonic): ...
    # commands
    def toggle(self) -> None
    def reset(self) -> None
    def set_mode(self, mode: str) -> None
    # queries
    running: bool
    def elapsed(self) -> float
    def remaining(self) -> float
    # tick — evaluated by the 100 ms UI timer
    def tick(self) -> TickResult
```

`TickResult` is a small dataclass carrying render/side-effect decisions the
overlay currently computes inline in `_update()`:

```python
@dataclass
class TickResult:
    display: float          # seconds to render
    state: str              # "run" | "pause" | "warn" | "end"
    beeps: list[Beep]       # e.g. [Beep(short=True)], empty most ticks
    finished: bool          # countdown hit zero this tick
    restarted: bool         # auto_restart_countdown kicked in
```

All beep scheduling (interval beats, `sound_alert_before` double-beep,
last-5-seconds short/long beeps) moves into `tick()`. `OverlayWindow._update()`
becomes: call `tick()`, map `state` to a color, set label text, forward `beeps`
to `SoundManager`. Injectable `clock` makes tests deterministic.

`overlay.py` keeps: widgets, painting, mouse/keyboard handling, menu, and the
thin `_update()` renderer. Expected size ~450 lines.

## 3. Packaging (idea 10)

New `pyproject.toml` (setuptools, src layout):

- `[project]`: name `timehud`, version sourced from `timehud.__version__`
  (`[tool.setuptools.dynamic]`), `dependencies = ["PyQt6>=6.11"]`,
  `[project.optional-dependencies] hotkeys = ["pynput>=1.8.1"]`,
  `dev = ["pytest", "ruff"]`.
- `[project.scripts] timehud = "timehud.main:main"`.
- `[tool.ruff]`: line-length 100, default rule set.
- `[tool.pytest.ini_options]`: `testpaths = ["tests"]`.
- Package data: include `timehud.svg`.

`install.sh` switches to `pip install -e ".[hotkeys]"` and the generated
launcher calls `.venv/bin/timehud` (no PYTHONPATH). `build.sh` installs the
project into the AppImage venv the same way; `requirements.txt` stays as a
convenience for the manual-install path in the README. `app/entrypoint.sh`
unchanged (module invocation still works since the package is installed).

## 4. Tests (idea 11)

`tests/` with pytest, no Qt required:

- `test_timer_engine.py` — fake clock fixture; covers: stopwatch accumulation
  across pause/resume, countdown reaching zero stops (and auto-restart variant),
  reset semantics, interval beep at each `sound_interval` boundary,
  `sound_alert_before` double-beep timing, last-5-seconds beep sequence
  (short at 5..2, long at 1), no beeps when `sound_enabled` is false.
- `test_config.py` — save/load round trip in `tmp_path` (monkeypatched
  `CONFIG_PATH`), unknown-key filtering, presets default + custom round trip.

Run: `pytest` (after `pip install -e ".[dev]"`).

## Error handling

- Malformed preset entries in config.json (missing keys, wrong types) are
  skipped at menu-build time; config load already falls back to defaults on
  parse errors.
- Duplicate preset names allowed but "Save current as preset…" overwrites an
  existing preset with the same name (simplest predictable rule).

## Implementation order

1. pyproject + install.sh/build.sh updates (everything after this is testable).
2. TimerEngine extraction + tests (behavior-preserving refactor).
3. Presets: config field → context menu → settings tab → tests.

## Success criteria

- `pytest` green; app behaves identically for existing users (old config loads).
- `pip install -e .` + `timehud` command launches the overlay.
- Preset switch from right-click menu changes countdown in ≤2 clicks.
