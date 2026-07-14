# Interval Mode + Progress Bar + CI + UX Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Third timer mode (interval work/rest × rounds) with phase-aware colors and a progress bar, plus CI tests, tray-menu freshness, settings Cancel revert, and consistent duration formatting.

**Architecture:** Interval logic lives entirely in the Qt-free `TimerEngine` (phase state machine reusing the countdown mechanics); `TickResult` grows `phase/round/rounds/progress` so the overlay stays a renderer. Progress bar is a tiny custom QWidget. Tray staleness is fixed by splitting menu population from menu creation and rebuilding on `aboutToShow`.

**Tech Stack:** Python ≥3.10, PyQt6, pytest, ruff, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-07-14-interval-mode-and-fixes-design.md`

## Global Constraints

- No Qt imports in `timer_engine.py`, `themes.py`, or any test file.
- Existing stopwatch/countdown behavior unchanged (39 existing tests must stay green untouched, except the classic-theme guard which gains one field assertion).
- Old configs keep loading (new fields have defaults).
- The countdown "snap back to full duration after finish" quirk stays — do NOT fix it.
- Duration display rule: MM:SS below one hour, HH:MM:SS from one hour (existing `_fmt` behavior, moving to `fmt_seconds`).
- Interval semantics: trailing rest of the last round is skipped; `interval_rest = 0` = back-to-back work rounds; work→rest beep is `Beep(double=True)`, (rest|work)→work beep is `Beep()`; no periodic `sound_interval` beeps and no `sound_alert_before` warn-window in interval mode; last-5 shorts at displayed 6..2 only (no long at 1).
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: CI workflow

**Files:**
- Create: `.github/workflows/test.yml`

**Interfaces:** none.

- [ ] **Step 1: Create the workflow**

```yaml
name: Tests

on:
  push:
    branches: [main]
  pull_request:

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - run: pip install -e ".[dev]"
      - run: ruff check src tests
      - run: pytest -q
```

- [ ] **Step 2: Validate YAML locally**

Run: `.venv/bin/python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/test.yml')); print('YAML-OK')"`
(If PyYAML missing, `pip install pyyaml` into the venv first.)
Expected: `YAML-OK`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/test.yml
git commit -m "ci: run ruff and pytest on push and PRs"
```

---

### Task 2: `fmt_seconds` shared formatter

**Files:**
- Modify: `src/timehud/timer_engine.py` (add module function)
- Modify: `src/timehud/overlay.py` (`_fmt` at ~line 831 becomes an import alias)
- Modify: `src/timehud/settings_dialog.py` (`_reload_preset_list`)
- Modify: `tests/test_timer_engine.py` (append tests)

**Interfaces:**
- Produces: `timehud.timer_engine.fmt_seconds(secs: float) -> str`.

- [ ] **Step 1: Failing tests** (append to `tests/test_timer_engine.py`)

```python
from timehud.timer_engine import fmt_seconds


class TestFmtSeconds:
    def test_under_a_minute(self):
        assert fmt_seconds(59) == "00:59"

    def test_minutes(self):
        assert fmt_seconds(61) == "01:01"
        assert fmt_seconds(300) == "05:00"

    def test_hour_boundary(self):
        assert fmt_seconds(3599) == "59:59"
        assert fmt_seconds(3600) == "01:00:00"

    def test_large(self):
        assert fmt_seconds(86399) == "23:59:59"

    def test_negative_clamps_to_zero(self):
        assert fmt_seconds(-3) == "00:00"
```

Run: `.venv/bin/pytest tests/test_timer_engine.py -k FmtSeconds -v` → ImportError.

- [ ] **Step 2: Implement in `timer_engine.py`** (module level, after the dataclasses)

```python
def fmt_seconds(secs: float) -> str:
    """Format seconds → MM:SS, or HH:MM:SS from one hour up."""
    s = max(0, int(secs))
    h, rem = divmod(s, 3600)
    m, sec = divmod(rem, 60)
    if h:
        return f"{h:02d}:{m:02d}:{sec:02d}"
    return f"{m:02d}:{sec:02d}"
