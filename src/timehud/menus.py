"""
menus.py – Context/tray menu construction for TimeHUD.

The overlay owns the actions' behavior; this module only builds the menu
structure. `populate_context_menu` fills a caller-provided QMenu so the tray
can rebuild its persistent menu on aboutToShow.
"""

from PyQt6.QtGui import QAction, QActionGroup
from PyQt6.QtWidgets import QMenu

from timehud.config import interval_preset_rounds, valid_presets
from timehud.themes import THEMES
from timehud.timer_engine import fmt_seconds

MENU_STYLE = """
QMenu {
    background: #1A1A1A; color: #CCCCCC;
    border: 1px solid #333; border-radius: 6px;
    padding: 4px 0;
}
QMenu::item          { padding: 6px 22px; }
QMenu::item:selected { background: #2E2E2E; color: #FFF; }
QMenu::separator     { height: 1px; background: #333; margin: 3px 6px; }
"""


def preset_menu_label(p: dict) -> str:
    if p.get("type") == "interval":
        return f'{p["name"]} {p["work"]}/{p["rest"]} ×{interval_preset_rounds(p)}'
    if p.get("type") == "stopwatch":
        return f'{p["name"]} {p["work"]}/{p["rest"]} ↑'
    return f'{p["name"]} {fmt_seconds(p["duration"])}'


def populate_context_menu(
    overlay, menu: QMenu, include_window_actions: bool | None = None
) -> None:
    """Fill `menu` with the full TimeHUD context menu, wired to `overlay`."""
    config = overlay.config
    if include_window_actions is None:
        include_window_actions = config.show_tray_icon
    menu.setStyleSheet(MENU_STYLE)

    act_settings = menu.addAction("Settings…")
    # Lambda drops QAction.triggered's `checked` bool, which would
    # otherwise land in the optional `tab` parameter.
    act_settings.triggered.connect(lambda: overlay._open_settings())
    menu.addSeparator()

    # Presets sub-menu
    preset_menu = menu.addMenu("Presets")
    act_sw = preset_menu.addAction("Stopwatch")
    act_sw.setCheckable(True)
    act_sw.setChecked(config.timer_mode == "stopwatch")
    act_sw.triggered.connect(overlay._apply_stopwatch)
    presets = valid_presets(config.presets)
    if presets:
        preset_menu.addSeparator()
        for p in presets:
            a = preset_menu.addAction(preset_menu_label(p))
            a.setCheckable(True)
            preset_mode = p.get("type", "countdown")
            if preset_mode not in ("interval", "stopwatch"):
                preset_mode = "countdown"
            a.setChecked(
                config.timer_mode == preset_mode
                and config.active_preset == p["name"]
            )
            a.triggered.connect(lambda checked, p=p: overlay._apply_preset(p))
    preset_menu.addSeparator()
    act_save = preset_menu.addAction("Save current as preset…")
    # Plain stopwatch (no cycle) has nothing to capture in a preset
    act_save.setEnabled(
        config.timer_mode != "stopwatch" or config.stopwatch_work > 0
    )
    act_save.triggered.connect(overlay._save_current_preset)
    act_manage = preset_menu.addAction("Manage presets…")
    act_manage.triggered.connect(lambda: overlay._open_settings(tab="presets"))

    # Theme sub-menu
    theme_menu = menu.addMenu("Theme")
    theme_group = QActionGroup(theme_menu)
    theme_group.setExclusive(True)
    for t in THEMES.values():
        a: QAction = theme_menu.addAction(t.label)
        a.setCheckable(True)
        a.setChecked(t.name == config.theme)
        theme_group.addAction(a)
        a.triggered.connect(lambda checked, n=t.name: overlay._set_theme(n))
    menu.addSeparator()

    if include_window_actions:
        ct_label = "Click-Through: ON" if config.click_through else "Click-Through: OFF"
        act_ct = menu.addAction(ct_label)
        act_ct.triggered.connect(overlay._toggle_click_through)

    # Opacity sub-menu
    op_menu = menu.addMenu("Opacity")
    op_group = QActionGroup(op_menu)
    op_group.setExclusive(True)
    current_pct = max(0, min(100, round(config.opacity * 100)))
    matched_opacity = False
    for pct in (30, 50, 70, 85, 95, 100):
        a = op_menu.addAction(f"{pct}%")
        a.setCheckable(True)
        op_group.addAction(a)
        if current_pct == pct:
            a.setChecked(True)
            matched_opacity = True
        a.triggered.connect(lambda checked, v=pct / 100: overlay._set_opacity(v))
    if not matched_opacity:
        current_action = op_menu.addAction(f"Current: {current_pct}%")
        current_action.setCheckable(True)
        current_action.setChecked(True)
        op_group.addAction(current_action)
        current_action.triggered.connect(
            lambda checked, v=current_pct / 100: overlay._set_opacity(v)
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
        a = pos_menu.addAction(preset.replace("-", " ").title())
        a.setCheckable(True)
        a.setChecked(preset == config.position)
        pos_group.addAction(a)
        a.triggered.connect(lambda checked, p=preset: overlay._set_preset_position(p))

    if include_window_actions:
        menu.addSeparator()
        act_toggle = menu.addAction("Show/Hide Overlay")
        act_toggle.triggered.connect(overlay.toggle_visibility)
    act_quit = menu.addAction("Quit")
    act_quit.triggered.connect(overlay._quit_app)
