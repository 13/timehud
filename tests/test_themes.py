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
        assert t.color_rest == "#4FA8FF"
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


class TestColorRest:
    def test_all_themes_have_color_rest(self):
        expected = {
            "classic": "#4FA8FF",
            "terminal": "#60A5FA",
            "glass": "#93C5FD",
            "mono": "#B0BEC5",
        }
        for name, color in expected.items():
            assert THEMES[name].color_rest == color
