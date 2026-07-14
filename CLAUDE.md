# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

TimeHUD is a PyQt6 transparent HUD overlay for Linux showing a clock plus stopwatch/countdown timer above fullscreen apps. Dependencies: PyQt6 (required), pynput (optional `hotkeys` extra, enables global hotkeys).

## Commands

```bash
# Set up: venv + editable install + ./timehud launcher
bash install.sh

# Development install (pytest + ruff)
pip install -e ".[dev]"

# Run
./timehud                          # launcher (uses .venv)
timehud                            # console script (when installed)
# Useful flags: --position <preset>, --reset-config, --wayland, --no-tray, --version

# Tests (Qt-free, run headless)
pytest                             # full suite
pytest tests/test_timer_engine.py -v                    # one file
pytest tests/test_timer_engine.py::TestCountdown -v     # one class

# Lint
ruff check src tests

# Headless smoke test (exit 124 = ran fine until timeout)
QT_QPA_PLATFORM=offscreen timeout 5 python -m timehud.main --no-tray

# Build AppImage locally (separate .venv-appimage, python-appimage)
./build.sh
```

Releases: pushing a `v*` tag triggers `.github/workflows/build-appimage.yml` (builds AppImage, creates GitHub Release). The AppImage recipe bundles the package via `local+timehud` in `app/requirements.txt` + PYTHONPATH — it does not use pyproject, so packaging changes don't affect `build.sh`/`app/`.

## Architecture

Application code in `src/timehud/` (run as `python -m timehud.main` or `timehud` script):

- **`main.py`** — entry point. Forces `QT_QPA_PLATFORM=xcb` (XWayland) *before* Qt imports unless `--wayland` is passed — required for `X11BypassWindowManagerHint` (always-on-top over fullscreen) on Wayland sessions. Owns the system tray icon and pynput global-hotkey registration; calls `window.toggle_timer/reset_timer/toggle_visibility`, so those overlay method names are a contract.
- **`timer_engine.py`** — `TimerEngine`, the Qt-free timer state machine (stopwatch/countdown/interval modes, pause/resume, beep scheduling). The UI calls `tick()` every 100 ms and gets a `TickResult(display, state, beeps, finished, restarted, phase, round, rounds, progress)`; state is `"run" | "pause" | "warn" | "end"`. Injectable `clock` parameter makes tests deterministic. **Keep this module free of Qt imports** — the test suite depends on it running headless.
- **`themes.py`** — frozen `Theme` dataclass registry (classic/terminal/glass/mono), Qt-free. `apply_theme` stamps colors into config; structure (radius, alphas, scales) is read live via `get_theme(config.theme)`. Classic must stay pixel-identical to the pre-theme look — guarded by `tests/test_themes.py`.
- **`overlay.py`** — `OverlayWindow`: frameless translucent always-on-top widget. Renders `TickResult` (state→color mapping), forwards `Beep` events to `SoundManager`, handles drag/wheel/keyboard, and builds the context menu (shared by window right-click and tray — the tray menu is built once, the window menu per-open).
- **`config.py`** — `Config` dataclass persisted as JSON at `~/.config/timehud/config.json`. `Config.load()` filters unknown keys, so adding a field only requires a new dataclass attribute with a default (old configs keep loading — preserve this). Presets live here (`presets` list of `{"name", "duration"}`, `active_preset`), with module-level `valid_presets()` filtering malformed entries.
- **`settings_dialog.py`** — tabbed `SettingsDialog` with live updates: changes apply to the running overlay as controls change (`_connect_live_updates`), not only on Apply. `select_tab("presets")` opens a specific tab (used by "Manage presets…").
- **`sound_manager.py`** — generates beep WAVs with stdlib only, shells out to first available player (paplay → aplay → ffplay → mpv). No Python audio dependency.

Packaging: `pyproject.toml` (setuptools src layout, dynamic version); `app/` is the AppDir skeleton and `scripts/prepare_appdir.sh` shapes it for python-appimage.

## Conventions

- Version lives in `src/timehud/__init__.py` (`__version__`), read dynamically by pyproject; AppImage version comes from git tags in `build.sh`.
- Behavior quirks in `TimerEngine` (warn window only when remaining > 6 s; auto-restart not resetting `_sound_alert_before_beats`; display reverting to full duration one tick after countdown finish) are deliberate 1:1 ports from the original overlay code, pinned by tests — don't "fix" without intent.
- Keep the README settings-JSON example in sync with `Config` dataclass fields when adding config keys.
- `active_preset` must be cleared whenever `countdown_duration` changes by means other than applying a preset (wheel handler and settings dialog already do this).
- Interval mode: trailing rest skipped, no periodic `sound_interval` beeps, last-5 shorts only (no long at 1) — pinned by tests.
