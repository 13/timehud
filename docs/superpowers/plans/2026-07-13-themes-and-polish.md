# Theme System + Visual Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Four selectable visual themes (Classic default = current look, Terminal, Glass, Mono) plus shared polish: tabular digits, color fades, auto-hide controls, last-seconds pulse.

**Architecture:** New Qt-free `themes.py` registry of frozen `Theme` dataclasses. Picking a theme stamps its colors into the existing `Config` color fields; structural values (radius, alphas, scales, fonts, separator) are read live via `get_theme(config.theme)`. Overlay styling is consolidated into one `_apply_styles()` method used by both build and settings-live-update paths.

**Tech Stack:** Python ≥3.10, PyQt6 (QVariantAnimation, QGraphicsOpacityEffect; optional QFont.setFeature on Qt ≥6.7), pytest.

**Spec:** `docs/superpowers/specs/2026-07-13-themes-and-polish-design.md`

## Global Constraints

- `themes.py` has NO Qt imports; all new tests run headless.
- Classic theme must render pixel-identical to today: `#000000`/alpha 185 bg, border alpha 38, radius 13, clock `#00FF88`, timer `#FFFFFF`/`#888888`, warn `#FF9900`, end `#FF3333`, separator on, scales 1.0/1.25. Guarded by test.
- `Config.theme` defaults to `"classic"`; old configs load unchanged.
- Unknown theme name → classic (never crash).
- Users' manual color tweaks sit on top of stamped theme colors.
- Line numbers below refer to the current files; locate by content if drifted.
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Theme registry + config field

**Files:**
- Create: `src/timehud/themes.py`
- Modify: `src/timehud/config.py` (add `theme` field)
- Test: `tests/test_themes.py`

**Interfaces:**
- Consumes: `Config` dataclass.
- Produces (used by Tasks 2–5):
  - `Theme` frozen dataclass with fields: `name, label, font_family, color_bg, bg_alpha, border_alpha, top_edge_alpha, radius, color_clock, clock_alpha, color_timer_run, color_timer_pause, color_warn, color_end, show_separator, clock_scale, timer_scale`
  - `THEMES: dict[str, Theme]` with keys `classic, terminal, glass, mono`
  - `get_theme(name: str) -> Theme` (fallback classic)
  - `apply_theme(config, name: str) -> Theme` (stamps colors, sets `config.theme`)
  - `Config.theme: str = "classic"`

- [ ] **Step 1: Write failing tests**

`tests/test_themes.py`:

```python
from timehud.config import Config
from timehud.themes import THEMES, apply_theme, get_theme


class TestRegistry:
    def test_four_builtin_themes(self):
        assert set(THEMES) == {"classic", "terminal", "glass", "mono"}

    def test_names_match_keys(self):
        for key, theme in THEMES.items():
            assert theme.name == key

    def test_get_theme_fallback(self):
        assert get_theme("nope") is THEMES["classic"]
        assert get_theme("glass") is THEMES["glass"]


class TestClassicMatchesLegacyLook:
    """Guards the zero-change promise for existing users."""

    def test_classic_values(self):
        t = THEMES["classic"]
        assert t.color_bg == "#000000"
        assert t.bg_alpha == 185
        assert t.border_alpha == 38
        assert t.top_edge_alpha == 0
        assert t.radius == 13
        assert t.color_clock == "#00FF88"
        assert t.clock_alpha == 1.0
        assert t.color_timer_run == "#FFFFFF"
        assert t.color_timer_pause == "#888888"
        assert t.color_warn == "#FF9900"
        assert t.color_end == "#FF3333"
        assert t.show_separator is True
        assert t.clock_scale == 1.0
        assert t.timer_scale == 1.25
        assert t.font_family == ""


class TestApplyTheme:
    def test_stamps_colors_and_sets_theme(self):
        cfg = Config()
        apply_theme(cfg, "mono")
        assert cfg.theme == "mono"
        assert cfg.color_bg == THEMES["mono"].color_bg
        assert cfg.color_clock == THEMES["mono"].color_clock
        assert cfg.color_timer_run == THEMES["mono"].color_timer_run
        assert cfg.color_timer_pause == THEMES["mono"].color_timer_pause
        assert cfg.font_family == "Monospace"

    def test_classic_keeps_user_font(self):
        cfg = Config(font_family="JetBrains Mono")
        apply_theme(cfg, "classic")
        assert cfg.font_family == "JetBrains Mono"

    def test_unknown_name_falls_back_to_classic(self):
        cfg = Config()
        apply_theme(cfg, "does-not-exist")
        assert cfg.theme == "classic"

    def test_default_config_theme_is_classic(self):
        assert Config().theme == "classic"
```