```

- [ ] **Step 3: Replace overlay `_fmt` body with an alias**

Delete the `def _fmt(...)` function at the bottom of `overlay.py`; add to the
`from timehud.timer_engine import TimerEngine` line: `, fmt_seconds`; add
right below the imports section: `_fmt = fmt_seconds` (keeps all call sites).

- [ ] **Step 4: Use in settings presets list**

`settings_dialog.py`: add import `from timehud.timer_engine import fmt_seconds`.
In `_reload_preset_list`, replace the `divmod` formatting:

```python
    def _reload_preset_list(self) -> None:
        self.preset_list.clear()
        for p in valid_presets(self.config.presets):
            self.preset_list.addItem(f'{p["name"]}  —  {fmt_seconds(p["duration"])}')
```

- [ ] **Step 5: Verify + commit**

```bash
.venv/bin/pytest -q && .venv/bin/ruff check src tests
git add src/timehud/timer_engine.py src/timehud/overlay.py src/timehud/settings_dialog.py tests/test_timer_engine.py
git commit -m "refactor: shared fmt_seconds; fix preset list hour formatting"
```

---

### Task 3: Tray menu rebuilds on open

**Files:**
- Modify: `src/timehud/overlay.py` — `create_context_menu` splits into wrapper + `_populate_context_menu`
- Modify: `src/timehud/main.py` — persistent tray menu with `aboutToShow` rebuild

**Interfaces:**
- Produces: `OverlayWindow._populate_context_menu(menu: QMenu, include_window_actions: bool) -> None`.

- [ ] **Step 1: Split `create_context_menu`**

The current method body (style + all action/submenu construction) moves into
`_populate_context_menu(self, menu, include_window_actions)`; every
`menu = QMenu(self)` / `return menu` stays in the wrapper:

```python
    def create_context_menu(self, include_window_actions: bool | None = None) -> QMenu:
        """Build and return the context menu (used by both overlay and tray)."""
        menu = QMenu(self)
        self._populate_context_menu(menu, include_window_actions)
        return menu

    def _populate_context_menu(self, menu: QMenu, include_window_actions: bool | None = None) -> None:
        if include_window_actions is None:
            include_window_actions = self.config.show_tray_icon
        menu.setStyleSheet(_MENU_STYLE)
        # ... existing body unchanged, minus the QMenu creation and return ...
```

Submenus keep being created via `menu.addMenu(...)` so they're parented to
the passed menu and die with `menu.clear()`.

- [ ] **Step 2: Tray uses a persistent, self-refreshing menu**

`main.py`, inside `_set_tray_visibility` — replace
`tray_icon.setContextMenu(window.create_context_menu())` with:

```python
            if tray_icon.contextMenu() is None:
                tray_menu = QMenu()

                def _rebuild_tray_menu() -> None:
                    tray_menu.clear()
                    window._populate_context_menu(tray_menu, include_window_actions=True)

                tray_menu.aboutToShow.connect(_rebuild_tray_menu)
                _rebuild_tray_menu()
                tray_icon.setContextMenu(tray_menu)
```

Add `QMenu` to the `PyQt6.QtWidgets` import in `main.py`.
Note: `tray_menu` must not be garbage-collected — keep a module/closure
reference (`tray_icon.setContextMenu` stores it C++-side, and the closure
holds the Python ref; that is sufficient).

- [ ] **Step 3: Verify**

```bash
.venv/bin/pytest -q && .venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main; test $? -eq 124 && echo SMOKE-OK
```

(No `--no-tray` this time — the tray path must construct.) Plus headless check:

```bash
QT_QPA_PLATFORM=offscreen .venv/bin/python -c "
from PyQt6.QtWidgets import QApplication, QMenu
from timehud.config import Config
from timehud.overlay import OverlayWindow
app = QApplication([])
w = OverlayWindow(Config())
m = QMenu()
w._populate_context_menu(m, include_window_actions=True)
before = [a.text() for a in m.actions() if a.text()]
w.config.presets.append({'name': 'New', 'duration': 90})
m.clear()
w._populate_context_menu(m, include_window_actions=True)
preset_menu = [a.menu() for a in m.actions() if a.menu() and a.text() == 'Presets'][0]
assert any('New' in a.text() for a in preset_menu.actions()), 'rebuilt menu missing new preset'
print('TRAY-REBUILD-OK')
"
```

- [ ] **Step 4: Commit**

```bash
git add src/timehud/overlay.py src/timehud/main.py
git commit -m "fix: rebuild tray menu on open so checkmarks stay current"
```

---

### Task 4: Settings Cancel revert

**Files:**
- Modify: `src/timehud/overlay.py` — `_open_settings`

**Interfaces:** none new.

- [ ] **Step 1: Snapshot + restore**

At the top of `_open_settings`, before creating the dialog:

```python
        import copy
        from dataclasses import asdict
        snapshot = copy.deepcopy(asdict(self.config))
