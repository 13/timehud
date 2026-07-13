# Presets + Timer Engine + Packaging + Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Countdown presets reachable from the right-click menu, timer logic extracted into a Qt-free tested engine, and standard pyproject packaging.

**Architecture:** `TimerEngine` (new `src/timehud/timer_engine.py`, zero Qt imports) owns all timer state and beep scheduling; `OverlayWindow._update()` becomes a thin renderer of `TickResult`. Presets are a list in the existing `Config` dataclass, surfaced as a context-menu submenu and a settings tab. Packaging moves to `pyproject.toml` with a `timehud` console script.

**Tech Stack:** Python ≥3.10, PyQt6 ≥6.11, setuptools (src layout), pytest, ruff.

**Spec:** `docs/superpowers/specs/2026-07-13-presets-and-refactor-design.md`

## Global Constraints

- No Qt imports in `timer_engine.py` or in any test file. Tests must run headless.
- Behavior-preserving refactor: existing users see identical timing, colors, and beep behavior.
- Old `~/.config/timehud/config.json` files must keep loading (loader filters unknown keys; new fields have defaults).
- `requirements.txt` stays (manual-install path in README). `build.sh`, `app/requirements.txt`, `app/entrypoint.sh` are NOT modified — the AppImage recipe bundles via `local+timehud` + `PYTHONPATH=src` and is unaffected by pyproject.
- Preset shape: `{"name": str, "duration": int_seconds}`. Countdown-only.
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: pyproject packaging

**Files:**
- Create: `pyproject.toml`
- Modify: `install.sh` (venv setup + launcher)
- Modify: `README.md` (manual install section)

**Interfaces:**
- Produces: `pip install -e ".[dev]"` working from repo root; `timehud` console script; `pytest` configured with `testpaths=["tests"]` (used by every later task).

- [ ] **Step 1: Create `pyproject.toml`**

```toml
[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.build_meta"

[project]
name = "timehud"
dynamic = ["version"]
description = "Lightweight transparent HUD overlay for Linux — clock + stopwatch/countdown"
readme = "README.md"
requires-python = ">=3.10"
license = { text = "MIT" }
dependencies = ["PyQt6>=6.11.0"]

[project.optional-dependencies]
hotkeys = ["pynput>=1.8.1"]
dev = ["pytest>=8", "ruff>=0.4"]

[project.scripts]
timehud = "timehud.main:main"

[tool.setuptools.dynamic]
version = { attr = "timehud.__version__" }

[tool.setuptools.packages.find]
where = ["src"]

[tool.setuptools.package-data]
timehud = ["*.svg"]

[tool.ruff]
line-length = 100

[tool.pytest.ini_options]
testpaths = ["tests"]
```

Note: importing `timehud/__init__.py` for the version does not import PyQt6 (the
`__init__` only defines `__version__`), so building in a clean env works.

- [ ] **Step 2: Verify editable install + entry point**

```bash
python3 -m venv /tmp/thud-venv
/tmp/thud-venv/bin/pip install -q -e ".[dev]"
/tmp/thud-venv/bin/python -c "import timehud; print(timehud.__version__)"
/tmp/thud-venv/bin/timehud --version
```

Expected: prints `0.3.6` then `TimeHUD 0.3.6` (the `--version` action exits before any Qt window is created, so this works headless; PyQt6 import at module load must succeed).

- [ ] **Step 3: Update `install.sh`**

Replace the dependency-install and launcher-generation blocks:

```bash
echo "==> Installing dependencies …"
"$VENV/bin/pip" install --upgrade pip -q
"$VENV/bin/pip" install -e "$SCRIPT_DIR[hotkeys]"

# Create launcher script
LAUNCHER="$SCRIPT_DIR/timehud"
cat > "$LAUNCHER" << 'EOF'
#!/usr/bin/env bash
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$DIR/.venv/bin/timehud" "$@"
EOF
chmod +x "$LAUNCHER"
```

Everything else in `install.sh` (icon, desktop entry, echo block) stays.

- [ ] **Step 4: Run `bash install.sh` and `./timehud --version`**

Expected: install completes, `./timehud --version` prints `TimeHUD 0.3.6`.

- [ ] **Step 5: Update README "Manual install (no venv)" section**

Replace the code block:

```bash
pip install .          # or:  pip install .[hotkeys]  for global hotkeys
timehud
```

(The old `PYTHONPATH=src` invocation still works; keep it as a one-line
alternative for running from a checkout without installing:
`PYTHONPATH=src python -m timehud.main`.)

- [ ] **Step 6: Commit**

```bash
git add pyproject.toml install.sh README.md
git commit -m "build: add pyproject packaging with timehud entry point"
```

---

### Task 2: TimerEngine core (stopwatch/countdown state machine)