- [ ] **Step 2: Run, verify failure**

Run: `.venv/bin/pytest tests/test_themes.py -v`
Expected: `ModuleNotFoundError: No module named 'timehud.themes'`

- [ ] **Step 3: Implement**

`src/timehud/themes.py`:

```python
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
    show_separator: bool
    clock_scale: float   # clock px = font_size * clock_scale
    timer_scale: float   # timer px = font_size * timer_scale


THEMES: dict = {
    "classic": Theme(
        name="classic", label="Classic",
        font_family="",
        color_bg="#000000", bg_alpha=185,
        border_alpha=38, top_edge_alpha=0, radius=13,
        color_clock="#00FF88", clock_alpha=1.0,
        color_timer_run="#FFFFFF", color_timer_pause="#888888",
        color_warn="#FF9900", color_end="#FF3333",
        show_separator=True, clock_scale=1.0, timer_scale=1.25,
    ),
    "terminal": Theme(
        name="terminal", label="Terminal",
        font_family="Monospace",
        color_bg="#0A0C0B", bg_alpha=199,
        border_alpha=26, top_edge_alpha=0, radius=14,
        color_clock="#4ADE80", clock_alpha=0.92,
        color_timer_run="#F4F6F5", color_timer_pause="#6E7672",
        color_warn="#FBBF24", color_end="#FF3333",
        show_separator=True, clock_scale=0.9, timer_scale=1.3,
    ),
    "glass": Theme(
        name="glass", label="Glass",
        font_family="Noto Sans",
        color_bg="#101214", bg_alpha=168,
        border_alpha=36, top_edge_alpha=56, radius=18,
        color_clock="#E8EDEB", clock_alpha=0.68,
        color_timer_run="#FFFFFF", color_timer_pause="#8A9490",
        color_warn="#FBBF24", color_end="#FF3333",
        show_separator=False, clock_scale=0.8, timer_scale=1.4,
    ),
    "mono": Theme(
        name="mono", label="Mono",
        font_family="Monospace",
        color_bg="#000000", bg_alpha=217,
        border_alpha=0, top_edge_alpha=0, radius=10,
        color_clock="#FFFFFF", clock_alpha=0.55,
        color_timer_run="#FFFFFF", color_timer_pause="#7A7A7A",
        color_warn="#FBBF24", color_end="#FF3333",
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
```

`src/timehud/config.py` — add to the dataclass (after the Presets section):

```python
    # ── Theme ──────────────────────────────────────────────────────────────
    theme: str = "classic"   # built-in theme name; see timehud/themes.py
```

- [ ] **Step 4: Run all tests**

Run: `.venv/bin/pytest -q`
Expected: all PASS (26 existing + 10 new)

- [ ] **Step 5: Commit**

```bash
git add src/timehud/themes.py src/timehud/config.py tests/test_themes.py
git commit -m "feat: add theme registry with classic/terminal/glass/mono"
```

---

### Task 2: Overlay consumes theme (classic stays pixel-identical)

**Files:**
- Modify: `src/timehud/overlay.py` — module constants (~39-46), `_build_ui` (~120-155), `_update` colors (~286-291), `paintEvent` (~362-375), `update_ui` closure in `_open_settings` (~599-655)

