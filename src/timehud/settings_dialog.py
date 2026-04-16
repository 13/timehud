"""
settings_dialog.py – Modal settings window for TimeHUD.
Opens from the right-click context menu on the overlay.
"""

from PyQt6.QtWidgets import (
    QDialog, QVBoxLayout, QHBoxLayout, QFormLayout,
    QLabel, QComboBox, QSpinBox, QCheckBox,
    QLineEdit, QPushButton, QFileDialog,
    QTabWidget, QWidget, QSlider, QDialogButtonBox, QColorDialog
)
from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtGui import QColor, QPixmap
import os

from timehud.config import Config


_DARK_STYLE = """
QDialog, QWidget, QTabWidget::pane {
    background: #1C1C1E;
    color: #E5E5EA;
}
QTabBar::tab {
    background: #2C2C2E;
    color: #8E8E93;
    padding: 6px 18px;
    border: none;
}
QTabBar::tab:selected {
    background: #3A3A3C;
    color: #FFFFFF;
    border-bottom: 2px solid #00FF88;
}
QLabel { color: #E5E5EA; }
QComboBox, QSpinBox, QLineEdit {
    background: #2C2C2E;
    color: #E5E5EA;
    border: 1px solid #3A3A3C;
    border-radius: 4px;
    padding: 4px 8px;
    min-height: 22px;
}
QComboBox::drop-down  { border: none; }
QComboBox QAbstractItemView { background: #2C2C2E; color: #E5E5EA; selection-background-color: #3A3A3C; }
QCheckBox { color: #E5E5EA; spacing: 8px; }
QCheckBox::indicator { width: 16px; height: 16px; border: 1px solid #555; border-radius: 3px; background: #2C2C2E; }
QCheckBox::indicator:checked { background: #00FF88; border-color: #00FF88; }
QSlider::groove:horizontal { height: 4px; background: #3A3A3C; border-radius: 2px; }
QSlider::handle:horizontal { background: #00FF88; width: 14px; height: 14px; margin: -5px 0; border-radius: 7px; }
QSlider::sub-page:horizontal { background: #00CC66; border-radius: 2px; }
QPushButton {
    background: #3A3A3C;
    color: #E5E5EA;
    border: none;
    border-radius: 5px;
    padding: 6px 18px;
    min-height: 26px;
}
QPushButton:hover  { background: #48484A; }
QPushButton:pressed { background: #555; }
QPushButton#ok_btn { background: #00AA55; color: #FFFFFF; }
QPushButton#ok_btn:hover { background: #00CC66; }
"""