**Files:**
- Create: `src/timehud/timer_engine.py`
- Create: `tests/test_timer_engine.py`
- Create: `tests/conftest.py`

**Interfaces:**
- Consumes: `Config` dataclass fields `timer_mode`, `countdown_duration`, `sound_interval`, `sound_alert_before`, `sound_enabled`, `alert_last_5_seconds`, `auto_restart_countdown`.
- Produces (used by Tasks 3–4):
  - `TimerEngine(config, clock=time.monotonic)`
  - `engine.running: bool` (attribute)
  - `engine.elapsed() -> float`, `engine.remaining() -> float`
  - `engine.toggle() -> None`, `engine.reset() -> None`
  - `engine.set_mode(mode: str) -> None`
  - `engine.adjust_countdown(delta: float) -> None`
  - `engine.tick() -> TickResult` (Task 3 fills beeps; this task returns display/state/finished/restarted)
  - `TickResult(display: float, state: str, beeps: list[Beep], finished: bool, restarted: bool)` with `state` in `{"run","pause","warn","end"}`
  - `Beep(short: bool = False, double: bool = False)`

- [ ] **Step 1: Write failing tests + fake clock fixture**

`tests/conftest.py`:

```python
import pytest

from timehud.config import Config


class FakeClock:
    def __init__(self, start: float = 1000.0) -> None:
        self.now = start

    def __call__(self) -> float:
        return self.now

    def advance(self, secs: float) -> None:
        self.now += secs


@pytest.fixture
def clock() -> FakeClock:
    return FakeClock()


@pytest.fixture
def config() -> Config:
    return Config()
```

`tests/test_timer_engine.py`:

```python
import pytest

from timehud.timer_engine import TimerEngine


@pytest.fixture
def engine(config, clock):
    return TimerEngine(config, clock=clock)


class TestStopwatch:
    def test_starts_at_zero_paused(self, engine):
        assert engine.running is False
        assert engine.elapsed() == 0.0

    def test_accumulates_while_running(self, engine, clock):
        engine.toggle()
        clock.advance(12.5)
        assert engine.elapsed() == pytest.approx(12.5)

    def test_pause_freezes_and_resume_continues(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        engine.toggle()          # pause
        clock.advance(99)        # time passes while paused
        assert engine.elapsed() == pytest.approx(10)
        engine.toggle()          # resume
        clock.advance(5)
        assert engine.elapsed() == pytest.approx(15)

    def test_reset_zeroes(self, engine, clock):
        engine.toggle()
        clock.advance(30)
        engine.reset()
        assert engine.running is False
        assert engine.elapsed() == 0.0

    def test_tick_state_run_and_pause(self, engine, clock):
        assert engine.tick().state == "pause"
        engine.toggle()
        clock.advance(3)
        r = engine.tick()
        assert r.state == "run"
        assert r.display == pytest.approx(3)


class TestCountdown:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 60
        return TimerEngine(config, clock=clock)

    def test_counts_down(self, engine, clock):
        engine.toggle()
        clock.advance(25)
        assert engine.remaining() == pytest.approx(35)
        assert engine.tick().display == pytest.approx(35)

    def test_pause_resume(self, engine, clock):
        engine.toggle()
        clock.advance(20)
        engine.toggle()
        clock.advance(500)
        assert engine.remaining() == pytest.approx(40)

    def test_finish_stops_engine(self, engine, clock):
        engine.toggle()
        clock.advance(61)
        r = engine.tick()
        assert r.state == "end"
        assert r.finished is True
        assert r.display == 0.0
        assert engine.running is False
        # subsequent ticks stay ended but don't re-report finished
        assert engine.tick().finished is False

    def test_auto_restart(self, engine, clock):
        engine.config.auto_restart_countdown = True
        engine.toggle()
        clock.advance(61)
        r = engine.tick()
        assert r.restarted is True
        assert engine.running is True
        assert engine.remaining() == pytest.approx(60)

    def test_start_after_finish_reloads_duration(self, engine, clock):
        engine.toggle()
        clock.advance(61)
        engine.tick()
        engine.toggle()          # restart from full duration
        assert engine.remaining() == pytest.approx(60)

    def test_adjust_countdown_while_running(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        engine.config.countdown_duration += 60
        engine.adjust_countdown(60)
        assert engine.remaining() == pytest.approx(110)

    def test_adjust_countdown_while_stopped_reloads(self, engine):
        engine.config.countdown_duration = 90
        engine.adjust_countdown(30)
        assert engine.remaining() == pytest.approx(90)


class TestModeSwitch:
    def test_set_mode_resets(self, engine, clock):
        engine.toggle()
        clock.advance(42)
        engine.set_mode("countdown")
        assert engine.config.timer_mode == "countdown"
        assert engine.running is False
        assert engine.remaining() == pytest.approx(engine.config.countdown_duration)
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pytest tests/test_timer_engine.py -v`
Expected: collection error `ModuleNotFoundError: No module named 'timehud.timer_engine'`

