use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Stopwatch,
    Countdown,
    Interval,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub position: String,
    pub custom_x: i64,
    pub custom_y: i64,
    pub margin: i64,
    pub opacity: f64,
    pub font_size: i64,
    pub font_family: String,
    pub color_bg: String,
    pub color_clock: String,
    pub color_timer_run: String,
    pub color_timer_pause: String,
    pub show_clock: bool,
    pub show_timer: bool,
    pub timer_mode: Mode,
    pub countdown_duration: i64,
    pub interval_work: i64,
    pub interval_rest: i64,
    pub interval_rounds: i64,
    pub stopwatch_work: i64,
    pub stopwatch_rest: i64,
    pub phase_beeps: bool,
    pub halfway_beep: bool,
    pub sound_enabled: bool,
    pub sound_interval: i64,
    pub sound_alert_before: i64,
    pub sound_file: String,
    pub show_tray_icon: bool,
    pub show_controls: bool,
    pub click_through: bool,
    pub auto_restart_countdown: bool,
    pub alert_last_5_seconds: bool,
    pub presets: Vec<serde_json::Value>,
    pub active_preset: String,
    pub theme: String,
    pub progress_style: String,
    pub padding: i64,
    pub padding_top: i64,
    pub row_spacing: i64,
    pub clock_format: String,
    pub hotkey_toggle: String,
    pub hotkey_reset: String,
    pub hotkey_visibility: String,
    pub notify_on_finish: bool,
    /// Unknown keys, preserved on save.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            position: "top-right".into(),
            custom_x: -1,
            custom_y: -1,
            margin: 15,
            opacity: 0.88,
            font_size: 30,
            font_family: "Monospace".into(),
            color_bg: "#000000".into(),
            color_clock: "#00FF88".into(),
            color_timer_run: "#FFFFFF".into(),
            color_timer_pause: "#888888".into(),
            show_clock: true,
            show_timer: true,
            timer_mode: Mode::Stopwatch,
            countdown_duration: 300,
            interval_work: 40,
            interval_rest: 20,
            interval_rounds: 8,
            stopwatch_work: 0,
            stopwatch_rest: 0,
            phase_beeps: true,
            halfway_beep: false,
            sound_enabled: true,
            sound_interval: 60,
            sound_alert_before: 0,
            sound_file: String::new(),
            show_tray_icon: true,
            show_controls: true,
            click_through: false,
            auto_restart_countdown: false,
            alert_last_5_seconds: false,
            presets: vec![
                serde_json::json!({"name": "1 min", "duration": 60}),
                serde_json::json!({"name": "5 min", "duration": 300}),
            ],
            active_preset: String::new(),
            theme: "classic".into(),
            progress_style: "line".into(),
            padding: 12,
            padding_top: -1,
            row_spacing: 6,
            clock_format: "24h".into(),
            hotkey_toggle: String::new(),
            hotkey_reset: String::new(),
            hotkey_visibility: String::new(),
            notify_on_finish: false,
            extra: serde_json::Map::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/timehud/config.json`, default `~/.config/timehud/config.json`.
/// `path` with `suffix` appended to its file name (`config.json.bad`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

pub fn config_path() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
    };
    base.join("timehud").join("config.json")
}

/// Lists every known field once; generates tolerant `from_value` and `to_value`.
macro_rules! config_codec {
    ($($field:ident),+ $(,)?) => {
        impl Config {
            /// Per-field tolerant: a wrongly typed field falls back to its default alone.
            pub fn from_value(v: &Value) -> Config {
                let mut cfg = Config::default();
                let Some(obj) = v.as_object() else {
                    return cfg;
                };
                for (key, val) in obj {
                    match key.as_str() {
                        $(
                            stringify!($field) => {
                                if let Ok(x) = serde_json::from_value(val.clone()) {
                                    cfg.$field = x;
                                }
                            }
                        )+
                        _ => {
                            cfg.extra.insert(key.clone(), val.clone());
                        }
                    }
                }
                cfg
            }

            /// Unknown keys first, then known fields (known fields win).
            pub fn to_value(&self) -> Value {
                let mut map = self.extra.clone();
                $(
                    map.insert(
                        stringify!($field).to_string(),
                        serde_json::to_value(&self.$field).unwrap_or(Value::Null),
                    );
                )+
                Value::Object(map)
            }
        }
    };
}

config_codec!(
    position,
    custom_x,
    custom_y,
    margin,
    opacity,
    font_size,
    font_family,
    color_bg,
    color_clock,
    color_timer_run,
    color_timer_pause,
    show_clock,
    show_timer,
    timer_mode,
    countdown_duration,
    interval_work,
    interval_rest,
    interval_rounds,
    stopwatch_work,
    stopwatch_rest,
    phase_beeps,
    halfway_beep,
    sound_enabled,
    sound_interval,
    sound_alert_before,
    sound_file,
    show_tray_icon,
    show_controls,
    click_through,
    auto_restart_countdown,
    alert_last_5_seconds,
    presets,
    active_preset,
    theme,
    progress_style,
    padding,
    padding_top,
    row_spacing,
    clock_format,
    hotkey_toggle,
    hotkey_reset,
    hotkey_visibility,
    notify_on_finish,
);

impl Config {
    /// Missing or unparseable file gives defaults. An unparseable file is
    /// first copied to `<path>.bad` (overwriting) so a later save never
    /// silently destroys the user's only copy.
    pub fn load_from(path: &Path) -> Config {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("timehud: cannot read {}: {e}", path.display());
                }
                return Config::default();
            }
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => Config::from_value(&v),
            Err(e) => {
                let bad = with_suffix(path, ".bad");
                match std::fs::write(&bad, &bytes) {
                    Ok(()) => eprintln!(
                        "timehud: {} is not valid JSON ({e}); saved a copy as {} and using defaults",
                        path.display(),
                        bad.display()
                    ),
                    Err(we) => eprintln!(
                        "timehud: {} is not valid JSON ({e}); could not back it up to {}: {we}",
                        path.display(),
                        bad.display()
                    ),
                }
                Config::default()
            }
        }
    }

    /// Atomic: writes and syncs `<path>.tmp`, then renames over `path`,
    /// keeping an existing file's permission bits.
    /// A symlinked `path` is resolved first, so the link's target is
    /// replaced and the link itself kept. Creates the directory.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let path = resolve_save_target(path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = with_suffix(&path, ".tmp");
        let text = serde_json::to_string_pretty(&self.to_value()).map_err(std::io::Error::other)?;
        // Keep the mode of the file being replaced (e.g. a deliberate 0600).
        let mode = std::fs::metadata(&path).ok().map(|m| m.permissions());
        let written = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            if let Some(perms) = mode {
                f.set_permissions(perms)?;
            }
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)
        })();
        written.inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    pub fn load() -> Config {
        Config::load_from(&config_path())
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&config_path())
    }
}

/// Where a save to `path` should land: the canonical target, or for a
/// dangling symlink the link's (possibly not-yet-existing) target, so the
/// link survives the first save. Falls back to `path` itself.
fn resolve_save_target(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    match std::fs::read_link(path) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => path.parent().map_or(target.clone(), |d| d.join(&target)),
        Err(_) => path.to_path_buf(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preset {
    Countdown {
        name: String,
        duration: i64,
    },
    Interval {
        name: String,
        work: i64,
        rest: i64,
        total: i64,
    },
    Stopwatch {
        name: String,
        work: i64,
        rest: i64,
    },
}

/// Optional sound rules applied to the config when a preset is selected.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetRules {
    pub last5: Option<bool>,
    pub boundary: Option<bool>,
    pub halfway: Option<bool>,
}

impl Preset {
    pub fn name(&self) -> &str {
        match self {
            Preset::Countdown { name, .. }
            | Preset::Interval { name, .. }
            | Preset::Stopwatch { name, .. } => name,
        }
    }

    /// Serialized in the Python-compatible shape (legacy keys are not written).
    pub fn to_value(&self, rules: &PresetRules) -> Value {
        let mut v = match self {
            Preset::Countdown { name, duration } => json!({"name": name, "duration": duration}),
            Preset::Interval {
                name,
                work,
                rest,
                total,
            } => {
                json!({"name": name, "type": "interval", "work": work, "rest": rest, "total": total})
            }
            Preset::Stopwatch { name, work, rest } => {
                json!({"name": name, "type": "stopwatch", "work": work, "rest": rest})
            }
        };
        let obj = v.as_object_mut().expect("preset is an object");
        for (key, rule) in [
            ("last5", rules.last5),
            ("boundary", rules.boundary),
            ("halfway", rules.halfway),
        ] {
            if let Some(b) = rule {
                obj.insert(key.into(), Value::Bool(b));
            }
        }
        v
    }
}

/// Integer (not bool, not float) that is at least `minimum`.
fn int_min(v: Option<&Value>, minimum: i64) -> Option<i64> {
    v.and_then(Value::as_i64).filter(|&n| n >= minimum)
}

/// Optional bool rule: `Ok(None)` when absent, `Err` when present but not a bool.
fn rule(obj: &serde_json::Map<String, Value>, key: &str) -> Result<Option<bool>, ()> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(()),
    }
}

fn parse_preset(v: &Value) -> Option<(Preset, PresetRules)> {
    let obj = v.as_object()?;
    let name = obj.get("name")?.as_str()?.to_string();
    let rules = PresetRules {
        last5: rule(obj, "last5").ok()?,
        boundary: rule(obj, "boundary").ok()?,
        halfway: rule(obj, "halfway").ok()?,
    };
    let preset = match obj.get("type").and_then(Value::as_str) {
        Some("stopwatch") => Preset::Stopwatch {
            name,
            work: int_min(obj.get("work"), 1)?,
            rest: int_min(obj.get("rest"), 0)?,
        },
        Some("interval") => {
            let work = int_min(obj.get("work"), 1)?;
            let total = int_min(obj.get("total"), 1)?;
            if total < work {
                return None;
            }
            Preset::Interval {
                name,
                work,
                rest: int_min(obj.get("rest"), 0)?,
                total,
            }
        }
        _ => Preset::Countdown {
            name,
            duration: int_min(obj.get("duration"), 1)?,
        },
    };
    Some((preset, rules))
}

/// Filters malformed entries (hand-edited config); the index points into `raw`.
pub fn valid_presets(raw: &[Value]) -> Vec<(Preset, PresetRules, usize)> {
    raw.iter()
        .enumerate()
        .filter_map(|(i, v)| parse_preset(v).map(|(p, r)| (p, r, i)))
        .collect()
}

/// Rounds that fit into the preset's total time (at least one).
pub fn interval_preset_rounds(work: i64, rest: i64, total: i64) -> i64 {
    (total / (work + rest).max(1)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("timehud").join("config.json")
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn valid(raw: &[Value]) -> Vec<Value> {
        valid_presets(raw)
            .iter()
            .map(|(p, r, _)| p.to_value(r))
            .collect()
    }

    // ---- TestRoundTrip ----
    #[test]
    fn save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = cfg_in(&dir);
        let cfg = Config {
            font_size: 44,
            presets: vec![json!({"name": "Plank", "duration": 60})],
            ..Config::default()
        };
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded.font_size, 44);
        assert_eq!(
            loaded.presets,
            vec![json!({"name": "Plank", "duration": 60})]
        );
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn unknown_keys_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"font_size": 20, "from_the_future": true}"#).unwrap();
        assert_eq!(Config::load_from(&path).font_size, 20);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load_from(&dir.path().join("nope.json"));
        assert_eq!(
            cfg.presets,
            vec![
                json!({"name": "1 min", "duration": 60}),
                json!({"name": "5 min", "duration": 300})
            ]
        );
        assert_eq!(cfg.active_preset, "");
    }

    #[test]
    fn unparseable_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Config::load_from(&path), Config::default());
    }

    #[test]
    fn unparseable_file_backed_up_before_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let bad = dir.path().join("config.json.bad");
        std::fs::write(&bad, "stale").unwrap();
        let original = b"{\"font_size\": 40,, \xff}";
        std::fs::write(&path, original).unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg, Config::default());
        assert_eq!(std::fs::read(&bad).unwrap(), original);
        cfg.save_to(&path).unwrap();
        // The backup survives the save that replaced the broken file.
        assert_eq!(std::fs::read(&bad).unwrap(), original);
    }

    #[test]
    fn valid_or_missing_file_makes_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        Config::load_from(&path);
        std::fs::write(&path, r#"{"font_size": 20}"#).unwrap();
        Config::load_from(&path);
        assert!(!dir.path().join("config.json.bad").exists());
    }

    #[test]
    fn symlinked_config_updated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles").join("timehud.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"font_size": 20}"#).unwrap();
        let link = cfg_in(&dir);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let cfg = Config {
            font_size: 44,
            ..Config::load_from(&link)
        };
        cfg.save_to(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        assert_eq!(Config::load_from(&target).font_size, 44);
        // No temp file left next to the link or the target.
        assert!(!link.with_extension("json.tmp").exists());
        assert!(!target.with_extension("json.tmp").exists());
    }

    #[test]
    fn dangling_symlinked_config_keeps_link_on_first_save() {
        let dir = tempfile::tempdir().unwrap();
        let link = cfg_in(&dir);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        // Relative target that does not exist yet (fresh dotfiles checkout).
        std::os::unix::fs::symlink("dotfiles/timehud.json", &link).unwrap();
        let target = link.parent().unwrap().join("dotfiles/timehud.json");
        Config::default().save_to(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert!(target.is_file());
    }

    // ---- TestValidPresets ----
    #[test]
    fn filters_malformed_entries() {
        let raw = vec![
            json!({"name": "ok", "duration": 90}),
            json!({"name": "no duration"}),
            json!({"duration": 30}),
            json!({"name": 5, "duration": 30}),
            json!({"name": "bad duration", "duration": "30"}),
            json!("not a dict"),
            json!({"name": "zero", "duration": 0}),
        ];
        assert_eq!(valid(&raw), vec![json!({"name": "ok", "duration": 90})]);
    }

    #[test]
    fn bool_duration_rejected() {
        assert!(valid_presets(&[json!({"name": "x", "duration": true})]).is_empty());
    }

    #[test]
    fn float_duration_rejected() {
        assert!(valid_presets(&[json!({"name": "x", "duration": 1.5})]).is_empty());
    }

    #[test]
    fn index_points_into_raw() {
        let raw = vec![json!("junk"), json!({"name": "a", "duration": 5})];
        assert_eq!(valid_presets(&raw)[0].2, 1);
    }

    // ---- TestIntervalPresets ----
    #[test]
    fn valid_interval_preset_accepted() {
        let p = json!({"name": "45/15 10min", "type": "interval", "work": 45, "rest": 15, "total": 600});
        assert_eq!(valid(std::slice::from_ref(&p)), vec![p]);
    }

    #[test]
    fn interval_zero_rest_allowed() {
        let p =
            json!({"name": "30/0 20min", "type": "interval", "work": 30, "rest": 0, "total": 1200});
        assert_eq!(valid(std::slice::from_ref(&p)), vec![p]);
    }

    #[test]
    fn malformed_interval_presets_filtered() {
        let raw = vec![
            json!({"name": "no total", "type": "interval", "work": 45, "rest": 15}),
            json!({"name": "zero work", "type": "interval", "work": 0, "rest": 15, "total": 600}),
            json!({"name": "neg rest", "type": "interval", "work": 45, "rest": -1, "total": 600}),
            json!({"name": "total lt work", "type": "interval", "work": 45, "rest": 15, "total": 30}),
            json!({"name": "bool work", "type": "interval", "work": true, "rest": 15, "total": 600}),
            json!({"name": "str total", "type": "interval", "work": 45, "rest": 15, "total": "600"}),
        ];
        assert!(valid_presets(&raw).is_empty());
    }

    #[test]
    fn mixed_list_keeps_both_kinds() {
        let cd = json!({"name": "5 min", "duration": 300});
        let iv = json!({"name": "hiit", "type": "interval", "work": 45, "rest": 15, "total": 600});
        assert_eq!(
            valid(&[cd.clone(), iv.clone(), json!({"junk": 1})]),
            vec![cd, iv]
        );
    }

    #[test]
    fn rounds_from_total() {
        assert_eq!(interval_preset_rounds(45, 15, 600), 10);
    }

    #[test]
    fn rounds_zero_rest() {
        assert_eq!(interval_preset_rounds(30, 0, 1200), 40);
    }

    #[test]
    fn rounds_at_least_one() {
        assert_eq!(interval_preset_rounds(45, 15, 45), 1);
    }

    #[test]
    fn rounds_truncates_partial_cycle() {
        assert_eq!(interval_preset_rounds(45, 15, 630), 10);
    }

    // ---- TestStopwatchPresets ----
    #[test]
    fn valid_stopwatch_preset_accepted() {
        let p = json!({"name": "45/15 up", "type": "stopwatch", "work": 45, "rest": 15});
        assert_eq!(valid(std::slice::from_ref(&p)), vec![p]);
    }

    #[test]
    fn stopwatch_zero_rest_allowed() {
        let p = json!({"name": "laps", "type": "stopwatch", "work": 60, "rest": 0});
        assert_eq!(valid(std::slice::from_ref(&p)), vec![p]);
    }

    #[test]
    fn malformed_stopwatch_presets_filtered() {
        let raw = vec![
            json!({"name": "no work", "type": "stopwatch"}),
            json!({"name": "zero work", "type": "stopwatch", "work": 0, "rest": 15}),
            json!({"name": "neg rest", "type": "stopwatch", "work": 45, "rest": -1}),
            json!({"name": "bool", "type": "stopwatch", "work": true, "rest": 0}),
            json!({"name": "str", "type": "stopwatch", "work": "45", "rest": 0}),
            json!({"name": "legacy shape", "type": "stopwatch", "interval": 60}),
        ];
        assert!(valid_presets(&raw).is_empty());
    }

    #[test]
    fn all_three_kinds_coexist() {
        let cd = json!({"name": "5 min", "duration": 300});
        let iv = json!({"name": "hiit", "type": "interval", "work": 45, "rest": 15, "total": 600});
        let sw = json!({"name": "45/15 up", "type": "stopwatch", "work": 45, "rest": 15});
        assert_eq!(
            valid(&[cd.clone(), iv.clone(), sw.clone()]),
            vec![cd, iv, sw]
        );
    }

    // ---- TestPresetSoundRules ----
    #[test]
    fn sound_toggles_accepted_on_all_types() {
        let snd = |mut v: Value| {
            let o = v.as_object_mut().unwrap();
            o.insert("last5".into(), json!(true));
            o.insert("boundary".into(), json!(false));
            o.insert("halfway".into(), json!(true));
            v
        };
        let presets = vec![
            snd(json!({"name": "cd", "duration": 300})),
            snd(json!({"name": "iv", "type": "interval", "work": 45, "rest": 15, "total": 600})),
            snd(json!({"name": "sw", "type": "stopwatch", "work": 45, "rest": 15})),
        ];
        assert_eq!(valid(&presets), presets);
        let (_, rules, _) = &valid_presets(&presets)[0];
        assert_eq!(
            rules,
            &PresetRules {
                last5: Some(true),
                boundary: Some(false),
                halfway: Some(true)
            }
        );
    }

    #[test]
    fn sound_fields_optional() {
        let p = json!({"name": "plain", "duration": 300});
        assert_eq!(valid(std::slice::from_ref(&p)), vec![p]);
    }

    #[test]
    fn legacy_every_before_keys_ignored() {
        let p = json!({"name": "old", "duration": 300, "every": 60, "before": 5});
        let out = valid_presets(std::slice::from_ref(&p));
        assert_eq!(out.len(), 1); // keys tolerated, no longer validated
        assert_eq!(out[0].0.name(), "old");
    }

    #[test]
    fn malformed_toggles_filtered() {
        let raw = vec![
            json!({"name": "bad last5", "duration": 300, "last5": "yes"}),
            json!({"name": "bad boundary", "duration": 300, "boundary": "no"}),
            json!({"name": "bad halfway", "duration": 300, "halfway": 1}),
        ];
        assert!(valid_presets(&raw).is_empty());
    }

    // ---- additional ----
    #[test]
    fn wrong_typed_field_falls_back_alone() {
        let cfg = Config::from_value(&json!({"font_size": "big", "opacity": 0.5}));
        assert_eq!(cfg.font_size, 30);
        assert_eq!(cfg.opacity, 0.5);
    }

    #[test]
    fn int_fields_reject_floats_and_bools() {
        let cfg = Config::from_value(&json!({"font_size": 12.5, "margin": true, "padding": 7}));
        assert_eq!((cfg.font_size, cfg.margin, cfg.padding), (30, 15, 7));
    }

    #[test]
    fn unknown_keys_survive_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"future_key": [1,2]}"#).unwrap();
        Config::load_from(&path).save_to(&path).unwrap();
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["future_key"], json!([1, 2]));
    }

    #[test]
    fn python_default_fixture_loads() {
        let cfg = Config::load_from(&fixture("python_default.json"));
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn legacy_fixture() {
        let cfg = Config::load_from(&fixture("legacy_hand_edited.json"));
        assert_eq!(cfg.font_size, 24);
        assert_eq!(cfg.timer_mode, Mode::Countdown);
        assert_eq!(cfg.presets.len(), 3);
        let names: Vec<_> = valid_presets(&cfg.presets)
            .iter()
            .map(|(p, _, i)| (p.name().to_string(), *i))
            .collect();
        assert_eq!(names, vec![("old".to_string(), 0), ("hiit".to_string(), 1)]);
    }

    #[test]
    fn saved_file_keeps_python_keys() {
        let py: Value =
            serde_json::from_str(&std::fs::read_to_string(fixture("python_default.json")).unwrap())
                .unwrap();
        let ours = Config::default().to_value();
        for (k, v) in py.as_object().unwrap() {
            let got = ours.get(k).unwrap_or_else(|| panic!("missing key {k}"));
            let same_type = std::mem::discriminant(got) == std::mem::discriminant(v);
            assert!(same_type, "type mismatch for {k}: {got} vs {v}");
        }
        assert_eq!(ours["timer_mode"], json!("stopwatch"));
    }

    #[test]
    fn timer_mode_serializes_lowercase() {
        for (m, s) in [
            (Mode::Stopwatch, "stopwatch"),
            (Mode::Countdown, "countdown"),
            (Mode::Interval, "interval"),
        ] {
            let cfg = Config {
                timer_mode: m,
                ..Config::default()
            };
            assert_eq!(cfg.to_value()["timer_mode"], json!(s));
        }
    }

    #[test]
    fn save_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = cfg_in(&dir);
        Config::default().save_to(&path).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn save_preserves_existing_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = cfg_in(&dir);
        Config::default().save_to(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        Config::default().save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn save_to_readonly_dir_returns_err() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let res = Config::default().save_to(&dir.path().join("config.json"));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(res.is_err());
    }

    #[test]
    fn new_keys_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.clock_format, "24h");
        assert_eq!(cfg.hotkey_toggle, "");
        assert!(!cfg.notify_on_finish);
    }
}
