import json

import pytest

import timehud.config as config_mod
from timehud.config import Config, interval_preset_rounds, valid_presets


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


class TestIntervalPresets:
    def test_valid_interval_preset_accepted(self):
        p = {"name": "45/15 10min", "type": "interval", "work": 45, "rest": 15, "total": 600}
        assert valid_presets([p]) == [p]

    def test_zero_rest_allowed(self):
        p = {"name": "30/0 20min", "type": "interval", "work": 30, "rest": 0, "total": 1200}
        assert valid_presets([p]) == [p]

    def test_malformed_interval_presets_filtered(self):
        raw = [
            {"name": "no total", "type": "interval", "work": 45, "rest": 15},
            {"name": "zero work", "type": "interval", "work": 0, "rest": 15, "total": 600},
            {"name": "neg rest", "type": "interval", "work": 45, "rest": -1, "total": 600},
            {"name": "total lt work", "type": "interval", "work": 45, "rest": 15, "total": 30},
            {"name": "bool work", "type": "interval", "work": True, "rest": 15, "total": 600},
            {"name": "str total", "type": "interval", "work": 45, "rest": 15, "total": "600"},
        ]
        assert valid_presets(raw) == []

    def test_mixed_list_keeps_both_kinds(self):
        cd = {"name": "5 min", "duration": 300}
        iv = {"name": "hiit", "type": "interval", "work": 45, "rest": 15, "total": 600}
        assert valid_presets([cd, iv, {"junk": 1}]) == [cd, iv]

    def test_rounds_from_total(self):
        p = {"name": "x", "type": "interval", "work": 45, "rest": 15, "total": 600}
        assert interval_preset_rounds(p) == 10        # 600 / 60

    def test_rounds_zero_rest(self):
        p = {"name": "x", "type": "interval", "work": 30, "rest": 0, "total": 1200}
        assert interval_preset_rounds(p) == 40        # 1200 / 30

    def test_rounds_at_least_one(self):
        p = {"name": "x", "type": "interval", "work": 45, "rest": 15, "total": 45}
        assert interval_preset_rounds(p) == 1

    def test_rounds_truncates_partial_cycle(self):
        p = {"name": "x", "type": "interval", "work": 45, "rest": 15, "total": 630}
        assert interval_preset_rounds(p) == 10        # 630 // 60


class TestStopwatchPresets:
    def test_valid_stopwatch_preset_accepted(self):
        p = {"name": "45/15 up", "type": "stopwatch", "work": 45, "rest": 15}
        assert valid_presets([p]) == [p]

    def test_zero_rest_allowed(self):
        p = {"name": "laps", "type": "stopwatch", "work": 60, "rest": 0}
        assert valid_presets([p]) == [p]

    def test_malformed_stopwatch_presets_filtered(self):
        raw = [
            {"name": "no work", "type": "stopwatch"},
            {"name": "zero work", "type": "stopwatch", "work": 0, "rest": 15},
            {"name": "neg rest", "type": "stopwatch", "work": 45, "rest": -1},
            {"name": "bool", "type": "stopwatch", "work": True, "rest": 0},
            {"name": "str", "type": "stopwatch", "work": "45", "rest": 0},
            {"name": "legacy shape", "type": "stopwatch", "interval": 60},
        ]
        assert valid_presets(raw) == []

    def test_all_three_kinds_coexist(self):
        cd = {"name": "5 min", "duration": 300}
        iv = {"name": "hiit", "type": "interval", "work": 45, "rest": 15, "total": 600}
        sw = {"name": "45/15 up", "type": "stopwatch", "work": 45, "rest": 15}
        assert valid_presets([cd, iv, sw]) == [cd, iv, sw]


class TestPresetSoundRules:
    def test_sound_fields_accepted_on_all_types(self):
        snd = {"last5": True, "every": 60, "before": 5}
        presets = [
            {"name": "cd", "duration": 300, **snd},
            {"name": "iv", "type": "interval", "work": 45, "rest": 15, "total": 600, **snd},
            {"name": "sw", "type": "stopwatch", "work": 45, "rest": 15, **snd},
        ]
        assert valid_presets(presets) == presets

    def test_sound_fields_optional(self):
        p = {"name": "plain", "duration": 300}
        assert valid_presets([p]) == [p]

    def test_zero_every_allowed(self):
        p = {"name": "quiet", "duration": 300, "every": 0}
        assert valid_presets([p]) == [p]

    def test_boundary_field_accepted(self):
        p = {"name": "sw", "type": "stopwatch", "work": 45, "rest": 15, "boundary": False}
        assert valid_presets([p]) == [p]

    def test_malformed_boundary_filtered(self):
        p = {"name": "sw", "type": "stopwatch", "work": 45, "rest": 15, "boundary": "no"}
        assert valid_presets([p]) == []

    def test_malformed_sound_fields_filtered(self):
        raw = [
            {"name": "bad last5", "duration": 300, "last5": "yes"},
            {"name": "bad every", "duration": 300, "every": -1},
            {"name": "bool every", "duration": 300, "every": True},
            {"name": "bad before", "duration": 300, "before": "5"},
        ]
        assert valid_presets(raw) == []
