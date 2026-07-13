import json

import pytest

import timehud.config as config_mod
from timehud.config import Config, valid_presets


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