class SettingsDialog(QDialog):
    config_changed = pyqtSignal()

    def __init__(self, config: Config, parent=None) -> None:
        super().__init__(parent)
        self.config = config
        self.setWindowTitle("TimeHUD – Settings")
        self.setModal(True)
        self.setMinimumWidth(400)
        self.setStyleSheet(_DARK_STYLE)
        self._build_ui()
        self._load_values()
        self._connect_live_updates()

    # ── UI construction ────────────────────────────────────────────────────

    def _build_ui(self) -> None:
        root = QVBoxLayout(self)
        root.setSpacing(12)

        tabs = QTabWidget()
        tabs.addTab(self._display_tab(),  "🖥  Display")
        tabs.addTab(self._timer_tab(),    "⏱  Timer")
        tabs.addTab(self._sound_tab(),    "🔔  Sound")
        tabs.addTab(self._about_tab(),    "ℹ  About")
        root.addWidget(tabs)

        # ── Dialog buttons ─────────────────────────────────────────────────
        btn_box = QHBoxLayout()
        cancel = QPushButton("Cancel")
        cancel.clicked.connect(self.reject)

        apply = QPushButton("Apply")
        apply.setObjectName("ok_btn")
        apply.clicked.connect(self._apply)

        btn_box.addStretch()
        btn_box.addWidget(cancel)
        btn_box.addWidget(apply)
        root.addLayout(btn_box)

    def _display_tab(self) -> QWidget:
        tab = QWidget()
        form = QFormLayout(tab)
        form.setSpacing(10)
        form.setContentsMargins(16, 16, 16, 16)

        # Position preset
        self.position_combo = QComboBox()
        self.position_combo.addItems([
            "top-left", "top-right",
            "bottom-left", "bottom-right",
            "top-center", "bottom-center",
        ])
        form.addRow("Position:", self.position_combo)

        # Font size
        self.font_size_spin = QSpinBox()
        self.font_size_spin.setRange(12, 80)
        self.font_size_spin.setSuffix(" px")
        form.addRow("Font size:", self.font_size_spin)

        # Font family
        self.font_family_edit = QLineEdit()
        self.font_family_edit.setPlaceholderText("e.g. Monospace, JetBrains Mono")
        form.addRow("Font family:", self.font_family_edit)

        # Opacity
        opacity_row = QHBoxLayout()
        self.opacity_slider = QSlider(Qt.Orientation.Horizontal)
        self.opacity_slider.setRange(20, 100)
        self.opacity_pct_label = QLabel("85%")
        self.opacity_pct_label.setMinimumWidth(38)
        self.opacity_slider.valueChanged.connect(
            lambda v: self.opacity_pct_label.setText(f"{v}%")
        )
        opacity_row.addWidget(self.opacity_slider)
        opacity_row.addWidget(self.opacity_pct_label)
        form.addRow("Opacity:", opacity_row)

        # Colors
        self.btn_color_bg = QPushButton("Background")
        self.btn_color_clock = QPushButton("Clock")
        self.btn_color_timer_run = QPushButton("Timer Running")
        self.btn_color_timer_pause = QPushButton("Timer Paused")
        colors_row1 = QHBoxLayout()
        colors_row1.addWidget(self.btn_color_bg)
        colors_row1.addWidget(self.btn_color_clock)
        colors_row2 = QHBoxLayout()
        colors_row2.addWidget(self.btn_color_timer_run)
        colors_row2.addWidget(self.btn_color_timer_pause)
        form.addRow("Colors:", colors_row1)
        form.addRow("", colors_row2)

        self.btn_color_bg.clicked.connect(lambda: self._pick_color("color_bg", self.btn_color_bg))
        self.btn_color_clock.clicked.connect(lambda: self._pick_color("color_clock", self.btn_color_clock))
        self.btn_color_timer_run.clicked.connect(lambda: self._pick_color("color_timer_run", self.btn_color_timer_run))
        self.btn_color_timer_pause.clicked.connect(lambda: self._pick_color("color_timer_pause", self.btn_color_timer_pause))

        # Checkboxes
        self.show_tray_icon_cb = QCheckBox("Show system tray icon")
        self.show_clock_cb = QCheckBox("Show system clock  (HH:MM:SS)")
        self.show_timer_cb = QCheckBox("Show timer  (stopwatch / countdown)")
        self.show_controls_cb = QCheckBox("Show timer controls (start/stop/reset)")

        form.addRow(self.show_tray_icon_cb)
        form.addRow(self.show_clock_cb)
        form.addRow(self.show_timer_cb)
        form.addRow(self.show_controls_cb)

        return tab

    def _timer_tab(self) -> QWidget:
        tab = QWidget()
        form = QFormLayout(tab)
        form.setSpacing(10)
        form.setContentsMargins(16, 16, 16, 16)

        self.mode_combo = QComboBox()
        self.mode_combo.addItems(["stopwatch", "countdown"])
        form.addRow("Default mode:", self.mode_combo)

        self.countdown_spin = QSpinBox()
        self.countdown_spin.setRange(5, 86400)
        self.countdown_spin.setSuffix(" s")
        self.countdown_spin.setToolTip("Countdown duration in seconds (e.g. 300 = 5 min)")
        form.addRow("Countdown duration:", self.countdown_spin)

        # Helper to show minutes
        self._cd_note = QLabel()
        self._cd_note.setStyleSheet("color:#8E8E93; font-size:11px;")
        self.countdown_spin.valueChanged.connect(self._update_cd_note)
        form.addRow("", self._cd_note)

        self.auto_restart_countdown_cb = QCheckBox("Restart countdown automatically")
        form.addRow(self.auto_restart_countdown_cb)

        return tab

    def _sound_tab(self) -> QWidget:
        tab = QWidget()
        form = QFormLayout(tab)
        form.setSpacing(10)
        form.setContentsMargins(16, 16, 16, 16)

        self.sound_enabled_cb = QCheckBox("Enable periodic sound alerts")
        form.addRow(self.sound_enabled_cb)

        self.alert_last_5_seconds_cb = QCheckBox("Alert last 5 seconds with short beeps (countdown)")
        form.addRow(self.alert_last_5_seconds_cb)

        self.sound_interval_spin = QSpinBox()
        self.sound_interval_spin.setRange(5, 3600)
        self.sound_interval_spin.setSuffix(" s")
        self.sound_interval_spin.setToolTip("Play a beep every N seconds of active timer")
        form.addRow("Alert every:", self.sound_interval_spin)

        file_row = QHBoxLayout()
        self.sound_file_edit = QLineEdit()
        self.sound_file_edit.setPlaceholderText("Leave empty to use built-in beep")
        browse = QPushButton("…")
        browse.setFixedWidth(32)
        browse.clicked.connect(self._browse_sound)
        file_row.addWidget(self.sound_file_edit)
        file_row.addWidget(browse)
        form.addRow("Sound file:", file_row)

        note = QLabel("Supported players: paplay, aplay, ffplay, mpv")
        note.setStyleSheet("color:#8E8E93; font-size:11px;")
        form.addRow("", note)

        return tab

    def _about_tab(self) -> QWidget:
        from timehud import __version__
        tab = QWidget()
        layout = QVBoxLayout(tab)
        layout.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.setSpacing(10)

        logo_lbl = QLabel()

        # Searching for the logo in standard places
        base_dir = os.path.dirname(__file__)
        possible_paths = [
            os.path.join(base_dir, "assets", "timehud.svg"),
            os.path.join(base_dir, "timehud.svg"),
            os.path.join(base_dir, "..", "..", "timehud.svg"),
        ]

        logo_found = False
        for path in possible_paths:
            if os.path.exists(path):
                pm = QPixmap(path)
                if not pm.isNull():
                    pm = pm.scaled(80, 80, Qt.AspectRatioMode.KeepAspectRatio, Qt.TransformationMode.SmoothTransformation)
                    logo_lbl.setPixmap(pm)
                    logo_found = True
                    break

        if not logo_found:
            logo_lbl.setText("⏱")
            logo_lbl.setStyleSheet("font-size: 64px;")

        logo_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(logo_lbl)

        title_lbl = QLabel(f"<b>TimeHUD <span style='color:#00FF88;'>v{__version__}</span></b>")
        title_lbl.setStyleSheet("font-size: 18px;")
        title_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        layout.addWidget(title_lbl)

        desc_lbl = QLabel(
            "A lightweight, always-on-top system overlay.<br>"
            "Clock, Stopwatch & Countdown.<br><br>"
            "<a href='https://github.com/ben/timehud' style='color:#00FF88; text-decoration:none;'>"
            "https://github.com/ben/timehud</a>"
        )
        desc_lbl.setOpenExternalLinks(True)
        desc_lbl.setAlignment(Qt.AlignmentFlag.AlignCenter)
        desc_lbl.setStyleSheet("color: #8E8E93;")
        layout.addWidget(desc_lbl)

        layout.addStretch()
        return tab

    # ── Load / apply ───────────────────────────────────────────────────────

    def _update_color_btn(self, btn: QPushButton, hex_color: str):
        btn.setStyleSheet(f"background-color: {hex_color}; color: {'#000' if hex_color.upper() == '#FFFFFF' else '#FFF'} ; border: 1px solid #555;")

    def _pick_color(self, config_key: str, btn: QPushButton):
        initial = getattr(self.config, config_key)
        initial_color = QColor(initial) if initial else Qt.GlobalColor.white
        color = QColorDialog.getColor(initial_color, self, "Select Color")
        if color.isValid():
            setattr(self.config, config_key, color.name())
            self._update_color_btn(btn, color.name())
            self.config_changed.emit()

    def _connect_live_updates(self):
        def _emit_if_valid():
            self._apply_to_config()
            self.config_changed.emit()

        def _on_show_timer_toggled(checked):
            if not checked:
                self.show_controls_cb.setChecked(False)
            _emit_if_valid()

        self.position_combo.currentIndexChanged.connect(_emit_if_valid)
        self.font_size_spin.valueChanged.connect(_emit_if_valid)
        self.font_family_edit.textChanged.connect(_emit_if_valid)
        self.opacity_slider.valueChanged.connect(_emit_if_valid)
        self.show_tray_icon_cb.toggled.connect(_emit_if_valid)
        self.show_controls_cb.toggled.connect(_emit_if_valid)
        self.show_clock_cb.toggled.connect(_emit_if_valid)
        self.show_timer_cb.toggled.connect(_on_show_timer_toggled)
        self.mode_combo.currentIndexChanged.connect(_emit_if_valid)
        self.countdown_spin.valueChanged.connect(_emit_if_valid)

    def _apply_to_config(self):
        c = self.config
        c.position    = self.position_combo.currentText()
        c.font_size   = self.font_size_spin.value()
        c.font_family = self.font_family_edit.text() or "Monospace"
        c.opacity     = self.opacity_slider.value() / 100.0
        c.show_tray_icon = self.show_tray_icon_cb.isChecked()
        c.show_controls  = self.show_controls_cb.isChecked()
        c.show_clock  = self.show_clock_cb.isChecked()
        c.show_timer  = self.show_timer_cb.isChecked()
        c.timer_mode          = self.mode_combo.currentText()
        c.countdown_duration  = self.countdown_spin.value()
        c.auto_restart_countdown = self.auto_restart_countdown_cb.isChecked()
        c.sound_enabled  = self.sound_enabled_cb.isChecked()
        c.alert_last_5_seconds = self.alert_last_5_seconds_cb.isChecked()
        c.sound_interval = self.sound_interval_spin.value()
        c.sound_file     = self.sound_file_edit.text().strip()

    def _load_values(self) -> None:
        c = self.config

        idx = self.position_combo.findText(c.position)
        self.position_combo.setCurrentIndex(max(0, idx))

        self.font_size_spin.setValue(c.font_size)
        self.font_family_edit.setText(c.font_family)
        self.opacity_slider.setValue(int(c.opacity * 100))
        self.show_tray_icon_cb.setChecked(c.show_tray_icon)
        self.show_controls_cb.setChecked(c.show_controls)
        self.show_clock_cb.setChecked(c.show_clock)
        self.show_timer_cb.setChecked(c.show_timer)

        idx = self.mode_combo.findText(c.timer_mode)
        self.mode_combo.setCurrentIndex(max(0, idx))
        self.countdown_spin.setValue(c.countdown_duration)
        self._update_cd_note(c.countdown_duration)
        self.auto_restart_countdown_cb.setChecked(c.auto_restart_countdown)

        self.sound_enabled_cb.setChecked(c.sound_enabled)
        self.alert_last_5_seconds_cb.setChecked(c.alert_last_5_seconds)
        self.sound_interval_spin.setValue(c.sound_interval)
        self.sound_file_edit.setText(c.sound_file)

        self._update_color_btn(self.btn_color_bg, c.color_bg)
        self._update_color_btn(self.btn_color_clock, c.color_clock)
        self._update_color_btn(self.btn_color_timer_run, c.color_timer_run)
        self._update_color_btn(self.btn_color_timer_pause, c.color_timer_pause)

    def _apply(self) -> None:
        self._apply_to_config()
        self.accept()

    # ── Helpers ────────────────────────────────────────────────────────────

    def _update_cd_note(self, secs: int) -> None:
        m, s = divmod(secs, 60)
        h, m = divmod(m, 60)
        if h:
            txt = f"{h}h {m:02d}m {s:02d}s"
        elif m:
            txt = f"{m}m {s:02d}s"
        else:
            txt = f"{s}s"
        self._cd_note.setText(f"→ {txt}")

    def _browse_sound(self) -> None:
        path, _ = QFileDialog.getOpenFileName(
            self, "Select alert sound",
            os.path.expanduser("~"),
            "Audio Files (*.wav *.mp3 *.ogg *.flac);;All Files (*)",
        )
        if path:
            self.sound_file_edit.setText(path)