- [ ] **Step 3: Implement `src/timehud/timer_engine.py`**

Port the state and semantics 1:1 from `overlay.py` (`__init__` lines 84–91, `_get_elapsed`/`_get_remaining`/`toggle_timer`/`reset_timer` lines 364–403, `_update` display logic lines 283–348). Beep emission is Task 3; `tick()` here computes display/state/finished/restarted only.

```python
"""
timer_engine.py – Timer state machine for TimeHUD.

Pure Python, no Qt. The overlay calls tick() every ~100 ms and renders
the returned TickResult; commands (toggle/reset/…) come from UI events.
The injectable `clock` (defaults to time.monotonic) makes tests deterministic.
"""

import math
import time
from dataclasses import dataclass, field


@dataclass
class Beep:
    short: bool = False    # short blip (last-5-seconds countdown)
    double: bool = False   # double beep (sound_alert_before warning)


@dataclass
class TickResult:
    display: float                      # seconds to render
    state: str                          # "run" | "pause" | "warn" | "end"
    beeps: list = field(default_factory=list)
    finished: bool = False              # countdown hit zero on this tick
    restarted: bool = False             # auto_restart_countdown kicked in


class TimerEngine:
    def __init__(self, config, clock=time.monotonic) -> None:
        self.config = config
        self._clock = clock
        self.running = False
        self._start_mono = 0.0    # clock() when last started
        self._elapsed = 0.0       # accumulated seconds (stopwatch)
        self._cd_remaining = float(config.countdown_duration)
        self._sound_beats = 0
        self._sound_alert_before_beats = 0
        self._last_short_beep_sec = -1

    # ── Queries ────────────────────────────────────────────────────────
    def elapsed(self) -> float:
        """Stopwatch: total elapsed seconds (in countdown mode: run time since resume)."""
        if self.running:
            return self._elapsed + (self._clock() - self._start_mono)
        return self._elapsed

    def remaining(self) -> float:
        """Countdown: seconds remaining."""
        if self.running:
            return self._cd_remaining - (self._clock() - self._start_mono)
        return self._cd_remaining

    # ── Commands ───────────────────────────────────────────────────────
    def toggle(self) -> None:
        """Start if stopped, pause if running."""
        if self.running:
            if self.config.timer_mode == "stopwatch":
                self._elapsed += self._clock() - self._start_mono
            else:
                self._cd_remaining -= self._clock() - self._start_mono
                self._cd_remaining = max(0.0, self._cd_remaining)
            self.running = False
        else:
            if self.config.timer_mode == "countdown" and self._cd_remaining <= 0:
                self._cd_remaining = float(self.config.countdown_duration)
            self._start_mono = self._clock()
            self.running = True
            self._sound_beats = int(self.elapsed() / self.config.sound_interval)
            self._sound_alert_before_beats = int(
                (self.elapsed() + self.config.sound_alert_before)
                / self.config.sound_interval
            )

    def reset(self) -> None:
        self.running = False
        self._elapsed = 0.0
        self._cd_remaining = float(self.config.countdown_duration)
        self._sound_beats = 0
        self._sound_alert_before_beats = 0
        self._last_short_beep_sec = -1

    def set_mode(self, mode: str) -> None:
        self.config.timer_mode = mode
        self.reset()

    def adjust_countdown(self, delta: float) -> None:
        """config.countdown_duration was changed by `delta` seconds; keep
        the live remaining time in sync (running) or reload it (stopped)."""
        if self.running:
            self._cd_remaining += delta
        else:
            self._cd_remaining = float(self.config.countdown_duration)

    # ── Tick ───────────────────────────────────────────────────────────
    def tick(self) -> TickResult:
        beeps: list[Beep] = []
        finished = False
        restarted = False

        # Warn window: sound_alert_before seconds before the next interval beep
        warn = False
        if self.running and self.config.sound_enabled and self.config.sound_alert_before > 0:
            ref = self.elapsed()
            next_beep = (int(ref / self.config.sound_interval) + 1) * self.config.sound_interval
            warn = next_beep - self.config.sound_alert_before <= ref < next_beep

        if self.config.timer_mode == "stopwatch":
            display = self.elapsed()
            state = "run" if self.running else "pause"
            if state == "run" and warn:
                state = "warn"
        else:
            remaining = self.remaining()
            display = max(0.0, remaining)
            sec_display = int(math.ceil(display))
            if remaining <= 0:
                state = "end"
                if self.running:
                    finished = True
                    if self.config.auto_restart_countdown:
                        self._cd_remaining = float(self.config.countdown_duration)
                        self._start_mono = self._clock()
                        self._sound_beats = 0
                        self._last_short_beep_sec = -1
                        restarted = True
                    else:
                        self.running = False
            elif self.running and remaining <= 6.0 and self.config.alert_last_5_seconds:
                state = "end" if sec_display == 1 else "warn"
                if sec_display != self._last_short_beep_sec and 1 <= sec_display <= 6:
                    self._last_short_beep_sec = sec_display
                    beeps.append(Beep(short=sec_display != 1))
            else:
                state = "run" if self.running else "pause"
                if state == "run" and warn and remaining > 6.0:
                    state = "warn"

        # Periodic interval beeps
        if self.running and self.config.sound_enabled:
            ref = self.elapsed()
            if self.config.sound_alert_before > 0:
                target = (
                    (self._sound_alert_before_beats + 1) * self.config.sound_interval
                    - self.config.sound_alert_before
                )
                if target > 0 and ref >= target:
                    self._sound_alert_before_beats += 1
                    beeps.append(Beep(double=True))
            beats = int(ref / self.config.sound_interval)
            if beats > self._sound_beats:
                self._sound_beats = beats
                beeps.append(Beep())

        return TickResult(
            display=display, state=state, beeps=beeps,
            finished=finished, restarted=restarted,
        )
```

