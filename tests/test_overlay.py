"""
Overlay regression tests (pytest-qt, offscreen).

These pin the Qt-side behaviors that engine tests cannot see: fold geometry,
margins, label sync, preset application, the non-modal settings dialog, and
the beep/label alignment. The engine's clock is replaced with the FakeClock
fixture so no test sleeps in real time.
"""

import pytest

from timehud.config import Config
from timehud.overlay import OverlayWindow


@pytest.fixture
def overlay(qtbot, clock):
    """OverlayWindow with saving disabled, fake engine clock, no live tick."""

    def make(**config_kwargs) -> OverlayWindow:
        cfg = Config(**config_kwargs)
        cfg.save = lambda: None          # never touch the real config file
        w = OverlayWindow(cfg)
        w._tick.stop()                   # tests drive _update() explicitly
        w.engine._clock = clock
        w.engine.reset()
        qtbot.addWidget(w)
        w.show()
        return w

    return make


class TestGeometry:
    def test_default_margins(self, overlay):
        w = overlay()
        m = w.layout().contentsMargins()
        assert (m.left(), m.top(), m.right(), m.bottom()) == (16, 12, 16, 8)

    def test_padding_top_override(self, overlay):
        w = overlay(padding_top=2)
        assert w.layout().contentsMargins().top() == 2

    def test_fold_shrinks_window_and_unfold_restores(self, qtbot, overlay):
        w = overlay()
        h_full = w.height()
        w._fade_controls_to(0.0)
        qtbot.waitUntil(lambda: w._controls_pos == 0.0, timeout=2000)
        assert w.height() < h_full - 10
        assert w.layout().contentsMargins().bottom() == w.config.padding
        w._fade_controls_to(1.0)
        qtbot.waitUntil(lambda: w._controls_pos == 1.0, timeout=2000)
        assert w.height() == h_full
        assert w.ctrl_widget.maximumHeight() == 16777215
        assert w.layout().contentsMargins().bottom() == 8

    def test_buttons_fit_after_font_growth(self, qtbot, overlay):
        w = overlay()
        w._fade_controls_to(0.0)
        qtbot.waitUntil(lambda: w._controls_pos == 0.0, timeout=2000)
        w._fade_controls_to(1.0)
        qtbot.waitUntil(lambda: w._controls_pos == 1.0, timeout=2000)
        w.config.font_size = 60
        w._apply_styles()
        w._apply_button_sizes()
        w.adjustSize()
        assert w.ctrl_widget.maximumHeight() == 16777215

    def test_pulse_keeps_mode_label_in_place(self, qtbot, overlay):
        w = overlay(timer_mode="countdown")
        w._update()
        mode_y = w.lbl_mode.y()
        w._pulse_timer_label()
        qtbot.wait(80)                    # mid-pulse
        assert w.lbl_mode.y() == mode_y
        qtbot.waitUntil(lambda: w._pulse_anim is None or w._pulse_anim.state() != w._pulse_anim.State.Running, timeout=2000)
        assert w.lbl_mode.y() == mode_y


class TestPresets:
    def test_countdown_preset(self, overlay):
        w = overlay()
        w._apply_preset({"name": "5 min", "duration": 300})
        assert w.config.timer_mode == "countdown"
        assert w.config.countdown_duration == 300
        assert w.config.active_preset == "5 min"
        assert "5 MIN" in w.lbl_mode.text()

    def test_interval_preset_rounds_from_total(self, overlay):
        w = overlay()
        w._apply_preset(
            {"name": "hiit", "type": "interval", "work": 45, "rest": 15, "total": 600}
        )
        assert w.config.timer_mode == "interval"
        assert w.config.interval_rounds == 10

    def test_cycling_stopwatch_preset(self, overlay, clock):
        w = overlay()
        w._apply_preset({"name": "up", "type": "stopwatch", "work": 45, "rest": 15})
        assert w.config.stopwatch_work == 45
        w.toggle_timer()
        clock.advance(50)
        w._update()
        assert "REST 1" in w.lbl_mode.text()

    def test_mode_toggle_clears_preset(self, overlay):
        w = overlay()
        w._apply_preset({"name": "5 min", "duration": 300})
        w._toggle_mode()
        assert w.config.active_preset == ""

    def test_sound_toggles_applied_and_legacy_ignored(self, overlay):
        w = overlay()
        w.config.sound_interval = 90
        w._apply_preset(
            {"name": "hiit", "type": "interval", "work": 45, "rest": 15,
             "total": 600, "last5": True, "boundary": False, "halfway": True}
        )
        assert w.config.alert_last_5_seconds is True
        assert w.config.phase_beeps is False
        assert w.config.halfway_beep is True
        # legacy every/before keys and plain presets leave sound_interval alone
        w._apply_preset({"name": "legacy", "duration": 60, "every": 0, "before": 5})
        assert w.config.sound_interval == 90
        assert w.config.phase_beeps is True      # defaults restored
        assert w.config.halfway_beep is False


