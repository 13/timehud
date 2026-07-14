"""
overlay.py – TimeHUD main window.
Transparent, frameless, always-on-top overlay drawn over fullscreen apps.
Uses Qt.WindowType.X11BypassWindowManagerHint so it sits above browser
fullscreen / video players on X11.
Layout (compact, no title bar):
  ┌─────────────────────────┐
  │  HH:MM:SS   ← clock     │
  │ ─────────────────────── │
  │  00:05:23   ← timer     │
  │  STOPWATCH              │
  │  [▶]  [↺]  [SW]         │
  └─────────────────────────┘

Controls:
  Left-drag  – move the overlay (when NOT click-through)
  Right-click – context menu (settings, click-through, opacity, quit)
  Space       – start / pause timer   (local keyboard, window must be focused)
  R           – reset timer
  Ctrl+Q      – quit
"""
import datetime
from collections.abc import Callable
from dataclasses import asdict
from PyQt6.QtCore import (
    Qt, QPoint, QTimer, QEvent, QEasingCurve, QVariantAnimation
)
from PyQt6.QtGui import (
    QAction, QActionGroup, QColor, QFont, QPainter, QPainterPath, QPen,
    QCursor, QGuiApplication,
)
from PyQt6.QtWidgets import (
    QApplication, QGraphicsOpacityEffect, QHBoxLayout, QLabel, QMenu,
    QPushButton, QVBoxLayout, QWidget,
)
from timehud.config import Config, interval_preset_rounds, valid_presets
from timehud.sound_manager import SoundManager
from timehud.themes import THEMES, apply_theme, get_theme
from timehud.timer_engine import TimerEngine, fmt_seconds

_fmt = fmt_seconds

# ── Palette ────────────────────────────────────────────────────────────────
_SEP_COLOR = "rgba(255,255,255,35)"
_BTN_STYLE = """
QPushButton {
    background: rgba(255,255,255,18);
    color: #CCCCCC;
    border: 1px solid rgba(255,255,255,35);
    border-radius: 5px;
    font-size: 13px;
    font-weight: bold;
}
QPushButton:hover  { background: rgba(255,255,255,38); color:#FFF; }
QPushButton:pressed{ background: rgba(255,255,255,55); }
"""
_MENU_STYLE = """
QMenu {
    background: #1A1A1A; color: #CCCCCC;
    border: 1px solid #333; border-radius: 6px;
    padding: 4px 0;
}
QMenu::item          { padding: 6px 22px; }
QMenu::item:selected { background: #2E2E2E; color: #FFF; }
QMenu::separator     { height: 1px; background: #333; margin: 3px 6px; }
"""


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
        # Visibility before the dedup return: update_ui may have hidden the
        # bar while show_timer was off, with fraction/color unchanged.
        self.setVisible(fraction >= 0.0)
        if fraction == self._fraction and qc == self._color:
            return
        self._fraction = fraction
        self._color = qc
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