Behavior notes ported verbatim from `overlay.py._update()`:
- Countdown warn-window color only applies when `remaining > 6.0` (original line 343).
- Auto-restart resets `_sound_beats` and `_last_short_beep_sec` but NOT `_sound_alert_before_beats` (original lines 320–324).
- Last-5 beeps fire at displayed seconds 6..2 (short) and 1 (long) — the original condition includes 6 (line 334).

- [ ] **Step 4: Run tests, verify pass**

Run: `pytest tests/test_timer_engine.py -v`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add src/timehud/timer_engine.py tests/
git commit -m "feat: extract Qt-free TimerEngine with test suite"
```

---

### Task 3: Beep scheduling tests (engine already emits beeps)

**Files:**
- Modify: `tests/test_timer_engine.py` (append)

**Interfaces:**
- Consumes: `TimerEngine.tick() -> TickResult` with `beeps: list[Beep]` from Task 2.
- Produces: locked-in beep semantics for the overlay integration (Task 4 maps `Beep(short=…)` → `SoundManager.play_alert(short=…)`, `Beep(double=True)` → `play_alert(double_beep=True)`).

- [ ] **Step 1: Write the beep tests (they should pass immediately — the Task 2 implementation includes scheduling; these tests pin the behavior)**

Append to `tests/test_timer_engine.py`:

```python
def collect_beeps(engine, clock, seconds, step=0.1):
    """Advance in `step` increments, ticking like the 100 ms UI loop."""
    out = []
    n = int(seconds / step)
    for _ in range(n):
        clock.advance(step)
        out.extend(engine.tick().beeps)
    return out


class TestIntervalBeeps:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "stopwatch"
        config.sound_enabled = True
        config.sound_interval = 60
        config.sound_alert_before = 0
        return TimerEngine(config, clock=clock)

    def test_main_beep_every_interval(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 125)
        mains = [b for b in beeps if not b.short and not b.double]
        assert len(mains) == 2          # at 60 s and 120 s

    def test_no_beeps_when_sound_disabled(self, engine, clock):
        engine.config.sound_enabled = False
        engine.toggle()
        assert collect_beeps(engine, clock, 125) == []

    def test_no_beeps_while_paused(self, engine, clock):
        assert collect_beeps(engine, clock, 125) == []

    def test_alert_before_double_beep(self, engine, clock):
        engine.config.sound_alert_before = 5
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 59)
        doubles = [b for b in beeps if b.double]
        assert len(doubles) == 1        # at 55 s (60 − 5)
        beeps = collect_beeps(engine, clock, 3)  # through 62 s
        mains = [b for b in beeps if not b.short and not b.double]
        assert len(mains) == 1          # main beep still at 60 s

    def test_pause_resume_does_not_replay_past_beeps(self, engine, clock):
        engine.toggle()
        collect_beeps(engine, clock, 70)   # beep at 60 consumed
        engine.toggle()                    # pause at 70 s
        engine.toggle()                    # resume
        beeps = collect_beeps(engine, clock, 5)
        assert beeps == []                 # nothing until 120 s


class TestLastFiveSeconds:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 30
        config.sound_enabled = True
        config.sound_interval = 6000       # keep interval beeps out of the way
        config.alert_last_5_seconds = True
        return TimerEngine(config, clock=clock)

    def test_short_beeps_then_long(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 29.95)
        shorts = [b for b in beeps if b.short]
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(shorts) == 5            # displayed 6,5,4,3,2
        assert len(longs) == 1             # displayed 1

    def test_warn_then_end_state(self, engine, clock):
        engine.toggle()
        clock.advance(26.5)                # remaining 3.5 → displayed 4
        assert engine.tick().state == "warn"
        clock.advance(2.6)                 # remaining 0.9 → displayed 1
        assert engine.tick().state == "end"

    def test_disabled_flag_suppresses(self, engine, clock):
        engine.config.alert_last_5_seconds = False
        engine.toggle()
        beeps = collect_beeps(engine, clock, 29.5)
        assert [b for b in beeps if b.short] == []
