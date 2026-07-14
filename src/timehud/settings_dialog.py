"""
settings_dialog.py – Modal settings window for TimeHUD.
Opens from the right-click context menu on the overlay.
"""

from PyQt6.QtWidgets import (
    QDialog, QVBoxLayout, QHBoxLayout, QFormLayout,
    QLabel, QComboBox, QSpinBox, QCheckBox,
    QLineEdit, QPushButton, QFileDialog,
    QTabWidget, QWidget, QSlider, QColorDialog,
    QListWidget
)
from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtGui import QColor
import os

from timehud.config import Config, interval_preset_rounds, valid_presets
from timehud.themes import THEMES, apply_theme
from timehud.timer_engine import fmt_seconds


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
        self.setModal(False)   # HUD stays usable while settings are open
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
        self._tabs = tabs
        tabs.addTab(self._display_tab(),  "🖥️  Display")
        tabs.addTab(self._timer_tab(),    "⏱  Timer")
        tabs.addTab(self._presets_tab(),  "📋  Presets")
        tabs.addTab(self._sound_tab(),    "🔔  Sound")
        tabs.addTab(self._about_tab(),    "ℹ️  About")
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

        self.theme_combo = QComboBox()
        for t in THEMES.values():
            self.theme_combo.addItem(t.label, t.name)
        form.addRow("Theme:", self.theme_combo)

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

        self.padding_spin = QSpinBox()
        self.padding_spin.setRange(2, 40)
        self.padding_spin.setSuffix(" px")
        self.padding_spin.setToolTip("Space between the window border and the text")
        form.addRow("Padding:", self.padding_spin)

        self.padding_top_spin = QSpinBox()
        self.padding_top_spin.setRange(-1, 40)
        self.padding_top_spin.setSuffix(" px")
        self.padding_top_spin.setSpecialValueText("same as padding")
        self.padding_top_spin.setToolTip("Top padding override; 'same as padding' follows the value above")
        form.addRow("Top padding:", self.padding_top_spin)

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
        self.mode_combo.addItems(["stopwatch", "countdown", "interval"])
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

        self.progress_style_combo = QComboBox()
        self.progress_style_combo.addItems(["line", "border", "off"])
        self.progress_style_combo.setToolTip(
            "line = bar under the timer, border = progress traced around the window"
        )
        form.addRow("Progress bar:", self.progress_style_combo)

        return tab

    def _presets_tab(self) -> QWidget:
        tab = QWidget()
        layout = QVBoxLayout(tab)

        self.preset_list = QListWidget()
        self.preset_list.currentRowChanged.connect(self._preset_selected)
        layout.addWidget(self.preset_list)

        row = QHBoxLayout()
        self.preset_name_edit = QLineEdit()
        self.preset_name_edit.setPlaceholderText("Name")
        self.preset_type_combo = QComboBox()
        self.preset_type_combo.addItems(["countdown", "interval", "stopwatch"])
        self.preset_type_combo.currentIndexChanged.connect(self._preset_type_changed)
        self.preset_dur_spin = QSpinBox()
        self.preset_dur_spin.setRange(1, 24 * 3600)
        self.preset_dur_spin.setSuffix(" s")
        self.preset_dur_spin.setValue(300)
        row.addWidget(self.preset_name_edit, 1)
        row.addWidget(self.preset_type_combo)
        row.addWidget(self.preset_dur_spin)
        layout.addLayout(row)

        iv_row = QHBoxLayout()
        self.preset_work_spin = QSpinBox()
        self.preset_work_spin.setRange(5, 3600)
        self.preset_work_spin.setSuffix(" s work")
        self.preset_work_spin.setValue(45)
        self.preset_rest_spin = QSpinBox()
        self.preset_rest_spin.setRange(0, 3600)
        self.preset_rest_spin.setSuffix(" s rest")
        self.preset_rest_spin.setValue(15)
        self.preset_total_spin = QSpinBox()
        self.preset_total_spin.setRange(1, 24 * 60)
        self.preset_total_spin.setSuffix(" min total")
        self.preset_total_spin.setValue(10)
        iv_row.addWidget(self.preset_work_spin)
        iv_row.addWidget(self.preset_rest_spin)
        iv_row.addWidget(self.preset_total_spin)
        self._preset_iv_row = QWidget()
        self._preset_iv_row.setLayout(iv_row)
        self._preset_iv_row.hide()
        layout.addWidget(self._preset_iv_row)

        sw_row = QHBoxLayout()
        self.preset_swwork_spin = QSpinBox()
        self.preset_swwork_spin.setRange(5, 3600)
        self.preset_swwork_spin.setSuffix(" s work")
        self.preset_swwork_spin.setValue(45)
        self.preset_swrest_spin = QSpinBox()
        self.preset_swrest_spin.setRange(0, 3600)
        self.preset_swrest_spin.setSuffix(" s rest")
        self.preset_swrest_spin.setValue(15)
        sw_row.addWidget(self.preset_swwork_spin)
        sw_row.addWidget(self.preset_swrest_spin)
        self._preset_sw_row = QWidget()
        self._preset_sw_row.setLayout(sw_row)
        self._preset_sw_row.hide()
        layout.addWidget(self._preset_sw_row)

        snd_row = QHBoxLayout()
        self.preset_last5_cb = QCheckBox("last-5 beeps")
        self.preset_every_spin = QSpinBox()
        self.preset_every_spin.setRange(0, 3600)
        self.preset_every_spin.setSuffix(" s alert every (0 = off)")
        self.preset_before_spin = QSpinBox()
        self.preset_before_spin.setRange(0, 600)
        self.preset_before_spin.setSuffix(" s pre-beep (0 = off)")
        snd_row.addWidget(self.preset_last5_cb)
        snd_row.addWidget(self.preset_every_spin)
        snd_row.addWidget(self.preset_before_spin)
        layout.addLayout(snd_row)

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

        self.sound_alert_before_spin = QSpinBox()
        self.sound_alert_before_spin.setRange(0, 3600)
        self.sound_alert_before_spin.setSuffix(" s (0 to disable)")
        self.sound_alert_before_spin.setToolTip("Play a double short beep N seconds before the main alert")
        form.addRow("Double beep before alert:", self.sound_alert_before_spin)

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
        from PyQt6.QtGui import QIcon
        for path in possible_paths:
            if os.path.exists(path):
                icon = QIcon(path)
                if not icon.isNull():
                    pm = icon.pixmap(80, 80)
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

    def _reload_preset_list(self) -> None:
        self.preset_list.clear()
        for p in valid_presets(self.config.presets):
            if p.get("type") == "interval":
                rounds = interval_preset_rounds(p)
                self.preset_list.addItem(
                    f'{p["name"]}  —  {p["work"]}/{p["rest"]} ×{rounds}'
                )
            elif p.get("type") == "stopwatch":
                self.preset_list.addItem(
                    f'{p["name"]}  —  {p["work"]}/{p["rest"]} ↑'
                )
            else:
                self.preset_list.addItem(f'{p["name"]}  —  {fmt_seconds(p["duration"])}')

    def _preset_type_changed(self) -> None:
        kind = self.preset_type_combo.currentText()
        self.preset_dur_spin.setVisible(kind == "countdown")
        self._preset_iv_row.setVisible(kind == "interval")
        self._preset_sw_row.setVisible(kind == "stopwatch")

    def _preset_selected(self, row: int) -> None:
        presets = valid_presets(self.config.presets)
        if not (0 <= row < len(presets)):
            return
        p = presets[row]
        self.preset_name_edit.setText(p["name"])
        c = self.config
        self.preset_last5_cb.setChecked(p.get("last5", c.alert_last_5_seconds))
        self.preset_every_spin.setValue(p.get("every", c.sound_interval))
        self.preset_before_spin.setValue(p.get("before", c.sound_alert_before))
        if p.get("type") == "interval":
            self.preset_type_combo.setCurrentText("interval")
            self.preset_work_spin.setValue(p["work"])
            self.preset_rest_spin.setValue(p["rest"])
            self.preset_total_spin.setValue(max(1, round(p["total"] / 60)))
        elif p.get("type") == "stopwatch":
            self.preset_type_combo.setCurrentText("stopwatch")
            self.preset_swwork_spin.setValue(p["work"])
            self.preset_swrest_spin.setValue(p["rest"])
        else:
            self.preset_type_combo.setCurrentText("countdown")
            self.preset_dur_spin.setValue(p["duration"])

    def _preset_add(self) -> None:
        name = self.preset_name_edit.text().strip()
        if not name:
            return
        kind = self.preset_type_combo.currentText()
        if kind == "interval":
            new_preset = {
                "name": name,
                "type": "interval",
                "work": self.preset_work_spin.value(),
                "rest": self.preset_rest_spin.value(),
                "total": self.preset_total_spin.value() * 60,
            }
        elif kind == "stopwatch":
            new_preset = {
                "name": name,
                "type": "stopwatch",
                "work": self.preset_swwork_spin.value(),
                "rest": self.preset_swrest_spin.value(),
            }
        else:
            new_preset = {"name": name, "duration": self.preset_dur_spin.value()}
        new_preset["last5"] = self.preset_last5_cb.isChecked()
        new_preset["every"] = self.preset_every_spin.value()
        new_preset["before"] = self.preset_before_spin.value()
        presets = [p for p in valid_presets(self.config.presets) if p["name"] != name]
        presets.append(new_preset)
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
        self.position_combo.currentIndexChanged.connect(_emit_if_valid)
        self.font_size_spin.valueChanged.connect(_emit_if_valid)
        self.padding_spin.valueChanged.connect(_emit_if_valid)
        self.padding_top_spin.valueChanged.connect(_emit_if_valid)
        self.font_family_edit.textChanged.connect(_emit_if_valid)
        self.opacity_slider.valueChanged.connect(_emit_if_valid)
        self.show_tray_icon_cb.toggled.connect(_emit_if_valid)
        self.show_controls_cb.toggled.connect(_emit_if_valid)
        self.show_clock_cb.toggled.connect(_emit_if_valid)
        self.show_timer_cb.toggled.connect(_on_show_timer_toggled)
        self.mode_combo.currentIndexChanged.connect(_emit_if_valid)
        self.countdown_spin.valueChanged.connect(_emit_if_valid)
        self.interval_work_spin.valueChanged.connect(_emit_if_valid)
        self.interval_rest_spin.valueChanged.connect(_emit_if_valid)
        self.interval_rounds_spin.valueChanged.connect(_emit_if_valid)
        self.progress_style_combo.currentIndexChanged.connect(_emit_if_valid)
        self.sound_alert_before_spin.valueChanged.connect(_emit_if_valid)

    def _apply_to_config(self):
        c = self.config
        c.position    = self.position_combo.currentText()
        c.font_size   = self.font_size_spin.value()
        c.padding     = self.padding_spin.value()
        c.padding_top = self.padding_top_spin.value()
        c.font_family = self.font_family_edit.text() or "Monospace"
        c.opacity     = self.opacity_slider.value() / 100.0
        c.show_tray_icon = self.show_tray_icon_cb.isChecked()
        c.show_controls  = self.show_controls_cb.isChecked()
        c.show_clock  = self.show_clock_cb.isChecked()
        c.show_timer  = self.show_timer_cb.isChecked()
        c.timer_mode          = self.mode_combo.currentText()
        if self.countdown_spin.value() != c.countdown_duration:
            c.active_preset = ""   # duration changed manually → preset no longer applies
        if (
            self.interval_work_spin.value(),
            self.interval_rest_spin.value(),
            self.interval_rounds_spin.value(),
        ) != (c.interval_work, c.interval_rest, c.interval_rounds):
            c.active_preset = ""   # interval settings changed manually
        c.countdown_duration  = self.countdown_spin.value()
        c.auto_restart_countdown = self.auto_restart_countdown_cb.isChecked()
        c.interval_work   = self.interval_work_spin.value()
        c.interval_rest   = self.interval_rest_spin.value()
        c.interval_rounds = self.interval_rounds_spin.value()
        c.progress_style  = self.progress_style_combo.currentText()
        c.sound_enabled  = self.sound_enabled_cb.isChecked()
        c.alert_last_5_seconds = self.alert_last_5_seconds_cb.isChecked()
        c.sound_interval = self.sound_interval_spin.value()
        c.sound_alert_before = self.sound_alert_before_spin.value()
        c.sound_file     = self.sound_file_edit.text().strip()

    def _load_values(self) -> None:
        c = self.config

        idx = self.theme_combo.findData(c.theme)
        self.theme_combo.setCurrentIndex(max(0, idx))

        idx = self.position_combo.findText(c.position)
        self.position_combo.setCurrentIndex(max(0, idx))

        self.font_size_spin.setValue(c.font_size)
        self.padding_spin.setValue(c.padding)
        self.padding_top_spin.setValue(c.padding_top)
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
        self.interval_work_spin.setValue(c.interval_work)
        self.interval_rest_spin.setValue(c.interval_rest)
        self.interval_rounds_spin.setValue(c.interval_rounds)
        idx = self.progress_style_combo.findText(c.progress_style)
        self.progress_style_combo.setCurrentIndex(max(0, idx))

        self.sound_enabled_cb.setChecked(c.sound_enabled)
        self.alert_last_5_seconds_cb.setChecked(c.alert_last_5_seconds)
        self.sound_interval_spin.setValue(c.sound_interval)
        self.sound_alert_before_spin.setValue(c.sound_alert_before)
        self.sound_file_edit.setText(c.sound_file)

        self._reload_preset_list()
        self.preset_last5_cb.setChecked(c.alert_last_5_seconds)
        self.preset_every_spin.setValue(c.sound_interval)
        self.preset_before_spin.setValue(c.sound_alert_before)

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