class TestBeepLabelAlignment:
    def test_shorts_on_displayed_5_to_1_long_on_zero(self, overlay, clock):
        w = overlay(
            timer_mode="countdown",
            countdown_duration=6,
            sound_enabled=True,
            alert_last_5_seconds=True,
        )
        calls = []
        w.sound.play_alert = lambda short=False, double_beep=False: calls.append(
            ("double" if double_beep else ("short" if short else "long"), w.lbl_timer.text())
        )
        w.toggle_timer()
        for _ in range(65):               # 6.5 s in 100 ms ticks
            clock.advance(0.1)
            w._update()
        shorts = [label for kind, label in calls if kind == "short"]
        longs = [label for kind, label in calls if kind == "long"]
        assert shorts == ["00:05", "00:04", "00:03", "00:02", "00:01"]
        assert longs == ["00:00"]

    def test_label_starts_at_full_duration(self, overlay, clock):
        w = overlay(timer_mode="countdown", countdown_duration=30)
        w.toggle_timer()
        clock.advance(0.1)
        w._update()
        assert w.lbl_timer.text() == "00:30"


class TestProgressStyles:
    def test_line_border_off(self, overlay):
        w = overlay(timer_mode="countdown")
        w.config.progress_style = "line"
        w._update()
        assert w.progress_bar._fraction == 1.0
        assert w._border_fraction == -1.0
        w.config.progress_style = "border"
        w._update()
        assert w.progress_bar._fraction == -1.0
        assert w._border_fraction == 1.0
        w.config.progress_style = "off"
        w._update()
        assert w._border_fraction == -1.0

    def test_stopwatch_shows_no_bar(self, overlay):
        w = overlay()
        w._update()
        assert w.progress_bar._fraction == -1.0


class TestSettingsDialog:
    def test_non_modal_single_instance(self, qtbot, overlay):
        w = overlay()
        w._open_settings()
        dlg = w._settings_dlg
        assert dlg is not None and not dlg.isModal()
        w.toggle_timer()                  # HUD stays usable
        assert w.engine.running
        w.toggle_timer()
        w._open_settings()
        assert w._settings_dlg is dlg     # reused, not stacked
        dlg.reject()
        qtbot.waitUntil(lambda: w._settings_dlg is None, timeout=2000)

    def test_cancel_reverts_live_changes(self, qtbot, overlay):
        w = overlay()
        orig_size = w.config.font_size
        orig_presets = [dict(p) for p in w.config.presets]
        w._open_settings()
        dlg = w._settings_dlg
        dlg.font_size_spin.setValue(orig_size + 10)   # live-applies
        dlg.preset_name_edit.setText("Temp")
        dlg._preset_add()
        dlg.reject()
        qtbot.waitUntil(lambda: w._settings_dlg is None, timeout=2000)
        assert w.config.font_size == orig_size
        assert w.config.presets == orig_presets

    def test_countdown_duration_sync_keeps_idle(self, qtbot, overlay):
        w = overlay(timer_mode="countdown")
        assert w.engine.is_idle()
        w._open_settings()
        dlg = w._settings_dlg
        dlg.countdown_spin.setValue(w.config.countdown_duration + 60)
        assert w.engine.is_idle(), "duration edit must not fake-start the timer"
        dlg.reject()
        qtbot.waitUntil(lambda: w._settings_dlg is None, timeout=2000)

    def test_zero_sound_interval_survives_settings(self, qtbot, overlay):
        w = overlay()
        w.config.sound_interval = 0            # periodic beeps off (Sound tab)
        w._open_settings()
        dlg = w._settings_dlg
        assert dlg.sound_interval_spin.value() == 0
        dlg.font_size_spin.setValue(w.config.font_size + 2)
        assert w.config.sound_interval == 0
        dlg.accept()
        qtbot.waitUntil(lambda: w._settings_dlg is None, timeout=2000)
        assert w.config.sound_interval == 0