```

- [ ] **Step 2: Run the full engine suite**

Run: `pytest tests/test_timer_engine.py -v`
Expected: all PASS. If any beep test fails, the engine deviates from
`overlay.py` lines 283–362 — fix the engine, not the test, after re-reading
the original.

- [ ] **Step 3: Commit**

```bash
git add tests/test_timer_engine.py
git commit -m "test: pin interval and last-5-seconds beep scheduling"
```

---

### Task 4: Overlay integration (behavior-preserving)

**Files:**
- Modify: `src/timehud/overlay.py` — replace inline timer state with `TimerEngine`

**Interfaces:**
- Consumes: everything Task 2 produces.
- Produces: `OverlayWindow.engine` attribute; public methods `toggle_timer`, `reset_timer`, `toggle_visibility` keep their exact names/signatures (used by `main.py` hotkeys and tray).

- [ ] **Step 1: Replace state fields in `__init__` (current lines 83–91)**

```python
from timehud.timer_engine import TimerEngine
```

and in `__init__`:

```python
        # ── Timer state ───────────────────────────────────────────────────
        self.engine = TimerEngine(config)
```

(Delete `_running`, `_start_mono`, `_elapsed`, `_cd_remaining`, `_sound_beats`, `_sound_alert_before_beats`, `_last_short_beep_sec`.)

- [ ] **Step 2: Rewrite `_update()` (current lines 283–362) as a renderer**

```python
    def _update(self) -> None:
        """Called every 100 ms to refresh the display."""
        now = datetime.datetime.now()
        if self.config.show_clock:
            self.lbl_clock.setText(now.strftime("%H:%M:%S"))
        if not self.config.show_timer:
            return

        result = self.engine.tick()
        self.lbl_timer.setText(_fmt(result.display))

        colors = {
            "run":   self.config.color_timer_run,
            "pause": self.config.color_timer_pause,
            "warn":  _TMR_WARN,
            "end":   _TMR_END,
        }
        self.lbl_timer.setStyleSheet(
            f"color:{colors[result.state]}; background:transparent;"
        )

        if result.finished and not result.restarted:
            self.btn_start.setText("▶")

        for beep in result.beeps:
            if beep.double:
                self.sound.play_alert(double_beep=True)
            else:
                self.sound.play_alert(short=beep.short)
```

- [ ] **Step 3: Delegate commands (replace current lines 363–411)**

Delete `_get_elapsed` and `_get_remaining`. Rewrite:

```python
    def toggle_timer(self) -> None:
        """Start if stopped, pause if running."""
        self.engine.toggle()
        self.btn_start.setText("⏸" if self.engine.running else "▶")

    def reset_timer(self) -> None:
        """Stop and zero the timer."""
        self.engine.reset()
        self.btn_start.setText("▶")
        self._update()

    def _toggle_mode(self) -> None:
        """Switch stopwatch ↔ countdown."""
        new_mode = "countdown" if self.config.timer_mode == "stopwatch" else "stopwatch"
        self.engine.set_mode(new_mode)
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self.config.save()
```

- [ ] **Step 4: Update the two remaining state consumers**

Wheel handler on `lbl_mode` (current lines 457–475) — duration adjustment branch becomes:

```python
                    if self.config.timer_mode == "countdown" and pos_x > width * 0.55:
                        step = 1 if pos_x > width * 0.8 else 60
                        delta = 0
                        if delta_wheel > 0:
                            delta = step
                        elif delta_wheel < 0:
                            delta = -min(step, self.config.countdown_duration - 1)
                        if delta:
                            self.config.countdown_duration += delta
                            self.engine.adjust_countdown(delta)
                        self.config.save()
                        self._refresh_mode_label()
                        self._update()
```

(rename the local `delta = event.angleDelta().y()` to `delta_wheel` in this
branch). Mode-cycling branches (lines 476–494) replace the
`self.reset_timer(); self.config.timer_mode = new_mode` pairs with
`self.engine.set_mode(new_mode); self.btn_start.setText("▶")`.

`main.py` cleanup (`window.sound.cleanup()`) and hotkey wiring
(`window.toggle_timer` / `window.reset_timer` / `window.toggle_visibility`)
need no changes.

- [ ] **Step 5: Verify — tests green + headless smoke run**

```bash
pytest -q
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

Expected: tests pass; `SMOKE-OK` (app ran until timeout killed it, no traceback).
Also do a real run if a display is available: `./timehud` — start/pause/reset,
mode toggle, wheel-adjust countdown, last-5-seconds beeps.

