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
from PyQt6.QtCore import (
    Qt, QPoint, QTimer, QEvent, QPropertyAnimation, pyqtProperty
)
from PyQt6.QtGui import (
    QAction, QActionGroup, QColor, QFont, QPainter, QPainterPath, QPen,
    QCursor, QGuiApplication,
)
from PyQt6.QtWidgets import (
    QApplication, QHBoxLayout, QLabel, QMenu,
    QPushButton, QVBoxLayout, QWidget,
)
from timehud.config import Config, valid_presets
from timehud.sound_manager import SoundManager
from timehud.timer_engine import TimerEngine
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
        self.lbl_mode.setCursor(Qt.CursorShape.PointingHandCursor)
        self.lbl_mode.installEventFilter(self)

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
    # ══ Timer logic ══════════════════════════════════════════════════════════
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
                            self.config.active_preset = ""
                            self.engine.adjust_countdown(delta)
                        self.config.save()
                        self._refresh_mode_label()
                        self._update()
                    else:
                        modes = ["stopwatch", "countdown"]
                        curr = modes.index(self.config.timer_mode) if self.config.timer_mode in modes else 0
                        new_mode = modes[(curr + (-1 if delta_wheel > 0 else 1)) % len(modes)]
                        if self.config.timer_mode != new_mode:
                            self.engine.set_mode(new_mode)
                            self.btn_start.setText("▶")
                            self._refresh_mode_label()
                            self.config.save()
                elif obj == self.lbl_timer:
                    delta = event.angleDelta().y()
                    modes = ["stopwatch", "countdown"]
                    curr = modes.index(self.config.timer_mode) if self.config.timer_mode in modes else 0
                    new_mode = modes[(curr + (-1 if delta > 0 else 1)) % len(modes)]
                    if self.config.timer_mode != new_mode:
                        self.engine.set_mode(new_mode)
                        self.btn_start.setText("▶")
                        self._refresh_mode_label()
                        self.config.save()
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
        if include_window_actions is None:
            include_window_actions = self.config.show_tray_icon

        menu = QMenu(self)
        menu.setStyleSheet(_MENU_STYLE)
        act_settings = menu.addAction("⚙  Settings…")
        act_settings.triggered.connect(self._open_settings)
        menu.addSeparator()

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
                a = preset_menu.addAction(f'{p["name"]} {_fmt(p["duration"])}')
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

        if include_window_actions:
            ct_label = "🖱  Click-Through: ON" if self.config.click_through else "🖱  Click-Through: OFF"
            act_ct = menu.addAction(ct_label)
            act_ct.triggered.connect(self._toggle_click_through)

        # Opacity sub-menu
        op_menu = menu.addMenu("💧  Opacity")
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
        pos_menu = menu.addMenu("📍  Position")
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
    def _open_settings(self, tab: str | None = None) -> None:
        # Import here to avoid circular deps / speed up startup
        from timehud.settings_dialog import SettingsDialog
        dlg = SettingsDialog(self.config, parent=None)
        if tab is not None and hasattr(dlg, "select_tab"):
            dlg.select_tab(tab)

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

            if cfg.show_tray_icon != self._last_show_tray_icon:
                self._last_show_tray_icon = cfg.show_tray_icon
                if self._on_tray_icon_toggle is not None:
                    self._on_tray_icon_toggle(cfg.show_tray_icon)

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