```

Replace the tail:

```python
        if dlg.exec():
            update_ui()
            self.config.save()
```

with:

```python
        if dlg.exec():
            update_ui()
        else:
            # Cancel: live-applied changes are rolled back
            for key, value in snapshot.items():
                setattr(self.config, key, value)
            update_ui()
        self.config.save()
```

(`update_ui` already handles restyle, resize, reposition, tray toggle; the
existing `_sync_countdown_duration` call inside it re-syncs the engine if the
restore changed the countdown duration back.)

- [ ] **Step 2: Verify**

```bash
.venv/bin/pytest -q
QT_QPA_PLATFORM=offscreen .venv/bin/python -c "
from PyQt6.QtWidgets import QApplication, QDialog
from timehud.config import Config
from timehud.overlay import OverlayWindow
app = QApplication([])
w = OverlayWindow(Config())
orig_size = w.config.font_size
orig_presets = [dict(p) for p in w.config.presets]
def fake_exec(dlg):
    dlg.font_size_spin.setValue(orig_size + 10)   # live-applies via signal
    dlg.preset_name_edit.setText('Temp')
    dlg.preset_dur_spin.setValue(45)
    dlg._preset_add()                              # mutates config.presets live
    return 0                                       # Cancel
QDialog.exec = fake_exec
w._open_settings()
assert w.config.font_size == orig_size, w.config.font_size
assert w.config.presets == orig_presets
print('CANCEL-REVERT-OK')
"
```

- [ ] **Step 3: Commit**

```bash
git add src/timehud/overlay.py
git commit -m "fix: cancel in settings reverts live-applied changes"
```

---

### Task 5: Engine interval mode (TDD)

**Files:**
- Modify: `src/timehud/config.py` (three fields)
- Modify: `src/timehud/timer_engine.py`
- Modify: `tests/test_timer_engine.py` (append)

**Interfaces:**
- Consumes: existing `TimerEngine` internals.
- Produces (Tasks 7–8 rely on these):
  - `Config.interval_work: int = 40`, `interval_rest: int = 20`, `interval_rounds: int = 8`
  - `TickResult` new fields: `phase: str = ""`, `round: int = 0`, `rounds: int = 0`, `progress: float = -1.0`
  - Interval `timer_mode == "interval"` behavior per Global Constraints
  - `progress`: countdown → `display / countdown_duration`; interval → `display / current_phase_duration`; stopwatch → -1.0; clamped 0..1

- [ ] **Step 1: Failing tests** (append to `tests/test_timer_engine.py`)

```python
class TestInterval:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "interval"
        config.interval_work = 40
        config.interval_rest = 20
        config.interval_rounds = 3
        config.sound_enabled = True
        config.sound_interval = 60           # must NOT fire in interval mode
        config.alert_last_5_seconds = False
        return TimerEngine(config, clock=clock)

    def test_idle_start(self, engine):
        assert engine.is_idle() is True
        r = engine.tick()
        assert r.phase == "work" and r.round == 1 and r.rounds == 3
        assert r.display == pytest.approx(40)

    def test_work_to_rest_transition(self, engine, clock):
        engine.toggle()
        clock.advance(40.05)
        r = engine.tick()
        assert r.phase == "rest" and r.round == 1
        assert [b for b in r.beeps if b.double], "work->rest must double-beep"
        assert r.display == pytest.approx(20, abs=0.1)

    def test_rest_to_work_advances_round(self, engine, clock):
        engine.toggle()
        clock.advance(40.05); engine.tick()
        clock.advance(20.05)
        r = engine.tick()
        assert r.phase == "work" and r.round == 2
        assert [b for b in r.beeps if not b.short and not b.double]

    def test_session_finish_skips_last_rest(self, engine, clock):
        engine.toggle()
        # 3 rounds: W R W R W -> end (no trailing rest)
        for _ in range(2):
            clock.advance(40.05); engine.tick()   # work ends
            clock.advance(20.05); engine.tick()   # rest ends
        clock.advance(40.05)
        r = engine.tick()
        assert r.finished is True and r.state == "end"
        assert engine.running is False
        # parked at zero afterwards
        r2 = engine.tick()
        assert r2.state == "end" and r2.display == 0.0

    def test_zero_rest_back_to_back(self, engine, clock):
        engine.config.interval_rest = 0
        engine.reset()
        engine.toggle()
        clock.advance(40.05)
        r = engine.tick()
        assert r.phase == "work" and r.round == 2
        assert [b for b in r.beeps if not b.double and not b.short]

    def test_no_periodic_beeps_in_interval(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 39)   # crosses nothing, sound_interval=60 would... stay silent
        assert beeps == []

    def test_last5_shorts_no_long(self, engine, clock):
        engine.config.alert_last_5_seconds = True
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 39.5)
        shorts = [b for b in beeps if b.short]
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(shorts) == 5      # displayed 6,5,4,3,2
        assert longs == []           # no long at 1 inside a phase

    def test_pause_resume_mid_phase(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        engine.toggle()
        clock.advance(500)
        assert engine.remaining() == pytest.approx(30)
        assert engine.is_idle() is False

    def test_restart_after_finish(self, engine, clock):
        engine.toggle()
        for _ in range(2):
            clock.advance(40.05); engine.tick()
            clock.advance(20.05); engine.tick()
        clock.advance(40.05); engine.tick()      # finished
        engine.toggle()                           # start again from round 1
        r = engine.tick()
        assert engine.running is True
        assert r.phase == "work" and r.round == 1
        assert r.display == pytest.approx(40, abs=0.1)


class TestProgress:
    def test_stopwatch_has_no_progress(self, engine, clock):
        engine.toggle()
        clock.advance(5)
        assert engine.tick().progress == -1.0

    def test_countdown_progress(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 100
        e = TimerEngine(config, clock=clock)
        e.toggle()
        clock.advance(25)
        assert e.tick().progress == pytest.approx(0.75)

    def test_interval_progress_per_phase(self, config, clock):
        config.timer_mode = "interval"
        config.interval_work = 40
        config.interval_rest = 20
        config.interval_rounds = 2
        e = TimerEngine(config, clock=clock)
        e.toggle()
        clock.advance(30)
        assert e.tick().progress == pytest.approx(0.25)   # 10/40 left
        clock.advance(10.05)
        e.tick()                                          # into rest
        clock.advance(5)
        assert e.tick().progress == pytest.approx(15 / 20, abs=0.01)
```

Run: `.venv/bin/pytest tests/test_timer_engine.py -k "Interval or Progress" -v`
Expected: failures (missing config fields / TickResult fields / behavior).

- [ ] **Step 2: Config fields** (`config.py`, Timer section)

```python
    # ── Interval mode ──────────────────────────────────────────────────────
    interval_work: int = 40     # seconds of work per round
    interval_rest: int = 20     # seconds of rest (0 = back-to-back rounds)
    interval_rounds: int = 8
```

- [ ] **Step 3: Engine implementation**

`TickResult` gains fields (after `restarted`):

```python
    phase: str = ""          # "work" | "rest" in interval mode, else ""
    round: int = 0           # 1-based current round (interval mode)
    rounds: int = 0          # configured rounds (interval mode)
    progress: float = -1.0   # fraction of phase/countdown remaining; -1 = n/a
```

`__init__` additions (after `_cd_remaining`):

```python
        self._phase = "work"
        self._round = 1
        if config.timer_mode == "interval":
            self._cd_remaining = float(config.interval_work)
```

`reset()` — after the existing `_cd_remaining` line:

```python
        self._phase = "work"
        self._round = 1
        if self.config.timer_mode == "interval":
            self._cd_remaining = float(self.config.interval_work)
```

`toggle()` start branch — extend the finished-countdown guard:

```python
            if self.config.timer_mode == "countdown" and self._cd_remaining <= 0:
                self._cd_remaining = float(self.config.countdown_duration)
            elif self.config.timer_mode == "interval" and self._cd_remaining <= 0:
                self._phase = "work"
                self._round = 1
                self._cd_remaining = float(self.config.interval_work)
```

`is_idle()` — insert before the countdown return:

```python
        if self.config.timer_mode == "interval":
            return (
                self._phase == "work"
                and self._round == 1
                and self._cd_remaining == float(self.config.interval_work)
            )
```

New helpers (after `adjust_countdown`):

```python
    def _phase_duration(self) -> float:
        if self._phase == "rest":
            return float(self.config.interval_rest)
        return float(self.config.interval_work)

    def _advance_phase(self):
        """Move to the next interval phase. Returns (beep | None, session_done)."""
        if self._phase == "work":
            if self._round >= self.config.interval_rounds:
                self.running = False
                self._cd_remaining = 0.0
                return None, True
            if self.config.interval_rest > 0:
                self._phase = "rest"
                self._cd_remaining = float(self.config.interval_rest)
            else:
                self._round += 1
                self._cd_remaining = float(self.config.interval_work)
            self._start_mono = self._clock()
            self._last_short_beep_sec = -1
            return (Beep(double=True) if self._phase == "rest" else Beep()), False
        self._phase = "work"
        self._round += 1
        self._cd_remaining = float(self.config.interval_work)
        self._start_mono = self._clock()
        self._last_short_beep_sec = -1
        return Beep(), False
```

`tick()` changes:

1. The warn-window block at the top gains a mode guard:
   `if self.running and self.config.timer_mode != "interval" and self.config.sound_enabled and ...`
2. The stopwatch/countdown branches stay byte-identical. Insert an interval
   branch between them (`if stopwatch: ... elif self.config.timer_mode == "interval": ... else: # countdown`):

```python
        elif self.config.timer_mode == "interval":
            remaining = self.remaining()
            display = max(0.0, remaining)
            if remaining <= 0:
                if self.running:
                    transition_beep, done = self._advance_phase()
                    if transition_beep is not None:
                        beeps.append(transition_beep)
                    if done:
                        finished = True
                        state = "end"
                    else:
                        remaining = self.remaining()
                        display = max(0.0, remaining)
                        state = "run"
                else:
                    state = "end"     # finished session parked at 00:00
            elif self.running and remaining <= 6.0 and self.config.alert_last_5_seconds:
                sec_display = int(math.ceil(display))
                state = "warn"
                if sec_display != self._last_short_beep_sec and 2 <= sec_display <= 6:
                    self._last_short_beep_sec = sec_display
                    beeps.append(Beep(short=True))
            else:
                state = "run" if self.running else "pause"
```

3. The periodic-beep block at the bottom gains the same mode guard:
   `if self.running and self.config.timer_mode != "interval" and self.config.sound_enabled:`
4. Compute the new TickResult fields just before the return:

```python
        progress = -1.0
        if self.config.timer_mode == "countdown" and self.config.countdown_duration > 0:
            progress = max(0.0, min(1.0, display / self.config.countdown_duration))
        elif self.config.timer_mode == "interval":
            dur = self._phase_duration()
            if dur > 0:
                progress = max(0.0, min(1.0, display / dur))

        in_interval = self.config.timer_mode == "interval"
        return TickResult(
            display=display, state=state, beeps=beeps,
            finished=finished, restarted=restarted,
            phase=self._phase if in_interval else "",
            round=self._round if in_interval else 0,
            rounds=self.config.interval_rounds if in_interval else 0,
            progress=progress,
        )
```

- [ ] **Step 4: Run full suite**

Run: `.venv/bin/pytest -q` → all pass (existing 39+5 fmt + new interval/progress). `ruff check src tests` clean.

- [ ] **Step 5: Commit**

```bash
git add src/timehud/config.py src/timehud/timer_engine.py tests/test_timer_engine.py
git commit -m "feat: interval training mode and progress fraction in TimerEngine"
```

---

### Task 6: Theme `color_rest`

**Files:**
- Modify: `src/timehud/themes.py`
- Modify: `tests/test_themes.py`

**Interfaces:**
- Produces: `Theme.color_rest: str` — classic `#4FA8FF`, terminal `#60A5FA`, glass `#93C5FD`, mono `#B0BEC5`.

- [ ] **Step 1: Failing test** (append to `tests/test_themes.py`; also extend the classic guard)

```python
class TestColorRest:
    def test_all_themes_have_color_rest(self):
        expected = {
            "classic": "#4FA8FF",
            "terminal": "#60A5FA",
            "glass": "#93C5FD",
            "mono": "#B0BEC5",
        }
        for name, color in expected.items():
            assert THEMES[name].color_rest == color
```

And in `TestClassicMatchesLegacyLook.test_classic_values` add:
`assert t.color_rest == "#4FA8FF"`.

- [ ] **Step 2: Implement** — add `color_rest: str` field to the `Theme`
dataclass (after `color_end`) and the value to each of the four entries.

- [ ] **Step 3: Verify + commit**

```bash
.venv/bin/pytest tests/test_themes.py -v && .venv/bin/pytest -q
git add src/timehud/themes.py tests/test_themes.py
git commit -m "feat: per-theme rest-phase color"
```

---

### Task 7: Overlay + settings interval UI

**Files:**
- Modify: `src/timehud/overlay.py` — mode cycling (3 modes), `_refresh_mode_label`, `_update` (rest color + live round label), interval config sync
- Modify: `src/timehud/settings_dialog.py` — mode combo + interval spinboxes

**Interfaces:**
- Consumes: Task 5 TickResult fields, Task 6 `color_rest`.
- Produces: nothing new for later tasks (Task 8 wires the bar separately).

- [ ] **Step 1: 3-mode cycling**

Both `modes = ["stopwatch", "countdown"]` occurrences in the wheel handler
(~lines 512, 523) become `modes = ["stopwatch", "countdown", "interval"]`.
`_toggle_mode` becomes:

```python
    def _toggle_mode(self) -> None:
        """Cycle stopwatch → countdown → interval."""
        modes = ["stopwatch", "countdown", "interval"]
        curr = modes.index(self.config.timer_mode) if self.config.timer_mode in modes else 0
        self.engine.set_mode(modes[(curr + 1) % len(modes)])
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self.config.save()
        self._update()
```

- [ ] **Step 2: Mode label**

`_refresh_mode_label` becomes if/elif/else:

```python
    def _refresh_mode_label(self) -> None:
        if self.config.timer_mode == "stopwatch":
            self.lbl_mode.setText("STOPWATCH")
            self.btn_mode.setText("SW")
        elif self.config.timer_mode == "interval":
            cfg = self.config
            self.lbl_mode.setText(
                f"INTERVAL {cfg.interval_work}s/{cfg.interval_rest}s ×{cfg.interval_rounds}"
            )
            self.btn_mode.setText("IV")
        else:
            dur = _fmt(self.config.countdown_duration)
            if self.config.active_preset:
                self.lbl_mode.setText(f"{self.config.active_preset.upper()}  ·  {dur}")
            else:
                self.lbl_mode.setText(f"COUNTDOWN  {dur}")
            self.btn_mode.setText("CD")
```

Live round label — `__init__` gains `self._interval_label = ""`; in
`_update`, after the color handling:

```python
        if result.phase and not self.engine.is_idle():
            label = f"{result.phase.upper()} {result.round}/{result.rounds}"
            if label != self._interval_label:
                self._interval_label = label
                self.lbl_mode.setText(label)
        else:
            self._interval_label = ""
```

(`_refresh_mode_label` restores the static text on reset/mode change since
the idle branch clears `_interval_label`.)

- [ ] **Step 3: Rest color in `_update`**

Replace `self._set_timer_color(colors[result.state], animate)` context with:

```python
        color = colors[result.state]
        if result.phase == "rest" and result.state == "run":
            color = theme.color_rest
        self._set_timer_color(color, animate)
```

- [ ] **Step 4: Interval config sync (same desync class as countdown)**

`__init__`: `self._last_interval_cfg = (config.interval_work, config.interval_rest, config.interval_rounds)`.
New method next to `_sync_countdown_duration`:

```python
    def _sync_interval_config(self) -> None:
        """Reset the engine when interval settings change while stopped."""
        cur = (
            self.config.interval_work,
            self.config.interval_rest,
            self.config.interval_rounds,
        )
        if cur != self._last_interval_cfg:
            self._last_interval_cfg = cur
            if self.config.timer_mode == "interval" and not self.engine.running:
                self.engine.reset()
                self.btn_start.setText("▶")
```

Call it in `update_ui` right after `self._sync_countdown_duration()`, and
call `self._refresh_mode_label()` already happens later in `update_ui` (it
picks up new interval numbers).

- [ ] **Step 5: Settings Timer tab**

`_timer_tab`: `self.mode_combo.addItems(["stopwatch", "countdown", "interval"])`
(replace the two-item list). After the auto-restart checkbox rows, add:

```python
        self.interval_work_spin = QSpinBox()
        self.interval_work_spin.setRange(5, 3600)
        self.interval_work_spin.setSuffix(" s")
        form.addRow("Interval work:", self.interval_work_spin)

        self.interval_rest_spin = QSpinBox()
        self.interval_rest_spin.setRange(0, 3600)
        self.interval_rest_spin.setSuffix(" s")
        form.addRow("Interval rest:", self.interval_rest_spin)

        self.interval_rounds_spin = QSpinBox()
        self.interval_rounds_spin.setRange(1, 99)
        form.addRow("Interval rounds:", self.interval_rounds_spin)
```

`_apply_to_config` additions:

```python
        c.interval_work   = self.interval_work_spin.value()
        c.interval_rest   = self.interval_rest_spin.value()
        c.interval_rounds = self.interval_rounds_spin.value()
```

`_load_values` additions:

```python
        self.interval_work_spin.setValue(c.interval_work)
        self.interval_rest_spin.setValue(c.interval_rest)
        self.interval_rounds_spin.setValue(c.interval_rounds)
```

`_connect_live_updates` additions:

```python
        self.interval_work_spin.valueChanged.connect(_emit_if_valid)
        self.interval_rest_spin.valueChanged.connect(_emit_if_valid)
        self.interval_rounds_spin.valueChanged.connect(_emit_if_valid)
```

- [ ] **Step 6: Verify**

```bash
.venv/bin/pytest -q && .venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
QT_QPA_PLATFORM=offscreen .venv/bin/python -c "
from PyQt6.QtWidgets import QApplication
from timehud.config import Config
from timehud.overlay import OverlayWindow
app = QApplication([])
cfg = Config(); cfg.timer_mode = 'interval'
w = OverlayWindow(cfg)
assert w.btn_mode.text() == 'IV'
assert 'INTERVAL' in w.lbl_mode.text()
w._toggle_mode()
assert w.config.timer_mode == 'stopwatch'
w._toggle_mode(); w._toggle_mode()
assert w.config.timer_mode == 'interval'
print('MODE-CYCLE-OK')
"
```

- [ ] **Step 7: Commit**

```bash
git add src/timehud/overlay.py src/timehud/settings_dialog.py
git commit -m "feat: interval mode UI — 3-mode cycle, round label, rest color, settings"
```

---

### Task 8: Progress bar widget

**Files:**
- Modify: `src/timehud/overlay.py` — new `_ProgressBar` class, layout insertion, `_update` wiring, `update_ui` visibility

**Interfaces:**
- Consumes: `TickResult.progress` (Task 5), `theme.color_rest`/`color_warn`/`color_clock`.

- [ ] **Step 1: Widget class** (module level, before `OverlayWindow`)

```python
class _ProgressBar(QWidget):
    """Thin rounded bar showing remaining fraction of a countdown/phase."""

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self._fraction = -1.0
        self._color = QColor("#FFFFFF")
        self.setFixedHeight(3)
        self.hide()

    def set_state(self, fraction: float, color: str) -> None:
        qc = QColor(color)
        if fraction == self._fraction and qc == self._color:
            return
        self._fraction = fraction
        self._color = qc
        self.setVisible(fraction >= 0.0)
        self.update()

    def paintEvent(self, _event) -> None:  # noqa: N802
        if self._fraction < 0:
            return
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(QColor(255, 255, 255, 30))
        p.drawRoundedRect(self.rect(), 1.5, 1.5)
        w = int(self.width() * min(1.0, self._fraction))
        if w > 0:
            p.setBrush(self._color)
            p.drawRoundedRect(0, 0, w, self.height(), 1.5, 1.5)
```

- [ ] **Step 2: Layout insertion** — in `_build_ui`, after
`root.addWidget(self.lbl_timer)`:

```python
        self.progress_bar = _ProgressBar()
        root.addWidget(self.progress_bar)
```

- [ ] **Step 3: Wire in `_update`** — after the timer-color handling:

```python
        theme_bar_color = theme.color_clock
        if result.state in ("warn", "end"):
            theme_bar_color = theme.color_warn
        elif result.phase == "rest":
            theme_bar_color = theme.color_rest
        self.progress_bar.set_state(result.progress, theme_bar_color)
```

And in `update_ui`, next to the other visibility lines:
`self.progress_bar.setVisible(False)` when `not cfg.show_timer` — concretely:

```python
            if not cfg.show_timer:
                self.progress_bar.hide()
```

(When the timer is shown again, the next `_update` tick's `set_state`
restores visibility.)

- [ ] **Step 4: Verify**

```bash
.venv/bin/pytest -q && .venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen .venv/bin/python -c "
from PyQt6.QtWidgets import QApplication
from timehud.config import Config
from timehud.overlay import OverlayWindow
app = QApplication([])
cfg = Config(); cfg.timer_mode = 'countdown'
w = OverlayWindow(cfg)
w._update()
assert w.progress_bar._fraction == 1.0, w.progress_bar._fraction
cfg2 = Config()
w2 = OverlayWindow(cfg2)   # stopwatch
w2._update()
assert w2.progress_bar._fraction == -1.0
print('BAR-OK')
"
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

- [ ] **Step 5: Commit**

```bash
git add src/timehud/overlay.py
git commit -m "feat: progress bar for countdown and interval phases"
```

---

### Task 9: Docs + full verification

**Files:**
- Modify: `README.md`, `CLAUDE.md`

- [ ] **Step 1: README**

- Settings JSON example: add `"interval_work": 40, "interval_rest": 20, "interval_rounds": 8,` after `countdown_duration`.
- Controls: SW/CD references become SW/CD/IV ("Toggle Stopwatch → Countdown → Interval").
- New subsection after Themes:

```markdown
### Interval mode

Work/rest rounds for training (default 40 s work / 20 s rest × 8 rounds,
configurable in Settings → Timer). The round counter shows `WORK 3/8` /
`REST 3/8`, rest phases tint the timer blue, and a thin progress bar under
the timer depletes through each phase. Double beep = rest starts, long
beep = work starts; the session ends after the last work phase.
```

- [ ] **Step 2: CLAUDE.md**

- `timer_engine.py` bullet: extend state list to "stopwatch/countdown/interval" and TickResult fields to include `phase/round/rounds/progress`.
- Conventions: add "- Interval mode: trailing rest skipped, no periodic `sound_interval` beeps, last-5 shorts only (no long at 1) — pinned by tests."

- [ ] **Step 3: Full verification + commit**

```bash
.venv/bin/pytest -q && .venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
git add README.md CLAUDE.md
git commit -m "docs: document interval mode and progress bar"
```

- [ ] **Step 4: Live display check (controller runs — real screen):** launch
with an interval config, screenshot idle + running states; verify bar, round
label, rest tint, and that the settings dialog opens from the menu.