- [ ] **Step 6: Commit**

```bash
git add src/timehud/overlay.py
git commit -m "refactor: drive overlay from TimerEngine"
```

---

### Task 5: Preset config fields + validation helper

**Files:**
- Modify: `src/timehud/config.py`
- Create: `tests/test_config.py`

**Interfaces:**
- Produces (used by Tasks 6–7):
  - `Config.presets: list` — of `{"name": str, "duration": int}`
  - `Config.active_preset: str` — name of applied preset, `""` if none
  - `valid_presets(presets: list) -> list[dict]` (module-level in `timehud.config`)

- [ ] **Step 1: Write failing tests**

`tests/test_config.py`:

```python
import json

import pytest

import timehud.config as config_mod
from timehud.config import Config, valid_presets


@pytest.fixture
def config_path(tmp_path, monkeypatch):
    path = str(tmp_path / "config.json")
    monkeypatch.setattr(config_mod, "CONFIG_PATH", path)
    return path


class TestRoundTrip:
    def test_save_load(self, config_path):
        cfg = Config(font_size=44, presets=[{"name": "Plank", "duration": 60}])
        cfg.save()
        loaded = Config.load()
        assert loaded.font_size == 44
        assert loaded.presets == [{"name": "Plank", "duration": 60}]

    def test_unknown_keys_ignored(self, config_path):
        with open(config_path, "w") as fh:
            json.dump({"font_size": 20, "from_the_future": True}, fh)
        loaded = Config.load()
        assert loaded.font_size == 20

    def test_missing_file_gives_defaults(self, config_path):
        cfg = Config.load()
        assert cfg.presets == [
            {"name": "1 min", "duration": 60},
            {"name": "5 min", "duration": 300},
        ]
        assert cfg.active_preset == ""


class TestValidPresets:
    def test_filters_malformed_entries(self):
        raw = [
            {"name": "ok", "duration": 90},
            {"name": "no duration"},
            {"duration": 30},
            {"name": 5, "duration": 30},
            {"name": "bad duration", "duration": "30"},
            "not a dict",
            {"name": "zero", "duration": 0},
        ]
        assert valid_presets(raw) == [{"name": "ok", "duration": 90}]

    def test_bool_duration_rejected(self):
        assert valid_presets([{"name": "x", "duration": True}]) == []
```

- [ ] **Step 2: Run, verify failure**

Run: `pytest tests/test_config.py -v`
Expected: `ImportError: cannot import name 'valid_presets'`

- [ ] **Step 3: Implement in `config.py`**

Add to the dataclass (in the Behaviour section):

```python
    # ── Presets ────────────────────────────────────────────────────────────
    # Countdown presets shown in the right-click menu.
    presets: list = field(default_factory=lambda: [
        {"name": "1 min", "duration": 60},
        {"name": "5 min", "duration": 300},
    ])
    active_preset: str = ""   # name of the applied preset, "" = none
```

Module-level helper (after the dataclass):

```python
def valid_presets(presets: list) -> list:
    """Filter out malformed preset entries (defensive against hand-edited config)."""
    out = []
    for p in presets:
        if (
            isinstance(p, dict)
            and isinstance(p.get("name"), str)
            and isinstance(p.get("duration"), int)
            and not isinstance(p.get("duration"), bool)
            and p["duration"] > 0
        ):
            out.append(p)
    return out
```

Note: `save()`/`load()` currently reference the module-global `CONFIG_PATH`
at call time — the monkeypatch in the fixture relies on that; do not refactor
them to capture the path at import time. If `save()` inlines
`os.path.dirname(CONFIG_PATH)`, change both `save` and `load` to read
`config_mod`-level `CONFIG_PATH` via plain module global (already the case).

- [ ] **Step 4: Run all tests**