**Interfaces:**
- Consumes: `get_theme` (Task 1).
- Produces: `OverlayWindow._apply_styles()` (used by Task 3's `_set_theme` and Task 4's font changes); module helper `_rgba(hex_color, alpha) -> str`.

- [ ] **Step 1: Imports and constants**

Add import:

```python
from timehud.themes import get_theme
```

Delete module constants `_BG`, `_BORDER`, `_CLK_COLOR`, `_TMR_WARN`, `_TMR_END` (keep `_SEP_COLOR`, `_BTN_STYLE`, `_MENU_STYLE`). Add module helper next to `_fmt` at the bottom:

```python
def _rgba(hex_color: str, alpha: float) -> str:
    """'#RRGGBB' + 0-1 alpha → Qt stylesheet rgba() string."""
    h = hex_color.lstrip("#")
    r, g, b = (int(h[i:i + 2], 16) for i in (0, 2, 4))
    return f"rgba({r},{g},{b},{int(alpha * 255)})"
```

- [ ] **Step 2: Add `_apply_styles` and slim `_build_ui`**

New method after `_build_ui`:

```python
    def _apply_styles(self) -> None:
        """Apply theme + config fonts/colors to the labels. Idempotent."""
        cfg = self.config
        theme = get_theme(cfg.theme)
        fs = cfg.font_size
        ff = theme.font_family or cfg.font_family or "Monospace"

        def make_font(size: int, bold: bool = True) -> QFont:
            f = QFont(ff, -1)
            f.setPixelSize(size)
            f.setBold(bold)
            return f

        self.lbl_clock.setFont(make_font(int(fs * theme.clock_scale)))
        self.lbl_clock.setStyleSheet(
            f"color:{_rgba(cfg.color_clock, theme.clock_alpha)}; background:transparent;"
        )
        self.lbl_timer.setFont(make_font(int(fs * theme.timer_scale)))
        self.lbl_mode.setFont(make_font(max(10, fs // 3), bold=False))
        self.sep.setVisible(cfg.show_timer and theme.show_separator)
```

In `_build_ui`: remove the local `make_font` and every `setFont`/clock-`setStyleSheet` call on `lbl_clock`/`lbl_timer`/`lbl_mode` (keep the mode label's gray letter-spacing stylesheet and the timer's initial pause-color stylesheet — the timer stylesheet is overwritten each tick anyway). At the end of `_build_ui` (after `self.sep = sep`, before `_refresh_mode_label`), call:

```python
        self._apply_styles()
```

Note: `_apply_styles` needs `self.sep`, so the call must come after `self.sep = sep`.

- [ ] **Step 3: Theme-driven `_update` colors and `paintEvent`**

`_update` colors dict becomes:

```python
        theme = get_theme(self.config.theme)
        colors = {
            "run":   self.config.color_timer_run,
            "pause": self.config.color_timer_pause,
            "warn":  theme.color_warn,
            "end":   theme.color_end,
        }
```

`paintEvent` becomes:

```python
    def paintEvent(self, _event) -> None:  # noqa: N802
        """Draw the themed rounded background."""
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        theme = get_theme(self.config.theme)
        r = theme.radius
        path = QPainterPath()
        path.addRoundedRect(0, 0, self.width(), self.height(), r, r)
        bg = QColor(self.config.color_bg)
        bg.setAlpha(theme.bg_alpha)
        p.fillPath(path, bg)
        if theme.border_alpha > 0:
            pen = QPen(QColor(255, 255, 255, theme.border_alpha))
            pen.setWidthF(1.0)
            p.setPen(pen)
            p.drawPath(path)
        if theme.top_edge_alpha > 0:
            p.setPen(QPen(QColor(255, 255, 255, theme.top_edge_alpha)))
            p.drawLine(r, 1, self.width() - r, 1)
```

- [ ] **Step 4: Deduplicate `update_ui` in `_open_settings`**

Replace the font/style block of the `update_ui` closure with `self._apply_styles()`; keep visibility, button sizing, opacity, position, tray logic:

```python
        def update_ui():
            cfg = self.config
            if not cfg.show_timer:
                self.reset_timer()

            self._apply_styles()

            self.lbl_clock.setVisible(cfg.show_clock)
            self.lbl_timer.setVisible(cfg.show_timer)
            self.lbl_mode.setVisible(cfg.show_timer)
            self.ctrl_widget.setVisible(cfg.show_timer and cfg.show_controls)

            fs = cfg.font_size
            for btn in (self.btn_start, self.btn_reset, self.btn_mode):
                btn.setFixedHeight(max(24, fs - 4))
            self.btn_start.setFixedWidth(max(24, fs - 4) + 6)
            self.btn_reset.setFixedWidth(max(24, fs - 4) + 6)
            self.btn_mode.setFixedWidth(max(24, fs - 4) + 14)

            self.setWindowOpacity(cfg.opacity)
            self.adjustSize()
            if cfg.custom_x < 0:
                self._position_window()
            self._refresh_mode_label()

            if cfg.show_tray_icon != self._last_show_tray_icon:
                self._last_show_tray_icon = cfg.show_tray_icon
                if self._on_tray_icon_toggle is not None:
                    self._on_tray_icon_toggle(cfg.show_tray_icon)

            self.update()
```

(The old `sep.setVisible(cfg.show_timer)` line is gone — `_apply_styles` owns it now, adding the theme's `show_separator` condition.)

- [ ] **Step 5: Verify**

```bash
.venv/bin/pytest -q
.venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

Expected: 36 tests pass, ruff clean, SMOKE-OK. Classic is the default so the rendering path exercises theme lookups with legacy values.

- [ ] **Step 6: Commit**

```bash
git add src/timehud/overlay.py
git commit -m "refactor: drive overlay styling from theme registry"
```

---

### Task 3: Theme selectors (context menu + settings)

**Files:**
- Modify: `src/timehud/overlay.py` — `create_context_menu` (insert after the Presets sub-menu block, before `menu.addSeparator()`), new `_set_theme` method near `_set_opacity`
- Modify: `src/timehud/settings_dialog.py` — `_display_tab`, `_load_values`, `_connect_live_updates`

**Interfaces:**
- Consumes: `THEMES`, `apply_theme` (Task 1); `_apply_styles` (Task 2).
- Produces: `OverlayWindow._set_theme(name: str)`; `SettingsDialog.theme_combo`.

- [ ] **Step 1: Context-menu submenu**

Extend the overlay import to `from timehud.themes import THEMES, apply_theme, get_theme`. In `create_context_menu`, after the Presets sub-menu block (after the `act_manage` lines, before `menu.addSeparator()`):

```python
        # Theme sub-menu
        theme_menu = menu.addMenu("🎨  Theme")
        theme_group = QActionGroup(theme_menu)
        theme_group.setExclusive(True)
        for t in THEMES.values():
            a: QAction = theme_menu.addAction(t.label)
            a.setCheckable(True)
            a.setChecked(t.name == self.config.theme)
            theme_group.addAction(a)
            a.triggered.connect(lambda checked, n=t.name: self._set_theme(n))
```

- [ ] **Step 2: `_set_theme` method (place next to `_set_opacity`)**

```python
    def _set_theme(self, name: str) -> None:
        apply_theme(self.config, name)
        self._apply_styles()
        self.adjustSize()
        if self.config.custom_x < 0:
            self._position_window()
        self.config.save()
        self.update()      # repaint themed background
        self._update()     # refresh timer color immediately
```

- [ ] **Step 3: Settings combo**

`settings_dialog.py`: import `from timehud.themes import THEMES, apply_theme`. In `_display_tab`, before the Position row:

```python
        self.theme_combo = QComboBox()
        for t in THEMES.values():
            self.theme_combo.addItem(t.label, t.name)
        form.addRow("Theme:", self.theme_combo)
```

In `_load_values`:

```python
        idx = self.theme_combo.findData(c.theme)
        self.theme_combo.setCurrentIndex(max(0, idx))
```

In `_connect_live_updates`, add:

```python
        def _on_theme_changed():
            name = self.theme_combo.currentData()
            if name and name != self.config.theme:
                apply_theme(self.config, name)
                self._update_color_btn(self.btn_color_bg, self.config.color_bg)
                self._update_color_btn(self.btn_color_clock, self.config.color_clock)
                self._update_color_btn(self.btn_color_timer_run, self.config.color_timer_run)
                self._update_color_btn(self.btn_color_timer_pause, self.config.color_timer_pause)
                self.font_family_edit.setText(self.config.font_family)
                self.config_changed.emit()

        self.theme_combo.currentIndexChanged.connect(_on_theme_changed)
```

Important: `_apply_to_config` must not touch `config.theme` — no change needed there; verify nothing overwrites it.

- [ ] **Step 4: Verify**

```bash
.venv/bin/pytest -q
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
QT_QPA_PLATFORM=offscreen .venv/bin/python -c "
from PyQt6.QtWidgets import QApplication
from timehud.config import Config
from timehud.settings_dialog import SettingsDialog
app = QApplication([])
d = SettingsDialog(Config())
assert d.theme_combo.currentData() == 'classic'
d.theme_combo.setCurrentIndex([d.theme_combo.itemData(i) for i in range(d.theme_combo.count())].index('mono'))
assert d.config.theme == 'mono'
assert d.config.color_clock == '#FFFFFF'
print('THEME-COMBO-OK')
"
```

Expected: tests pass, SMOKE-OK, THEME-COMBO-OK.

- [ ] **Step 5: Commit**

```bash
git add src/timehud/overlay.py src/timehud/settings_dialog.py
git commit -m "feat: theme selector in context menu and settings"
```

---

### Task 4: Tabular digits + color fades

**Files:**
- Modify: `src/timehud/overlay.py` — `_apply_styles` (Task 2 version), `_update`, `__init__`, imports

**Interfaces:**
- Consumes: `_apply_styles`, `_update` from Task 2.
- Produces: `_set_timer_color(color: str, animate: bool)`, module helper `_tabular(font) -> QFont` (Task 5's pulse reuses the timer font).

- [ ] **Step 1: Tabular digits helper**

Module-level, next to `_rgba`:

```python
def _tabular(font: QFont) -> QFont:
    """Enable tabular (fixed-width) digits where supported (Qt >= 6.7)."""
    try:
        font.setFeature(QFont.Tag("tnum"), 1)
    except (AttributeError, TypeError):
        pass  # older Qt: mono fonts are tabular anyway
    return font
```

In `_apply_styles`'s `make_font`, change `return f` to `return _tabular(f)`.

- [ ] **Step 2: Color fade state + methods**

Imports: add `QVariantAnimation` to the `PyQt6.QtCore` import list. `__init__` (after `self.engine = ...`):

```python
        # Timer label color animation
        self._timer_color_target = ""
        self._timer_color_current = QColor(config.color_timer_pause)
        self._timer_color_anim: QVariantAnimation | None = None
```

New methods after `_update`:

```python
    def _set_timer_color(self, color: str, animate: bool) -> None:
        if color == self._timer_color_target:
            return
        self._timer_color_target = color
        if self._timer_color_anim is not None:
            self._timer_color_anim.stop()
            self._timer_color_anim = None
        if not animate:
            self._timer_color_current = QColor(color)
            self._paint_timer_color(self._timer_color_current)
            return
        anim = QVariantAnimation(self)
        anim.setDuration(200)
        anim.setStartValue(self._timer_color_current)
        anim.setEndValue(QColor(color))
        anim.valueChanged.connect(self._on_timer_color_step)
        anim.start()
        self._timer_color_anim = anim

    def _on_timer_color_step(self, value) -> None:
        self._timer_color_current = value
        self._paint_timer_color(value)

    def _paint_timer_color(self, qcolor) -> None:
        self.lbl_timer.setStyleSheet(
            f"color:{qcolor.name()}; background:transparent;"
        )
```

- [ ] **Step 3: Use it in `_update`**

Replace the direct `self.lbl_timer.setStyleSheet(...)` line with:

```python
        # Crisp flashes during the countdown's final seconds; fade otherwise
        animate = result.state != "end" and not (
            self.config.timer_mode == "countdown" and result.display <= 6.5
        )
        self._set_timer_color(colors[result.state], animate)
```

- [ ] **Step 4: Verify**

```bash
.venv/bin/pytest -q
.venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

- [ ] **Step 5: Commit**

```bash
git add src/timehud/overlay.py
git commit -m "feat: tabular digits and animated timer color fades"
```

---

### Task 5: `is_idle` + auto-hide controls + last-seconds pulse

**Files:**
- Modify: `src/timehud/timer_engine.py` (add `is_idle`)
- Modify: `tests/test_timer_engine.py` (append tests)
- Modify: `src/timehud/overlay.py` (auto-hide + pulse)

**Interfaces:**
- Consumes: `TimerEngine`, `_set_timer_color`/theme scales from earlier tasks.
- Produces: `TimerEngine.is_idle() -> bool`.

- [ ] **Step 1: Failing tests for `is_idle`**

Append to `tests/test_timer_engine.py`:

```python
class TestIsIdle:
    def test_idle_at_reset(self, engine):
        assert engine.is_idle() is True

    def test_not_idle_while_running(self, engine, clock):
        engine.toggle()
        clock.advance(1)
        assert engine.is_idle() is False

    def test_not_idle_when_paused_midway(self, engine, clock):
        engine.toggle()
        clock.advance(5)
        engine.toggle()
        assert engine.is_idle() is False

    def test_idle_again_after_reset(self, engine, clock):
        engine.toggle()
        clock.advance(5)
        engine.reset()
        assert engine.is_idle() is True

    def test_idle_countdown_at_full_duration(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 60
        from timehud.timer_engine import TimerEngine
        e = TimerEngine(config, clock=clock)
        assert e.is_idle() is True
        e.toggle()
        clock.advance(3)
        e.toggle()
        assert e.is_idle() is False
```

- [ ] **Step 2: Run, verify failure**

Run: `.venv/bin/pytest tests/test_timer_engine.py -k IsIdle -v`
Expected: FAIL with `AttributeError: 'TimerEngine' object has no attribute 'is_idle'`

- [ ] **Step 3: Implement `is_idle` (in `timer_engine.py`, Queries section)**

```python
    def is_idle(self) -> bool:
        """True when stopped at the initial position (never started or reset)."""
        if self.running:
            return False
        if self.config.timer_mode == "stopwatch":
            return self._elapsed == 0.0
        return self._cd_remaining == float(self.config.countdown_duration)
```

Run: `.venv/bin/pytest tests/test_timer_engine.py -v` → all pass.

- [ ] **Step 4: Auto-hide controls (overlay.py)**

Imports: add `QGraphicsOpacityEffect` to the `PyQt6.QtWidgets` import list. `__init__` (after the color-anim fields):

```python
        # Auto-hide controls
        self._controls_fx: QGraphicsOpacityEffect | None = None
        self._controls_anim: QVariantAnimation | None = None
        self._hide_controls_timer = QTimer(self)
        self._hide_controls_timer.setSingleShot(True)
        self._hide_controls_timer.setInterval(2000)
        self._hide_controls_timer.timeout.connect(lambda: self._fade_controls_to(0.0))
```

At the end of `_build_ui` (after `self._apply_styles()`):

```python
        self._controls_fx = QGraphicsOpacityEffect(self.ctrl_widget)
        self._controls_fx.setOpacity(1.0)
        self.ctrl_widget.setGraphicsEffect(self._controls_fx)
```

New methods (near `toggle_visibility`):

```python
    def enterEvent(self, event) -> None:  # noqa: N802
        self._hide_controls_timer.stop()
        self._fade_controls_to(1.0)
        super().enterEvent(event)

    def leaveEvent(self, event) -> None:  # noqa: N802
        # Keep buttons visible while idle at 00:00 for discoverability
        if not self.engine.is_idle():
            self._hide_controls_timer.start()
        super().leaveEvent(event)

    def _fade_controls_to(self, target: float) -> None:
        if self._controls_fx is None:
            return
        if self._controls_anim is not None:
            self._controls_anim.stop()
        anim = QVariantAnimation(self)
        anim.setDuration(300)
        anim.setStartValue(self._controls_fx.opacity())
        anim.setEndValue(target)
        anim.valueChanged.connect(self._controls_fx.setOpacity)
        anim.start()
        self._controls_anim = anim
```

- [ ] **Step 5: Last-seconds pulse (overlay.py)**

`__init__` addition: `self._pulse_anim: QVariantAnimation | None = None`

New methods (after `_fade_controls_to`):

```python
    def _pulse_timer_label(self) -> None:
        base = int(self.config.font_size * get_theme(self.config.theme).timer_scale)
        if self._pulse_anim is not None:
            self._pulse_anim.stop()
        anim = QVariantAnimation(self)
        anim.setDuration(180)
        anim.setStartValue(base)
        anim.setKeyValueAt(0.5, int(base * 1.06))
        anim.setEndValue(base)
        anim.valueChanged.connect(self._set_timer_px)
        anim.start()
        self._pulse_anim = anim

    def _set_timer_px(self, px) -> None:
        f = self.lbl_timer.font()
        f.setPixelSize(int(px))
        self.lbl_timer.setFont(f)
```

Trigger in `_update`, after the beep-forwarding loop:

```python
        if (
            result.beeps
            and self.config.timer_mode == "countdown"
            and result.display <= 6.5
        ):
            self._pulse_timer_label()
```

- [ ] **Step 6: Verify**

```bash
.venv/bin/pytest -q
.venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

Known risk to note in the report: the pulse resizes the label font, which can nudge window layout for 180 ms. If the final live check (Task 6) shows visible window jumping, the fallback is pulsing label opacity instead of size — do not implement the fallback preemptively.

- [ ] **Step 7: Commit**

```bash
git add src/timehud/timer_engine.py tests/test_timer_engine.py src/timehud/overlay.py
git commit -m "feat: auto-hide controls and last-seconds pulse"
```

---

### Task 6: README, ruff, live verification

**Files:**
- Modify: `README.md` (Settings section + context-menu list)
- Modify: `CLAUDE.md` (architecture: themes.py)

**Interfaces:** none new.

- [ ] **Step 1: README**

Settings JSON example: add `"theme": "classic",` before `"position"`. After the "Position presets" subsection add:

```markdown
### Themes

Right-click → **Theme** (or Settings → Display): `Classic` (default),
`Terminal`, `Glass`, `Mono`. Picking a theme sets the color defaults — you can
still customize individual colors afterwards in Settings.
```

Context-menu bullet list: add `- **Theme** – switch between built-in looks` after the Presets bullet.

- [ ] **Step 2: CLAUDE.md**

In the Architecture section, add after the `timer_engine.py` bullet:

```markdown
- **`themes.py`** — frozen `Theme` dataclass registry (classic/terminal/glass/mono), Qt-free. `apply_theme` stamps colors into config; structure (radius, alphas, scales) is read live via `get_theme(config.theme)`. Classic must stay pixel-identical to the pre-theme look — guarded by `tests/test_themes.py`.
```

- [ ] **Step 3: Full verification**

```bash
.venv/bin/pytest -q
.venv/bin/ruff check src tests
QT_QPA_PLATFORM=offscreen timeout 5 .venv/bin/python -m timehud.main --no-tray; test $? -eq 124 && echo SMOKE-OK
```

- [ ] **Step 4: Live display check (controller runs this — real screen available)**

Launch `./timehud`, screenshot, switch themes via a scripted config stamp (`--reset-config` NOT used; temp HOME instead), screenshot each theme, compare classic against the pre-branch screenshot. Controller task, not subagent.

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "docs: document theme system"
```