class OverlayWindow(QWidget):
    """Transparent HUD overlay: clock + stopwatch / countdown."""

    def __init__(
        self,
        config: Config,
        on_tray_icon_toggle: Callable[[bool], None] | None = None,
    ) -> None:
        super().__init__()
        self.config = config
        self.sound  = SoundManager(config)
        self._on_tray_icon_toggle = on_tray_icon_toggle
        self._last_show_tray_icon = config.show_tray_icon
        # ── Timer state ───────────────────────────────────────────────────
        self.engine = TimerEngine(config)
        # Timer label color animation
        self._timer_color_target = ""
        self._timer_color_current = QColor(config.color_timer_pause)
        self._timer_color_anim: QVariantAnimation | None = None
        # Auto-hide controls
        self._controls_fx: QGraphicsOpacityEffect | None = None
        self._controls_anim: QVariantAnimation | None = None
        self._controls_pos = 1.0   # 1.0 = fully unfolded, 0.0 = folded away
        self._hide_controls_timer = QTimer(self)
        self._hide_controls_timer.setSingleShot(True)
        self._hide_controls_timer.setInterval(2000)
        self._hide_controls_timer.timeout.connect(lambda: self._fade_controls_to(0.0))
        # Last-seconds pulse
        self._pulse_anim: QVariantAnimation | None = None
        # Track countdown duration to detect settings-driven changes
        self._last_cd_duration = config.countdown_duration
        self._interval_label = ""
        self._last_interval_cfg = (
            config.interval_work,
            config.interval_rest,
            config.interval_rounds,
        )
        # ── Drag state ────────────────────────────────────────────────────
        self._drag_offset: QPoint | None = None
        # ── Build ─────────────────────────────────────────────────────────
        self._apply_window_flags()
        self._build_ui()
        self._position_window()
        # ── Update loop (100 ms ≈ 10 fps, plenty for a HUD) ───────────────
        self._tick = QTimer(self)
        self._tick.timeout.connect(self._update)
        self._tick.start(100)
        # ── Fade-in ───────────────────────────────────────────────────────
        self._fade_value = 0.0
        self.setWindowOpacity(0.0)
        self._fade_timer = QTimer(self)
        self._fade_timer.timeout.connect(self._fade_step)
        self._fade_timer.start(20)   # ~50 fps for smooth fade

        self._old_size = None

    # ══ Window setup ════════════════════════════════════════════════════════
    def _apply_window_flags(self) -> None:
        """Set Qt flags for frameless transparent always-on-top overlay."""
        flags = (
            Qt.WindowType.FramelessWindowHint         # no title bar / borders
            | Qt.WindowType.WindowStaysOnTopHint      # always on top
            | Qt.WindowType.Tool                      # no taskbar entry
            | Qt.WindowType.X11BypassWindowManagerHint  # sit above fullscreen
        )
        if self.config.click_through:
            # Window ignores all mouse events → clicks pass through to app below
            flags |= Qt.WindowType.WindowTransparentForInput
        self.setWindowFlags(flags)
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)
        self.setAttribute(Qt.WidgetAttribute.WA_X11DoNotAcceptFocus)
    # ══ UI construction ══════════════════════════════════════════════════════
    def _build_ui(self) -> None:
        root = self.layout()
        if root is None:
            root = QVBoxLayout(self)
        root.setContentsMargins(16, 12, 16, 10)
        root.setSpacing(0)
        cfg = self.config
        # ── Clock row ─────────────────────────────────────────────────────
        self.lbl_clock = QLabel("--:--:--")
        self.lbl_clock.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.lbl_clock.installEventFilter(self)
        # ── Separator ─────────────────────────────────────────────────────
        sep = QLabel()
        sep.setFixedHeight(1)
        sep.setStyleSheet(f"background:{_SEP_COLOR}; margin: 5px 0px;")
        # ── Timer display ─────────────────────────────────────────────────
        self.lbl_timer = QLabel("00:00")
        self.lbl_timer.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.lbl_timer.setStyleSheet(f"color:{cfg.color_timer_pause}; background:transparent;")
        self.lbl_timer.setCursor(Qt.CursorShape.PointingHandCursor)
        self.lbl_timer.installEventFilter(self)
        # ── Mode label ────────────────────────────────────────────────────
        self.lbl_mode = QLabel("STOPWATCH")
        self.lbl_mode.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.lbl_mode.setStyleSheet(
            "color:#666; background:transparent; letter-spacing:2px;"
        )
        self.lbl_mode.setCursor(Qt.CursorShape.PointingHandCursor)
        self.lbl_mode.installEventFilter(self)

        # ── Control buttons ───────────────────────────────────────────────
        self.btn_start = QPushButton("▶", self)   # ▶ / ⏸
        self.btn_reset = QPushButton("↺", self)
        self.btn_mode  = QPushButton("SW", self)  # SW / CD

        for btn in (self.btn_start, self.btn_reset, self.btn_mode):
            btn.setStyleSheet(_BTN_STYLE)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            if not cfg.show_controls:
                btn.hide()
        self._apply_button_sizes()
        self.btn_start.clicked.connect(self.toggle_timer)
        self.btn_reset.clicked.connect(self.reset_timer)
        self.btn_mode.clicked.connect(self._toggle_mode)
        ctrl = QHBoxLayout()
        ctrl.setSpacing(6)
        ctrl.setContentsMargins(0, 6, 0, 0)
        ctrl.addStretch()
        ctrl.addWidget(self.btn_start)
        ctrl.addWidget(self.btn_reset)
        ctrl.addWidget(self.btn_mode)
        ctrl.addStretch()
        # ── Assemble ──────────────────────────────────────────────────────
        self.lbl_clock.setVisible(cfg.show_clock)
        root.addWidget(self.lbl_clock)
        sep.setVisible(cfg.show_timer)
        root.addWidget(sep)
        self.lbl_timer.setVisible(cfg.show_timer)
        root.addWidget(self.lbl_timer)
        self.progress_bar = _ProgressBar()
        root.addWidget(self.progress_bar)
        self.lbl_mode.setVisible(cfg.show_timer)
        root.addWidget(self.lbl_mode)

        self.ctrl_widget = QWidget()
        self.ctrl_widget.setLayout(ctrl)
        self.ctrl_widget.setVisible(cfg.show_timer and cfg.show_controls)
        root.addWidget(self.ctrl_widget)

        self.sep = sep

        self._apply_styles()
        self._refresh_mode_label()

        self._controls_fx = QGraphicsOpacityEffect(self.ctrl_widget)
        self._controls_fx.setOpacity(1.0)
        self.ctrl_widget.setGraphicsEffect(self._controls_fx)

    def _apply_button_sizes(self) -> None:
        """Size control buttons relative to the configured font size."""
        btn_h = max(24, self.config.font_size - 4)
        for btn in (self.btn_start, self.btn_reset, self.btn_mode):
            btn.setFixedHeight(btn_h)
        self.btn_start.setFixedWidth(btn_h + 6)
        self.btn_reset.setFixedWidth(btn_h + 6)
        self.btn_mode.setFixedWidth(btn_h + 14)

    def _apply_styles(self) -> None:
        """Apply theme + config fonts/colors to the labels. Idempotent."""
        if self._pulse_anim is not None:
            self._pulse_anim.stop()
            self._pulse_anim.deleteLater()
            self._pulse_anim = None
        cfg = self.config
        theme = get_theme(cfg.theme)
        fs = cfg.font_size
        ff = theme.font_family or cfg.font_family or "Monospace"

        def make_font(size: int, bold: bool = True) -> QFont:
            f = QFont(ff, -1)
            f.setPixelSize(size)
            f.setBold(bold)
            return _tabular(f)

        self.lbl_clock.setFont(make_font(int(fs * theme.clock_scale)))
        self.lbl_clock.setStyleSheet(
            f"color:{_rgba(cfg.color_clock, theme.clock_alpha)}; background:transparent;"
        )
        self.lbl_timer.setFont(make_font(int(fs * theme.timer_scale)))
        self.lbl_mode.setFont(make_font(max(10, fs // 3), bold=False))
        self.sep.setVisible(cfg.show_timer and theme.show_separator)

    # ══ Positioning ══════════════════════════════════════════════════════════
    def _position_window(self) -> None:
        self.adjustSize()
        w, h = self.width(), self.height()
        m    = self.config.margin
        screen = QGuiApplication.screenAt(QCursor.pos())
        if not screen:
            screen = QApplication.primaryScreen()
        scr  = screen.availableGeometry()
        # Use saved drag position if present
        if self.config.custom_x >= 0 and self.config.custom_y >= 0:
            self.move(self.config.custom_x, self.config.custom_y)
            return
        presets = {
            "top-left":      (m,                     m),
            "top-right":     (scr.width()  - w - m,  m),
            "bottom-left":   (m,                     scr.height() - h - m),
            "bottom-right":  (scr.width()  - w - m,  scr.height() - h - m),
            "top-center":    ((scr.width() - w) // 2, m),
            "bottom-center": ((scr.width() - w) // 2, scr.height() - h - m),
        }
        x, y = presets.get(self.config.position, (scr.width() - w - m, m))

        new_pos = QPoint(scr.x() + x, scr.y() + y)
        if self.isVisible():
            self.move(new_pos)
        else:
            self.move(new_pos)

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        
        if not self.isVisible():
            return
            
        old_sz = event.oldSize()
        new_sz = event.size()
        
        if not old_sz.isValid() or old_sz == new_sz:
            return
            
        dw = new_sz.width() - old_sz.width()
        dh = new_sz.height() - old_sz.height()

        # If using presets, _position_window handles it correctly on its own
        # but calling it constantly might not be ideal. We can manually offset
        # based on screen quadrants to preserve real "anchor" feeling anywhere.
        screen = QGuiApplication.screenAt(self.geometry().center())
        if not screen:
            screen = QApplication.primaryScreen()
            
        scr = screen.availableGeometry()
        cx = self.geometry().center().x()
        cy = self.geometry().center().y()
        
        dx, dy = 0, 0
        
        # If in the right half of the screen, anchor to the right
        if cx > scr.center().x():
            dx = -dw
            
        # If in the bottom half of the screen, anchor to the bottom
        if cy > scr.center().y():
            dy = -dh
            
        if dx != 0 or dy != 0:
            self.move(self.pos() + QPoint(dx, dy))

    # ══ Update loop ══════════════════════════════════════════════════════════
    def _update(self) -> None:
        """Called every 100 ms to refresh the display."""
        now = datetime.datetime.now()
        if self.config.show_clock:
            self.lbl_clock.setText(now.strftime("%H:%M:%S"))
        if not self.config.show_timer:
            return

        result = self.engine.tick()
        self.lbl_timer.setText(_fmt(result.display))

        theme = get_theme(self.config.theme)
        colors = {
            "run":   self.config.color_timer_run,
            "pause": self.config.color_timer_pause,
            "warn":  theme.color_warn,
            "end":   theme.color_end,
        }
        # Crisp flashes during the countdown's final seconds; fade otherwise
        animate = result.state != "end" and not (
            self.config.timer_mode == "countdown" and result.display <= 6.5
        )
        color = colors[result.state]
        if result.phase == "rest" and result.state == "run":
            color = theme.color_rest
        self._set_timer_color(color, animate)

        theme_bar_color = theme.color_clock
        if result.state in ("warn", "end"):
            theme_bar_color = theme.color_warn
        elif result.phase == "rest":
            theme_bar_color = theme.color_rest
        self.progress_bar.set_state(result.progress, theme_bar_color)

        if result.phase and not self.engine.is_idle():
            label = f"{result.phase.upper()} {result.round}/{result.rounds}"
            if label != self._interval_label:
                self._interval_label = label
                self.lbl_mode.setText(label)
        else:
            self._interval_label = ""

        if result.finished and not result.restarted:
            self.btn_start.setText("▶")

        for beep in result.beeps:
            if beep.double:
                self.sound.play_alert(double_beep=True)
            else:
                self.sound.play_alert(short=beep.short)

        if (
            result.beeps
            and self.config.timer_mode == "countdown"
            and result.display <= 6.5
        ):
            self._pulse_timer_label()

    def _set_timer_color(self, color: str, animate: bool) -> None:
        if color == self._timer_color_target:
            return
        self._timer_color_target = color
        if self._timer_color_anim is not None:
            self._timer_color_anim.stop()
            self._timer_color_anim.deleteLater()
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
    # ══ Timer logic ══════════════════════════════════════════════════════════
    def toggle_timer(self) -> None:
        """Start if stopped, pause if running."""
        self.engine.toggle()
        self.btn_start.setText("⏸" if self.engine.running else "▶")
    def reset_timer(self) -> None:
        """Stop and zero the timer."""
        self.engine.reset()
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self._update()
    def _sync_countdown_duration(self) -> None:
        """Resync engine remaining after countdown_duration changed in Settings."""
        if self.config.countdown_duration != self._last_cd_duration:
            self._last_cd_duration = self.config.countdown_duration
            if not self.engine.running:
                self.engine.adjust_countdown(0)   # stopped: reload from config
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
    def _toggle_mode(self) -> None:
        """Cycle stopwatch → countdown → interval."""
        modes = ["stopwatch", "countdown", "interval"]
        curr = modes.index(self.config.timer_mode) if self.config.timer_mode in modes else 0
        self.config.active_preset = ""   # manual mode change ends the preset
        self.engine.set_mode(modes[(curr + 1) % len(modes)])
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self.config.save()
        self._update()
    def _apply_stopwatch(self) -> None:
        self.engine.set_mode("stopwatch")
        self.btn_start.setText("▶")
        self._refresh_mode_label()
        self.config.save()
        self._update()
    def _apply_preset(self, preset: dict) -> None:
        if preset.get("type") == "interval":
            self.config.timer_mode = "interval"
            self.config.interval_work = int(preset["work"])
            self.config.interval_rest = int(preset["rest"])
            self.config.interval_rounds = interval_preset_rounds(preset)
            self._last_interval_cfg = (
                self.config.interval_work,
                self.config.interval_rest,
                self.config.interval_rounds,
            )
        else:
            self.config.timer_mode = "countdown"
            self.config.countdown_duration = int(preset["duration"])
            self._last_cd_duration = self.config.countdown_duration
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
        if self.config.timer_mode == "interval":
            cfg = self.config
            new_preset = {
                "name": name,
                "type": "interval",
                "work": cfg.interval_work,
                "rest": cfg.interval_rest,
                "total": (cfg.interval_work + cfg.interval_rest) * cfg.interval_rounds,
            }
        else:
            new_preset = {"name": name, "duration": int(self.config.countdown_duration)}
        # Same-name preset is overwritten (predictable rule per spec)
        presets = [p for p in valid_presets(self.config.presets) if p["name"] != name]
        presets.append(new_preset)
        self.config.presets = presets
        self.config.active_preset = name
        self._refresh_mode_label()
        self.config.save()
    def _refresh_mode_label(self) -> None:
        self._interval_label = ""   # force live label re-sync on next tick
        if self.config.timer_mode == "stopwatch":
            self.lbl_mode.setText("STOPWATCH")
            self.btn_mode.setText("SW")
        elif self.config.timer_mode == "interval":
            cfg = self.config
            base = f"{cfg.interval_work}s/{cfg.interval_rest}s ×{cfg.interval_rounds}"
            if cfg.active_preset:
                self.lbl_mode.setText(f"{cfg.active_preset.upper()} · {base}")
            else:
                self.lbl_mode.setText(f"INTERVAL {base}")
            self.btn_mode.setText("IV")
        else:
            dur = _fmt(self.config.countdown_duration)
            if self.config.active_preset:
                self.lbl_mode.setText(f"{self.config.active_preset.upper()}  ·  {dur}")
            else:
                self.lbl_mode.setText(f"COUNTDOWN  {dur}")
            self.btn_mode.setText("CD")
    # ══ Painting ═════════════════════════════════════════════════════════════
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
    # ══ Mouse events ═════════════════════════════════════════════════════════
    def eventFilter(self, obj, event):
        if obj is self.lbl_clock:
            if event.type() == QEvent.Type.Wheel:
                delta = event.angleDelta().y()
                step = 2 if delta > 0 else -2
                new_size = max(10, min(120, self.config.font_size + step))
                if new_size != self.config.font_size:
                    self.config.font_size = new_size
                    self._apply_styles()
                    self._apply_button_sizes()
                    self.adjustSize()
                    if self.config.custom_x < 0:
                        self._position_window()
                    self.config.save()
                return True
            return super().eventFilter(obj, event)
        if obj in (self.lbl_timer, self.lbl_mode):
            if event.type() == QEvent.Type.MouseButtonPress:
                if event.button() == Qt.MouseButton.LeftButton:
                    if obj == self.lbl_timer:
                        self.toggle_timer()
                elif event.button() == Qt.MouseButton.MiddleButton:
                    self._toggle_mode()
                return True
            elif event.type() == QEvent.Type.MouseButtonDblClick:
                if event.button() == Qt.MouseButton.LeftButton:
                    if obj == self.lbl_timer:
                        self.reset_timer()
                return True
            elif event.type() == QEvent.Type.Wheel:
                if obj == self.lbl_mode:
                    delta_wheel = event.angleDelta().y()
                    pos_x = event.position().x()
                    width = obj.width()

                    if self.config.timer_mode == "countdown" and pos_x > width * 0.55:
                        step = 1 if pos_x > width * 0.8 else 60
                        delta = 0
                        if delta_wheel > 0:
                            delta = step
                        elif delta_wheel < 0:
                            delta = -min(step, self.config.countdown_duration - 1)
                        if delta:
                            self.config.countdown_duration += delta
                            self._last_cd_duration = self.config.countdown_duration
                            self.config.active_preset = ""
                            self.engine.adjust_countdown(delta)
                        self.config.save()
                        self._refresh_mode_label()
                        self._update()
                    else:
                        modes = ["stopwatch", "countdown", "interval"]
                        curr = modes.index(self.config.timer_mode) if self.config.timer_mode in modes else 0
                        new_mode = modes[(curr + (-1 if delta_wheel > 0 else 1)) % len(modes)]
                        if self.config.timer_mode != new_mode:
                            self.config.active_preset = ""
                            self.engine.set_mode(new_mode)
                            self.btn_start.setText("▶")
                            self._refresh_mode_label()
                            self.config.save()
                            self._update()
                elif obj == self.lbl_timer:
                    delta = event.angleDelta().y()
                    modes = ["stopwatch", "countdown", "interval"]
                    curr = modes.index(self.config.timer_mode) if self.config.timer_mode in modes else 0
                    new_mode = modes[(curr + (-1 if delta > 0 else 1)) % len(modes)]
                    if self.config.timer_mode != new_mode:
                        self.config.active_preset = ""
                        self.engine.set_mode(new_mode)
                        self.btn_start.setText("▶")
                        self._refresh_mode_label()
                        self.config.save()
                        self._update()
                return True
        return super().eventFilter(obj, event)
    def mousePressEvent(self, event) -> None:  # noqa: N802
        if event.button() == Qt.MouseButton.LeftButton:
            self._drag_offset = (
                event.globalPosition().toPoint() - self.frameGeometry().topLeft()
            )
            event.accept()
    def mouseMoveEvent(self, event) -> None:  # noqa: N802
        if (
            event.buttons() == Qt.MouseButton.LeftButton
            and self._drag_offset is not None
        ):
            new_pos = event.globalPosition().toPoint() - self._drag_offset

            # Magnetic snapping
            screen = QGuiApplication.screenAt(event.globalPosition().toPoint())
            if not screen:
                screen = QApplication.primaryScreen()
            scr = screen.availableGeometry()
            snap = 20
            w, h = self.width(), self.height()

            if abs(new_pos.x() - scr.left()) < snap:
                new_pos.setX(scr.left() + self.config.margin)
            elif abs(new_pos.x() + w - scr.right()) < snap:
                new_pos.setX(scr.right() - w - self.config.margin)

            if abs(new_pos.y() - scr.top()) < snap:
                new_pos.setY(scr.top() + self.config.margin)
            elif abs(new_pos.y() + h - scr.bottom()) < snap:
                new_pos.setY(scr.bottom() - h - self.config.margin)

            self.move(new_pos)
            event.accept()
    def mouseReleaseEvent(self, event) -> None:  # noqa: N802
        if event.button() == Qt.MouseButton.LeftButton:
            self._drag_offset = None
            # Persist the custom position
            pos = self.frameGeometry().topLeft()
            self.config.custom_x = pos.x()
            self.config.custom_y = pos.y()
            self.config.save()
    def create_context_menu(self, include_window_actions: bool | None = None) -> QMenu:
        """Build and return the context menu (used by both overlay and tray)."""
        menu = QMenu(self)
        self._populate_context_menu(menu, include_window_actions)
        return menu

    def _populate_context_menu(
        self, menu: QMenu, include_window_actions: bool | None = None
    ) -> None:
        if include_window_actions is None:
            include_window_actions = self.config.show_tray_icon
        menu.setStyleSheet(_MENU_STYLE)
        act_settings = menu.addAction("Settings…")
        # Lambda drops QAction.triggered's `checked` bool, which would
        # otherwise land in the optional `tab` parameter.
        act_settings.triggered.connect(lambda: self._open_settings())
        menu.addSeparator()

        # Presets sub-menu
        preset_menu = menu.addMenu("Presets")
        act_sw = preset_menu.addAction("Stopwatch")
        act_sw.setCheckable(True)
        act_sw.setChecked(self.config.timer_mode == "stopwatch")
        act_sw.triggered.connect(self._apply_stopwatch)
        presets = valid_presets(self.config.presets)
        if presets:
            preset_menu.addSeparator()
            for p in presets:
                if p.get("type") == "interval":
                    label = (
                        f'{p["name"]} {p["work"]}/{p["rest"]} ×{interval_preset_rounds(p)}'
                    )
                else:
                    label = f'{p["name"]} {_fmt(p["duration"])}'
                a = preset_menu.addAction(label)
                a.setCheckable(True)
                preset_mode = "interval" if p.get("type") == "interval" else "countdown"
                a.setChecked(
                    self.config.timer_mode == preset_mode
                    and self.config.active_preset == p["name"]
                )
                a.triggered.connect(lambda checked, p=p: self._apply_preset(p))
        preset_menu.addSeparator()
        act_save = preset_menu.addAction("Save current as preset…")
        act_save.setEnabled(self.config.timer_mode in ("countdown", "interval"))
        act_save.triggered.connect(self._save_current_preset)
        act_manage = preset_menu.addAction("Manage presets…")
        act_manage.triggered.connect(lambda: self._open_settings(tab="presets"))

        # Theme sub-menu
        theme_menu = menu.addMenu("Theme")
        theme_group = QActionGroup(theme_menu)
        theme_group.setExclusive(True)
        for t in THEMES.values():
            a: QAction = theme_menu.addAction(t.label)
            a.setCheckable(True)
            a.setChecked(t.name == self.config.theme)
            theme_group.addAction(a)
            a.triggered.connect(lambda checked, n=t.name: self._set_theme(n))
        menu.addSeparator()

        if include_window_actions:
            ct_label = "Click-Through: ON" if self.config.click_through else "Click-Through: OFF"
            act_ct = menu.addAction(ct_label)
            act_ct.triggered.connect(self._toggle_click_through)

        # Opacity sub-menu
        op_menu = menu.addMenu("Opacity")
        op_group = QActionGroup(op_menu)
        op_group.setExclusive(True)
        current_pct = max(0, min(100, round(self.config.opacity * 100)))
        matched_opacity = False
        for pct in (30, 50, 70, 85, 95, 100):
            a: QAction = op_menu.addAction(f"{pct}%")
            a.setCheckable(True)
            op_group.addAction(a)
            if current_pct == pct:
                a.setChecked(True)
                matched_opacity = True
            a.triggered.connect(lambda checked, v=pct/100: self._set_opacity(v))
        if not matched_opacity:
            current_action = op_menu.addAction(f"Current: {current_pct}%")
            current_action.setCheckable(True)
            current_action.setChecked(True)
            op_group.addAction(current_action)
            current_action.triggered.connect(
                lambda checked, v=current_pct / 100: self._set_opacity(v)
            )
        # Position sub-menu
        pos_menu = menu.addMenu("Position")
        pos_group = QActionGroup(pos_menu)
        pos_group.setExclusive(True)
        for preset in (
            "top-left", "top-right",
            "bottom-left", "bottom-right",
            "top-center", "bottom-center",
        ):
            a: QAction = pos_menu.addAction(preset.replace("-", " ").title())
            a.setCheckable(True)
            a.setChecked(preset == self.config.position)
            pos_group.addAction(a)
            a.triggered.connect(lambda checked, p=preset: self._set_preset_position(p))

        if include_window_actions:
            menu.addSeparator()
            act_toggle = menu.addAction("Show/Hide Overlay")
            act_toggle.triggered.connect(self.toggle_visibility)
        act_quit = menu.addAction("Quit")
        act_quit.triggered.connect(self._quit_app)

    def contextMenuEvent(self, event) -> None:  # noqa: N802
        """Right-click context menu."""
        menu = self.create_context_menu()
        menu.exec(event.globalPos())
    def _set_opacity(self, value):
        self.config.opacity = value
        self.setWindowOpacity(value)
        self.config.save()
    def _set_theme(self, name: str) -> None:
        apply_theme(self.config, name)
        self._apply_styles()
        self.adjustSize()
        if self.config.custom_x < 0:
            self._position_window()
        self.config.save()
        self.update()      # repaint themed background
        self._update()     # refresh timer color immediately
    def _set_preset_position(self, preset):
        self.config.position = preset
        self.config.custom_x = -1
        self.config.custom_y = -1
        self._position_window()
        self.config.save()
    def _quit_app(self):
        self.config.save()
        QApplication.quit()
    # ══ Keyboard shortcuts (window must be focused) ═══════════════════════════
    def keyPressEvent(self, event) -> None:  # noqa: N802
        key  = event.key()
        mods = event.modifiers()
        if key == Qt.Key.Key_Space:
            self.toggle_timer()
        elif key == Qt.Key.Key_R:
            self.reset_timer()
        elif key == Qt.Key.Key_Escape:
            self.hide()
        elif key == Qt.Key.Key_Q and mods & Qt.KeyboardModifier.ControlModifier:
            self.config.save()
            QApplication.quit()
    # ══ Helper actions ════════════════════════════════════════════════════════
    def _toggle_click_through(self) -> None:
        """Toggle click-through mode (requires window flag refresh)."""
        pos = self.pos()
        self.config.click_through = not self.config.click_through
        self.config.save()
        self._apply_window_flags()
        self.move(pos)
        self.show()
    def _open_settings(self, tab: str | None = None) -> None:
        # Import here to avoid circular deps / speed up startup
        from timehud.settings_dialog import SettingsDialog

        snapshot = asdict(self.config)   # asdict deep-copies nested containers

        dlg = SettingsDialog(self.config, parent=None)
        if tab is not None:
            dlg.select_tab(tab)

        def update_ui():
            cfg = self.config
            if not cfg.show_timer:
                self.reset_timer()

            self._sync_countdown_duration()
            self._sync_interval_config()
            self._apply_styles()

            self.lbl_clock.setVisible(cfg.show_clock)
            self.lbl_timer.setVisible(cfg.show_timer)
            if not cfg.show_timer:
                self.progress_bar.hide()
            self.lbl_mode.setVisible(cfg.show_timer)
            self.ctrl_widget.setVisible(cfg.show_timer and cfg.show_controls)
            if cfg.show_timer and cfg.show_controls:
                self._reset_controls_fold()

            self._apply_button_sizes()

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

        dlg.config_changed.connect(update_ui)

        if dlg.exec():
            update_ui()
        else:
            # Cancel: live-applied changes are rolled back
            for key, value in snapshot.items():
                setattr(self.config, key, value)
            update_ui()
        self.config.save()

    def toggle_visibility(self) -> None:
        """Show or hide the overlay (used by global hotkeys)."""
        if self.isVisible():
            self.hide()
        else:
            self.show()
    # ══ Auto-hide controls ═══════════════════════════════════════════════════
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
        """Fold (0.0) or unfold (1.0) the control row, reclaiming its space."""
        if self._controls_fx is None or self._controls_pos == target:
            return
        if self._controls_anim is not None:
            self._controls_anim.stop()
            self._controls_anim.deleteLater()
        anim = QVariantAnimation(self)
        anim.setDuration(400)
        anim.setEasingCurve(QEasingCurve.Type.InOutCubic)
        anim.setStartValue(self._controls_pos)
        anim.setEndValue(target)
        anim.valueChanged.connect(self._controls_fold_step)
        anim.start()
        self._controls_anim = anim

    def _controls_fold_step(self, value) -> None:
        pos = float(value)
        self._controls_pos = pos
        self._controls_fx.setOpacity(pos)
        natural = self.ctrl_widget.sizeHint().height()
        self.ctrl_widget.setMaximumHeight(int(natural * pos))
        self.adjustSize()

    def _reset_controls_fold(self) -> None:
        """Instantly restore the unfolded state (settings toggled controls on)."""
        self._hide_controls_timer.stop()
        if self._controls_anim is not None:
            self._controls_anim.stop()
            self._controls_anim.deleteLater()
            self._controls_anim = None
        self._controls_pos = 1.0
        if self._controls_fx is not None:
            self._controls_fx.setOpacity(1.0)
        self.ctrl_widget.setMaximumHeight(16777215)

    def _pulse_timer_label(self) -> None:
        base = int(self.config.font_size * get_theme(self.config.theme).timer_scale)
        if self._pulse_anim is not None:
            self._pulse_anim.stop()
            self._pulse_anim.deleteLater()
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
    # ══ Fade-in animation ═════════════════════════════════════════════════════
    def _fade_step(self) -> None:
        self._fade_value = min(self._fade_value + 0.06, self.config.opacity)
        self.setWindowOpacity(self._fade_value)
        if self._fade_value >= self.config.opacity:
            self._fade_timer.stop()
# ── Helpers ────────────────────────────────────────────────────────────────
def _tabular(font: QFont) -> QFont:
    """Enable tabular (fixed-width) digits where supported (Qt >= 6.7)."""
    try:
        font.setFeature(QFont.Tag("tnum"), 1)
    except (AttributeError, TypeError):
        pass  # older Qt: mono fonts are tabular anyway
    return font
def _rgba(hex_color: str, alpha: float) -> str:
    """'#RRGGBB' + 0-1 alpha → Qt stylesheet rgba() string."""
    h = hex_color.lstrip("#")
    r, g, b = (int(h[i:i + 2], 16) for i in (0, 2, 4))
    return f"rgba({r},{g},{b},{int(alpha * 255)})"