Run: `pytest -q`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add src/timehud/config.py tests/test_config.py
git commit -m "feat: add countdown presets to config with validation helper"
```

---

### Task 6: Presets in the context menu + mode label

**Files:**
- Modify: `src/timehud/overlay.py` — `create_context_menu` (line ~538), `_refresh_mode_label` (line ~413), wheel handler, new helper methods
- Modify: `README.md` — context-menu section

**Interfaces:**
- Consumes: `Config.presets`, `Config.active_preset`, `valid_presets` (Task 5); `engine.set_mode`, `engine.reset` (Task 2).
- Produces: `OverlayWindow._apply_preset(preset: dict)`, `_save_current_preset()`; `_open_settings(tab: str | None = None)` gains the optional tab parameter Task 7 consumes.

- [ ] **Step 1: Import helper**

In `overlay.py`, change the config import line to:

```python
from timehud.config import Config, valid_presets
```

- [ ] **Step 2: Add the Presets submenu in `create_context_menu`, directly after the Settings action + separator**

```python
        # Presets sub-menu
        preset_menu = menu.addMenu("⏱  Presets")
        act_sw = preset_menu.addAction("Stopwatch")
        act_sw.setCheckable(True)
        act_sw.setChecked(self.config.timer_mode == "stopwatch")
        act_sw.triggered.connect(self._apply_stopwatch)
        presets = valid_presets(self.config.presets)
        if presets:
            preset_menu.addSeparator()
            for p in presets:
                a = preset_menu.addAction(f'{p["name"]} {_fmt(p["duration"])}')
                a.setCheckable(True)
                a.setChecked(
                    self.config.timer_mode == "countdown"
                    and self.config.active_preset == p["name"]
                )
                a.triggered.connect(lambda checked, p=p: self._apply_preset(p))
        preset_menu.addSeparator()
        act_save = preset_menu.addAction("Save current as preset…")
        act_save.setEnabled(self.config.timer_mode == "countdown")
        act_save.triggered.connect(self._save_current_preset)
        act_manage = preset_menu.addAction("Manage presets…")
        act_manage.triggered.connect(lambda: self._open_settings(tab="presets"))
        menu.addSeparator()
```

- [ ] **Step 3: Add helper methods (next to `_toggle_mode`)**

```python
    def _apply_stopwatch(self) -> None:
        self.engine.set_mode("stopwatch")
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self.config.save()

    def _apply_preset(self, preset: dict) -> None:
        self.config.timer_mode = "countdown"
        self.config.countdown_duration = int(preset["duration"])
        self.config.active_preset = preset["name"]
        self.engine.reset()
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self.config.save()
        self._update()

    def _save_current_preset(self) -> None:
        from PyQt6.QtWidgets import QInputDialog
        name, ok = QInputDialog.getText(self, "Save preset", "Preset name:")
        name = name.strip()
        if not ok or not name:
            return
        # Same-name preset is overwritten (predictable rule per spec)
        presets = [p for p in valid_presets(self.config.presets) if p["name"] != name]
        presets.append({"name": name, "duration": int(self.config.countdown_duration)})
        self.config.presets = presets
        self.config.active_preset = name
        self._refresh_mode_label()
        self.config.save()
```

- [ ] **Step 4: Show preset name in the mode label (`_refresh_mode_label`)**

```python
    def _refresh_mode_label(self) -> None:
        if self.config.timer_mode == "stopwatch":
            self.lbl_mode.setText("STOPWATCH")
            self.btn_mode.setText("SW")
        else:
            dur = _fmt(self.config.countdown_duration)
            if self.config.active_preset:
                self.lbl_mode.setText(f"{self.config.active_preset.upper()}  ·  {dur}")
            else:
                self.lbl_mode.setText(f"COUNTDOWN  {dur}")
            self.btn_mode.setText("CD")
```

- [ ] **Step 5: Clear `active_preset` when the duration is changed by other means**

In the wheel handler duration branch (Task 4 version), after
`self.config.countdown_duration += delta` add:

```python
                            self.config.active_preset = ""
```

- [ ] **Step 6: `_open_settings` accepts a tab hint (consumed in Task 7)**

```python
    def _open_settings(self, tab: str | None = None) -> None:
```

and after `dlg = SettingsDialog(...)`:

```python
        if tab is not None:
            dlg.select_tab(tab)
```

Guard for Task-6-only state: until Task 7 lands, add
`if hasattr(dlg, "select_tab"):` around the call so Task 6 is shippable alone.

- [ ] **Step 7: Verify**

```bash
pytest -q
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

Plus manual check on a display: right-click → Presets → pick "5 min" → timer
shows `05:00`, label shows `5 MIN · 05:00`, single click starts. Save-current
and checkmarks behave.

- [ ] **Step 8: Update README Controls section**

Add under "Right-click context menu":

```markdown
- **Presets** – one-click countdown presets (and back to stopwatch); save the
  current countdown as a preset or manage them in Settings
```

- [ ] **Step 9: Commit**

```bash
git add src/timehud/overlay.py README.md
git commit -m "feat: countdown presets in context menu with active-preset label"
```

---

### Task 7: Presets tab in the settings dialog

**Files:**
- Modify: `src/timehud/settings_dialog.py`

**Interfaces:**
- Consumes: `Config.presets`, `valid_presets` (Task 5); `_open_settings(tab="presets")` calls `select_tab("presets")` (Task 6).
- Produces: `SettingsDialog.select_tab(name: str) -> None`.

- [ ] **Step 1: Imports and tab registration**

Add `QListWidget` to the `PyQt6.QtWidgets` import list and change the config
import to `from timehud.config import Config, valid_presets`.

In `_build_ui`, keep a handle on the tab widget and register the new tab
before About:

```python
        self._tabs = tabs
        tabs.addTab(self._display_tab(),  "🖥️  Display")
        tabs.addTab(self._timer_tab(),    "⏱  Timer")
        tabs.addTab(self._presets_tab(),  "📋  Presets")
        tabs.addTab(self._sound_tab(),    "🔔  Sound")
        tabs.addTab(self._about_tab(),    "ℹ️  About")
```

- [ ] **Step 2: Build the tab**

```python
    def _presets_tab(self) -> QWidget:
        tab = QWidget()
        layout = QVBoxLayout(tab)

        self.preset_list = QListWidget()
        self.preset_list.currentRowChanged.connect(self._preset_selected)
        layout.addWidget(self.preset_list)

        row = QHBoxLayout()
        self.preset_name_edit = QLineEdit()
        self.preset_name_edit.setPlaceholderText("Name")
        self.preset_dur_spin = QSpinBox()
        self.preset_dur_spin.setRange(1, 24 * 3600)
        self.preset_dur_spin.setSuffix(" s")
        self.preset_dur_spin.setValue(300)
        row.addWidget(self.preset_name_edit, 1)
        row.addWidget(self.preset_dur_spin)
        layout.addLayout(row)

        btns = QHBoxLayout()
        add_btn = QPushButton("Add / Update")
        add_btn.clicked.connect(self._preset_add)
        rm_btn = QPushButton("Remove")
        rm_btn.clicked.connect(self._preset_remove)
        btns.addStretch()
        btns.addWidget(add_btn)
        btns.addWidget(rm_btn)
        layout.addLayout(btns)
        return tab
```

- [ ] **Step 3: Tab logic (mutates `config.presets` directly + emits `config_changed` so the overlay menu rebuild picks it up on next open)**

```python
    def _reload_preset_list(self) -> None:
        self.preset_list.clear()
        for p in valid_presets(self.config.presets):
            m, s = divmod(p["duration"], 60)
            self.preset_list.addItem(f'{p["name"]}  —  {m:02d}:{s:02d}')

    def _preset_selected(self, row: int) -> None:
        presets = valid_presets(self.config.presets)
        if 0 <= row < len(presets):
            self.preset_name_edit.setText(presets[row]["name"])
            self.preset_dur_spin.setValue(presets[row]["duration"])

    def _preset_add(self) -> None:
        name = self.preset_name_edit.text().strip()
        if not name:
            return
        presets = [p for p in valid_presets(self.config.presets) if p["name"] != name]
        presets.append({"name": name, "duration": self.preset_dur_spin.value()})
        self.config.presets = presets
        self._reload_preset_list()
        self.config_changed.emit()

    def _preset_remove(self) -> None:
        row = self.preset_list.currentRow()
        presets = valid_presets(self.config.presets)
        if 0 <= row < len(presets):
            removed = presets.pop(row)
            if self.config.active_preset == removed["name"]:
                self.config.active_preset = ""
            self.config.presets = presets
            self._reload_preset_list()
            self.config_changed.emit()

    def select_tab(self, name: str) -> None:
        """Open the dialog focused on a named tab (e.g. 'presets')."""
        labels = {self._tabs.tabText(i).split()[-1].lower(): i
                  for i in range(self._tabs.count())}
        if name.lower() in labels:
            self._tabs.setCurrentIndex(labels[name.lower()])
```

- [ ] **Step 4: Populate on load + clear stale active preset on manual duration edit**

In `_load_values`, append:

```python
        self._reload_preset_list()
```

In `_apply_to_config`, before `c.countdown_duration = self.countdown_spin.value()`:

```python
        if self.countdown_spin.value() != c.countdown_duration:
            c.active_preset = ""   # duration changed manually → preset no longer applies
```

- [ ] **Step 5: Verify**

```bash
pytest -q
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

Manual on a display: Settings → Presets tab: add "Plank" 60 s, remove one,
reopen right-click menu — list reflects changes. "Manage presets…" opens the
dialog on the Presets tab.

- [ ] **Step 6: Commit**

```bash
git add src/timehud/settings_dialog.py
git commit -m "feat: presets management tab in settings dialog"
```

---

### Task 8: Sync README config example + ruff pass

**Files:**
- Modify: `README.md` (settings JSON example)
- Possibly modify: any file ruff flags

**Interfaces:** none new.

- [ ] **Step 1: Add new fields to the README settings JSON example**

```json
  "presets": [
    { "name": "1 min", "duration": 60 },
    { "name": "5 min", "duration": 300 }
  ],
  "active_preset": ""
```

- [ ] **Step 2: Run ruff and fix findings in touched files only**

Run: `ruff check src tests`
Expected: clean (fix anything it reports in files this plan touched; leave
pre-existing findings in untouched files alone — note them instead).

- [ ] **Step 3: Full verification**

```bash
pytest -q
ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document presets config fields"
```
