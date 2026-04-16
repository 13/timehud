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
import time
import math
from PyQt6.QtCore import (
    Qt, QPoint, QTimer, QEvent, QPropertyAnimation, pyqtProperty
)
from PyQt6.QtGui import (
    QColor, QFont, QPainter, QPainterPath, QPen, QCursor, QGuiApplication,
)
from PyQt6.QtWidgets import (
    QApplication, QHBoxLayout, QLabel, QMenu,
    QPushButton, QVBoxLayout, QWidget,
)
from timehud.config import Config
from timehud.sound_manager import SoundManager
# ── Palette ────────────────────────────────────────────────────────────────
_BG        = QColor(0,   0,   0,  185)   # dark translucent background
_BORDER    = QColor(255, 255, 255, 38)   # subtle 1-px border
_CLK_COLOR = "#00FF88"                   # green clock
_TMR_RUN   = "#FFFFFF"                   # timer running
_TMR_PAUSE = "#888888"                   # timer paused
_TMR_WARN  = "#FF9900"                   # ≤ 10 s remaining
_TMR_END   = "#FF3333"                   # countdown finished
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
class OverlayWindow(QWidget):
    """Transparent HUD overlay: clock + stopwatch / countdown."""

    def __init__(self, config: Config) -> None:
        super().__init__()
        self.config = config
        self.sound  = SoundManager(config)
        # ── Timer state ───────────────────────────────────────────────────
        self._running      = False
        self._start_mono   = 0.0   # time.monotonic() when last started
        self._elapsed      = 0.0   # accumulated seconds (stopwatch)
        self._cd_remaining = float(config.countdown_duration)
        # Sound beat counter: fires every sound_interval seconds
        self._sound_beats  = 0
        self._last_short_beep_sec = -1
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
        fs  = cfg.font_size
        ff  = cfg.font_family or "Monospace"
        def make_font(size: int, bold: bool = True) -> QFont:
            f = QFont(ff, -1)
            f.setPixelSize(size)
            f.setBold(bold)
            return f
        # ── Clock row ─────────────────────────────────────────────────────
        self.lbl_clock = QLabel("--:--:--")
        self.lbl_clock.setFont(make_font(fs))
        self.lbl_clock.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.lbl_clock.setStyleSheet(f"color:{_CLK_COLOR}; background:transparent;")
        # ── Separator ─────────────────────────────────────────────────────
        sep = QLabel()
        sep.setFixedHeight(1)
        sep.setStyleSheet(f"background:{_SEP_COLOR}; margin: 5px 0px;")
        # ── Timer display ─────────────────────────────────────────────────
        self.lbl_timer = QLabel("00:00")
        self.lbl_timer.setFont(make_font(int(fs * 1.25)))
        self.lbl_timer.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.lbl_timer.setStyleSheet(f"color:{_TMR_PAUSE}; background:transparent;")
        self.lbl_timer.setCursor(Qt.CursorShape.PointingHandCursor)
        self.lbl_timer.installEventFilter(self)
        # ── Mode label ────────────────────────────────────────────────────
        self.lbl_mode = QLabel("STOPWATCH")
        self.lbl_mode.setFont(make_font(max(10, fs // 3), bold=False))
        self.lbl_mode.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.lbl_mode.setStyleSheet(
            "color:#666; background:transparent; letter-spacing:2px;"
        )

        # ── Control buttons ───────────────────────────────────────────────
        self.btn_start = QPushButton("▶", self)   # ▶ / ⏸
        self.btn_reset = QPushButton("↺", self)
        self.btn_mode  = QPushButton("SW", self)  # SW / CD

        btn_h = max(24, fs - 4)
        for btn in (self.btn_start, self.btn_reset, self.btn_mode):
            btn.setStyleSheet(_BTN_STYLE)
            btn.setFixedHeight(btn_h)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            if not cfg.show_controls:
                btn.hide()

        self.btn_start.setFixedWidth(btn_h + 6)
        self.btn_reset.setFixedWidth(btn_h + 6)
        self.btn_mode.setFixedWidth(btn_h + 14)
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
        self.lbl_mode.setVisible(cfg.show_timer)
        root.addWidget(self.lbl_mode)

        self.ctrl_widget = QWidget()
        self.ctrl_widget.setLayout(ctrl)
        self.ctrl_widget.setVisible(cfg.show_timer and cfg.show_controls)
        root.addWidget(self.ctrl_widget)

        self.sep = sep

        self._refresh_mode_label()

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
        # ── Clock ─────────────────────────────────────────────────────────
        if self.config.show_clock:
            self.lbl_clock.setText(now.strftime("%H:%M:%S"))
        if not self.config.show_timer:
            return
        # ── Timer ─────────────────────────────────────────────────────────
        if self.config.timer_mode == "stopwatch":
            elapsed = self._get_elapsed()
            self.lbl_timer.setText(_fmt(elapsed))
            color = self.config.color_timer_run if self._running else self.config.color_timer_pause
            self.lbl_timer.setStyleSheet(
                f"color:{color}; background:transparent;"
            )
        else:
            remaining = self._get_remaining()
            sec_display = int(math.ceil(max(0.0, remaining)))
            self.lbl_timer.setText(_fmt(max(0.0, remaining)))

            # Custom behavior for the last 5 seconds:
            if remaining <= 0:
                color = _TMR_END
                if self._running:
                    # Countdown finished strictly
                    if self.config.auto_restart_countdown:
                        self._cd_remaining = float(self.config.countdown_duration)
                        self._start_mono  = time.monotonic()
                        self._sound_beats = 0
                        self._last_short_beep_sec = -1
                    else:
                        self._running = False
                        self.btn_start.setText("▶")
            elif self._running and 0 < remaining <= 6.0 and self.config.alert_last_5_seconds:
                if sec_display == 1:
                    color = _TMR_END  # At 1 the color should be red
                else:
                    color = _TMR_WARN # At 2..5 the color should be orange

                if sec_display != self._last_short_beep_sec and sec_display in (1, 2, 3, 4, 5, 6):
                    self._last_short_beep_sec = sec_display
                    if sec_display == 1:
                        self.sound.play_alert(short=False)  # "at 1 it should beep long"
                    else:
                        self.sound.play_alert(short=True)   # Short on 5, 4, 3, 2
            else:
                color = self.config.color_timer_run if self._running else self.config.color_timer_pause

            self.lbl_timer.setStyleSheet(
                f"color:{color}; background:transparent;"
            )
        # ── Periodic sound alerts ─────────────────────────────────────────
        if self._running and self.config.sound_enabled:
            ref     = self._get_elapsed()   # seconds since "start" of this run
            beats   = int(ref / self.config.sound_interval)
            if beats > self._sound_beats:
                self._sound_beats = beats
                self.sound.play_alert()
    # ══ Timer logic ══════════════════════════════════════════════════════════
    def _get_elapsed(self) -> float:
        """Stopwatch: total elapsed seconds."""
        if self._running:
            return self._elapsed + (time.monotonic() - self._start_mono)
        return self._elapsed
    def _get_remaining(self) -> float:
        """Countdown: seconds remaining."""
        if self._running:
            return self._cd_remaining - (time.monotonic() - self._start_mono)
        return self._cd_remaining
    def toggle_timer(self) -> None:
        """Start if stopped, pause if running."""
        if self._running:
            # Pause – snapshot the accumulator
            if self.config.timer_mode == "stopwatch":
                self._elapsed += time.monotonic() - self._start_mono
            else:
                self._cd_remaining -= time.monotonic() - self._start_mono
                self._cd_remaining = max(0.0, self._cd_remaining)
            self._running = False
            self.btn_start.setText("▶")
        else:
            # Guard: don't start a finished countdown
            if self.config.timer_mode == "countdown" and self._cd_remaining <= 0:
                self._cd_remaining = float(self.config.countdown_duration)
            self._start_mono  = time.monotonic()
            self._running     = True
            self._sound_beats = int(self._get_elapsed() / self.config.sound_interval)
            self.btn_start.setText("⏸")
    def reset_timer(self) -> None:
        """Stop and zero the timer."""
        self._running      = False
        self._elapsed      = 0.0
        self._cd_remaining = float(self.config.countdown_duration)
        self._sound_beats  = 0
        self._last_short_beep_sec = -1
        self.btn_start.setText("▶")
        self._update()
    def _toggle_mode(self) -> None:
        """Switch stopwatch ↔ countdown."""
        self.reset_timer()
        if self.config.timer_mode == "stopwatch":
            self.config.timer_mode = "countdown"
        else:
            self.config.timer_mode = "stopwatch"
        self._refresh_mode_label()
        self.config.save()
    def _refresh_mode_label(self) -> None:
        if self.config.timer_mode == "stopwatch":
            self.lbl_mode.setText("STOPWATCH")
            self.btn_mode.setText("SW")
        else:
            dur = _fmt(self.config.countdown_duration)
            self.lbl_mode.setText(f"COUNTDOWN  {dur}")
            self.btn_mode.setText("CD")
    # ══ Painting ═════════════════════════════════════════════════════════════
    def paintEvent(self, _event) -> None:  # noqa: N802
        """Draw dark rounded background with subtle border."""
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        path = QPainterPath()
        path.addRoundedRect(0, 0, self.width(), self.height(), 13, 13)
        # Use dynamic bg color
        bg = QColor(self.config.color_bg)
        bg.setAlpha(185)
        p.fillPath(path, bg)
        pen = QPen(_BORDER)
        pen.setWidthF(1.0)
        p.setPen(pen)
        p.drawPath(path)
    # ══ Mouse events ═════════════════════════════════════════════════════════
    def eventFilter(self, obj, event):
        if obj == self.lbl_timer:
            if event.type() == QEvent.Type.MouseButtonPress:
                if event.button() == Qt.MouseButton.LeftButton:
                    self.toggle_timer()
                return True
            elif event.type() == QEvent.Type.MouseButtonDblClick:
                if event.button() == Qt.MouseButton.LeftButton:
                    self.reset_timer()
                return True
            elif event.type() == QEvent.Type.Wheel:
                self._toggle_mode()
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
    def create_context_menu(self) -> QMenu:
        """Build and return the context menu (used by both overlay and tray)."""
        menu = QMenu(self)
        menu.setStyleSheet(_MENU_STYLE)
        act_settings = menu.addAction("⚙  Settings…")
        act_settings.triggered.connect(self._open_settings)
        menu.addSeparator()
        ct_label = "🖱  Click-Through: ON" if self.config.click_through else "🖱  Click-Through: OFF"
        act_ct = menu.addAction(ct_label)
        act_ct.triggered.connect(self._toggle_click_through)
        # Opacity sub-menu
        op_menu = menu.addMenu("💧  Opacity")
        for pct in (30, 50, 70, 85, 95, 100):
            a = op_menu.addAction(f"{pct}%")
            a.triggered.connect(lambda checked, v=pct/100: self._set_opacity(v))
        # Position sub-menu
        pos_menu = menu.addMenu("📍  Position")
        for preset in (
            "top-left", "top-right",
            "bottom-left", "bottom-right",
            "top-center", "bottom-center",
        ):
            a = pos_menu.addAction(preset.replace("-", " ").title())
            a.triggered.connect(lambda checked, p=preset: self._set_preset_position(p))
        menu.addSeparator()
        act_toggle = menu.addAction("👁  Show/Hide Overlay")
        act_toggle.triggered.connect(self.toggle_visibility)
        act_quit = menu.addAction("✕  Quit")
        act_quit.triggered.connect(self._quit_app)
        return menu
    def contextMenuEvent(self, event) -> None:  # noqa: N802
        """Right-click context menu."""
        menu = self.create_context_menu()
        menu.exec(event.globalPos())
    def _set_opacity(self, value):
        self.config.opacity = value
        self.setWindowOpacity(value)
        self.config.save()
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
    def _open_settings(self) -> None:
        # Import here to avoid circular deps / speed up startup
        from timehud.settings_dialog import SettingsDialog
        dlg = SettingsDialog(self.config, parent=None)

        def update_ui():
            cfg = self.config
            if not cfg.show_timer:
                self.reset_timer()

            fs = cfg.font_size
            ff = cfg.font_family or "Monospace"
            def make_font(size: int, bold: bool = True) -> QFont:
                f = QFont(ff, -1)
                f.setPixelSize(size)
                f.setBold(bold)
                return f

            self.lbl_clock.setFont(make_font(fs))
            self.lbl_clock.setStyleSheet(f"color:{cfg.color_clock}; background:transparent;")

            self.lbl_timer.setFont(make_font(int(fs * 1.25)))
            self.lbl_mode.setFont(make_font(max(10, fs // 3), bold=False))

            self.lbl_clock.setVisible(cfg.show_clock)
            self.sep.setVisible(cfg.show_timer)
            self.lbl_timer.setVisible(cfg.show_timer)
            self.lbl_mode.setVisible(cfg.show_timer)
            self.ctrl_widget.setVisible(cfg.show_timer and cfg.show_controls)

            for btn in (self.btn_start, self.btn_reset, self.btn_mode):
                btn_h = max(24, fs - 4)
                btn.setFixedHeight(btn_h)
            self.btn_start.setFixedWidth(max(24, fs - 4) + 6)
            self.btn_reset.setFixedWidth(max(24, fs - 4) + 6)
            self.btn_mode.setFixedWidth(max(24, fs - 4) + 14)

            self.setWindowOpacity(cfg.opacity)
            self.adjustSize()
            if cfg.custom_x < 0:
                self._position_window()
            self._refresh_mode_label()
            self.update()

        dlg.config_changed.connect(update_ui)

        if dlg.exec():
            update_ui()
            self.config.save()

    def toggle_visibility(self) -> None:
        """Show or hide the overlay (used by global hotkeys)."""
        if self.isVisible():
            self.hide()
        else:
            self.show()
    # ══ Fade-in animation ═════════════════════════════════════════════════════
    def _fade_step(self) -> None:
        self._fade_value = min(self._fade_value + 0.06, self.config.opacity)
        self.setWindowOpacity(self._fade_value)
        if self._fade_value >= self.config.opacity:
            self._fade_timer.stop()
# ── Helpers ────────────────────────────────────────────────────────────────
def _fmt(secs: float) -> str:
    """Format seconds → HH:MM:SS (or MM:SS when < 1 h)."""
    s = max(0, int(secs))
    h, rem = divmod(s, 3600)
    m, sec = divmod(rem, 60)
    if h:
        return f"{h:02d}:{m:02d}:{sec:02d}"
    return f"{m:02d}:{sec:02d}"
