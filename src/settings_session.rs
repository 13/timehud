//! Bookkeeping for a live settings session, shared by the HUD and the
//! settings process. Pure (no X, no IO), working on config JSON objects.
//!
//! Both sides track the same two configs:
//!
//! - `local` (HUD side: [`Session::prev`]): what the settings window holds,
//!   i.e. the config of its last message;
//! - `base`: the HUD's values as of the hello or the last commit, kept
//!   current (by [`adopt_hud_config`]) for every key the user has not edited
//!   in the window.
//!
//! The keys where `local` differs from `base` ([`modified_keys`]) are the
//! user's unsaved edits: HUD-side changes to other keys are pushed to the
//! window and adopted on both sides; Cancel restores `base` for the edited
//! keys only, so HUD-side edits made meanwhile survive.

use serde_json::Value;
use std::collections::BTreeSet;

/// Keys that together define where the HUD sits: if settings changed any of
/// them, all of them come from settings (a preset pick must beat a drag).
pub const POSITION_GROUP: [&str; 3] = ["position", "custom_x", "custom_y"];

/// Applies onto `current` only the top-level keys the settings process
/// changed between its previous message `prev` and `new`, so HUD-side edits
/// made meanwhile (drag position, wheel font size, ...) survive.
/// [`POSITION_GROUP`] is merged as one unit.
pub fn merge_settings_changes(current: &Value, prev: &Value, new: &Value) -> Value {
    let mut out = current.clone();
    if let (Some(out_map), Some(new_map)) = (out.as_object_mut(), new.as_object()) {
        let changed = |k: &str| new_map.get(k).is_some_and(|v| prev.get(k) != Some(v));
        let group_changed = POSITION_GROUP.iter().any(|k| changed(k));
        for (k, v) in new_map {
            if changed(k) || (group_changed && POSITION_GROUP.contains(&k.as_str())) {
                out_map.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// Top-level keys whose value differs between `local` and `base` (either
/// side may lack a key); any [`POSITION_GROUP`] key brings the whole group.
pub fn modified_keys(local: &Value, base: &Value) -> BTreeSet<String> {
    let keys = |v: &Value| -> Vec<String> {
        v.as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut out: BTreeSet<String> = keys(local)
        .into_iter()
        .chain(keys(base))
        .filter(|k| local.get(k) != base.get(k))
        .collect();
    if POSITION_GROUP.iter().any(|k| out.contains(*k)) {
        out.extend(POSITION_GROUP.iter().map(|k| k.to_string()));
    }
    out
}

/// Takes the HUD's config `hud` into `local` and `base` for every key the
/// user has not edited (see [`modified_keys`]); edited keys stay as they
/// are. Returns the adopted keys whose value changed in `local`.
pub fn adopt_hud_config(local: &mut Value, base: &mut Value, hud: &Value) -> Vec<String> {
    let modified = modified_keys(local, base);
    let mut adopted = Vec::new();
    let (Some(hud_map), Some(local_map), Some(base_map)) =
        (hud.as_object(), local.as_object_mut(), base.as_object_mut())
    else {
        return adopted;
    };
    for (k, v) in hud_map {
        if modified.contains(k) {
            continue;
        }
        if local_map.get(k) != Some(v) {
            adopted.push(k.clone());
            local_map.insert(k.clone(), v.clone());
        }
        base_map.insert(k.clone(), v.clone());
    }
    adopted
}

/// The HUD's view of the connected settings process.
#[derive(Debug, Clone)]
pub struct Session {
    /// What the settings window holds: its last message, plus adopted pushes.
    prev: Value,
    /// Baseline the window's edits are measured against (and Cancel restores).
    base: Value,
}

impl Session {
    /// A session that just received the HUD config `hud` (the hello reply).
    pub fn new(hud: &Value) -> Session {
        Session {
            prev: hud.clone(),
            base: hud.clone(),
        }
    }

    /// A Preview (or with `commit` a Commit) of `new`: returns the config
    /// to apply, `current` with only what settings changed merged in.
    /// A commit makes `new` the baseline Cancel returns to.
    pub fn apply(&mut self, current: &Value, new: Value, commit: bool) -> Value {
        let merged = merge_settings_changes(current, &self.prev, &new);
        self.prev = new;
        if commit {
            self.base = self.prev.clone();
        }
        merged
    }

    /// Cancel: `current` with the baseline value restored for every key the
    /// window edited since the hello or last commit, `None` if it edited
    /// nothing. The session is back at its baseline afterwards.
    pub fn cancel(&mut self, current: &Value) -> Option<Value> {
        let modified = modified_keys(&self.prev, &self.base);
        if modified.is_empty() {
            return None;
        }
        let mut out = current.clone();
        if let Some(map) = out.as_object_mut() {
            for k in &modified {
                match self.base.get(k) {
                    Some(v) => map.insert(k.clone(), v.clone()),
                    None => map.remove(k),
                };
            }
        }
        self.prev = self.base.clone();
        Some(out)
    }

    /// After a HUD-side config change: the config to push to the window if
    /// it holds a stale value for any key the user has not edited there,
    /// else `None`. Records the push the way the window adopts it.
    pub fn hud_changed(&mut self, current: &Value) -> Option<Value> {
        let modified = modified_keys(&self.prev, &self.base);
        let stale = current
            .as_object()?
            .iter()
            .any(|(k, v)| !modified.contains(k) && self.prev.get(k) != Some(v));
        if !stale {
            return None;
        }
        adopt_hud_config(&mut self.prev, &mut self.base, current);
        Some(current.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_applies_only_changed_keys() {
        let current =
            json!({"font_size": 40, "custom_x": 500, "opacity": 0.88, "theme": "classic"});
        let prev = json!({"font_size": 30, "custom_x": -1, "opacity": 0.88, "theme": "classic"});
        let new = json!({"font_size": 30, "custom_x": -1, "opacity": 0.5, "theme": "classic"});
        assert_eq!(
            merge_settings_changes(&current, &prev, &new),
            json!({"font_size": 40, "custom_x": 500, "opacity": 0.5, "theme": "classic"})
        );
    }

    #[test]
    fn merge_settings_wins_on_keys_it_changed() {
        // Settings typed a custom_x: the whole position group comes from
        // settings (its custom_y -1 too), overriding the HUD drag.
        let current = json!({"position": "top-right", "custom_x": 500, "custom_y": 20});
        let prev = json!({"position": "top-right", "custom_x": -1, "custom_y": -1});
        let new = json!({"position": "top-right", "custom_x": 10, "custom_y": -1});
        assert_eq!(
            merge_settings_changes(&current, &prev, &new),
            json!({"position": "top-right", "custom_x": 10, "custom_y": -1})
        );
    }

    #[test]
    fn merge_takes_position_group_whole_from_settings() {
        // The HUD was dragged (custom_*) while settings picked a preset
        // position: the pick wins, including its unset custom_*.
        let current =
            json!({"position": "top-right", "custom_x": 500, "custom_y": 20, "font_size": 40});
        let prev =
            json!({"position": "top-right", "custom_x": -1, "custom_y": -1, "font_size": 30});
        let new =
            json!({"position": "bottom-left", "custom_x": -1, "custom_y": -1, "font_size": 30});
        assert_eq!(
            merge_settings_changes(&current, &prev, &new),
            json!({"position": "bottom-left", "custom_x": -1, "custom_y": -1, "font_size": 40})
        );
        // Settings did not touch the group: the drag survives.
        let new = json!({"position": "top-right", "custom_x": -1, "custom_y": -1, "font_size": 50});
        assert_eq!(
            merge_settings_changes(&current, &prev, &new),
            json!({"position": "top-right", "custom_x": 500, "custom_y": 20, "font_size": 50})
        );
    }

    #[test]
    fn merge_adds_keys_missing_from_prev_and_compares_deeply() {
        let current =
            json!({"presets": [{"name": "a", "duration": 60}, {"name": "b", "duration": 5}]});
        let prev = json!({"presets": [{"name": "a", "duration": 60}]});
        let new = json!({"presets": [{"name": "a", "duration": 60}], "future_key": 1});
        // presets unchanged on the settings side: the HUD-saved preset stays.
        assert_eq!(
            merge_settings_changes(&current, &prev, &new),
            json!({"presets": [{"name": "a", "duration": 60}, {"name": "b", "duration": 5}], "future_key": 1})
        );
    }

    #[test]
    fn merge_ignores_non_object_messages() {
        let current = json!({"font_size": 40});
        assert_eq!(
            merge_settings_changes(&current, &json!({}), &json!([1, 2])),
            current
        );
    }

    fn hud() -> Value {
        json!({
            "position": "top-right", "custom_x": -1, "custom_y": -1,
            "font_size": 30, "theme": "classic",
            "presets": [{"name": "a", "duration": 60}],
        })
    }

    fn with(v: &Value, k: &str, x: Value) -> Value {
        let mut v = v.clone();
        v[k] = x;
        v
    }

    #[test]
    fn modified_keys_group_position() {
        let base = hud();
        assert!(modified_keys(&base, &base).is_empty());
        let local = with(&with(&base, "custom_x", json!(5)), "theme", json!("nord"));
        let keys: Vec<_> = modified_keys(&local, &base).into_iter().collect();
        assert_eq!(keys, ["custom_x", "custom_y", "position", "theme"]);
    }

    #[test]
    fn cancel_without_previews_does_nothing() {
        let mut s = Session::new(&hud());
        // HUD-side edits meanwhile (drag, wheel) are not the window's.
        let current = with(
            &with(&hud(), "custom_x", json!(500)),
            "font_size",
            json!(44),
        );
        s.hud_changed(&current);
        assert_eq!(s.cancel(&current), None);
    }

    #[test]
    fn cancel_reverts_only_what_settings_changed() {
        let mut s = Session::new(&hud());
        let preview = with(&hud(), "theme", json!("nord"));
        let current = s.apply(&hud(), preview, false);
        // Then the HUD is dragged and its font wheeled.
        let current = with(
            &with(&current, "custom_x", json!(500)),
            "font_size",
            json!(44),
        );
        assert!(s.hud_changed(&current).is_some());
        let restored = s.cancel(&current).unwrap();
        assert_eq!(
            restored,
            with(
                &with(&hud(), "custom_x", json!(500)),
                "font_size",
                json!(44)
            )
        );
        // Back at baseline: a second Cancel (window close) is a no-op.
        assert_eq!(s.cancel(&restored), None);
    }

    #[test]
    fn cancel_restores_position_group_whole() {
        let mut s = Session::new(&hud());
        let preview = with(&hud(), "position", json!("bottom-left"));
        let current = s.apply(&hud(), preview, false);
        let restored = s.cancel(&current).unwrap();
        assert_eq!(restored, hud());
    }

    #[test]
    fn cancel_returns_to_last_commit() {
        let mut s = Session::new(&hud());
        let applied = with(&hud(), "theme", json!("nord"));
        let current = s.apply(&hud(), applied.clone(), true);
        assert_eq!(s.cancel(&current), None);
        let preview = with(&applied, "font_size", json!(50));
        let current = s.apply(&current, preview, false);
        assert_eq!(s.cancel(&current).unwrap(), applied);
    }

    #[test]
    fn cancel_restores_a_value_the_hud_set_before_settings_touched_it() {
        let mut s = Session::new(&hud());
        let current = with(&hud(), "font_size", json!(44)); // wheel
        let pushed = s.hud_changed(&current).unwrap();
        // The window adopts the push, then edits the font size itself.
        let preview = with(&pushed, "font_size", json!(60));
        let current = s.apply(&current, preview, false);
        assert_eq!(current["font_size"], 60);
        assert_eq!(s.cancel(&current).unwrap()["font_size"], 44);
    }

    #[test]
    fn hud_saved_preset_survives_later_settings_edits() {
        // Settings is open; the HUD saves a preset from its menu.
        let mut s = Session::new(&hud());
        let mut win_local = hud();
        let mut win_base = hud();
        let presets = json!([{"name": "a", "duration": 60}, {"name": "b", "duration": 5}]);
        let current = with(&hud(), "presets", presets.clone());
        let pushed = s.hud_changed(&current).expect("push the new presets");
        assert_eq!(
            adopt_hud_config(&mut win_local, &mut win_base, &pushed),
            ["presets"]
        );
        // Now an edit in the window (a font change) does not bring back
        // the stale presets list...
        let preview = with(&win_local, "font_size", json!(40));
        let current = s.apply(&current, preview.clone(), false);
        assert_eq!(current["presets"], presets);
        // ... and neither does a preset edit in the window: it starts from
        // the adopted list.
        let edited = json!([{"name": "a", "duration": 90}, {"name": "b", "duration": 5}]);
        let current = s.apply(&current, with(&preview, "presets", edited.clone()), false);
        assert_eq!(current["presets"], edited);
    }

    #[test]
    fn push_keeps_the_windows_unsaved_edits() {
        let mut s = Session::new(&hud());
        let mut win_local = with(&hud(), "theme", json!("nord"));
        let mut win_base = hud();
        let current = s.apply(&hud(), win_local.clone(), false);
        // The HUD changes the theme too (tray menu) and the font size.
        let current = with(
            &with(&current, "theme", json!("amber")),
            "font_size",
            json!(50),
        );
        let pushed = s.hud_changed(&current).unwrap();
        assert_eq!(
            adopt_hud_config(&mut win_local, &mut win_base, &pushed),
            ["font_size"]
        );
        assert_eq!(win_local["theme"], "nord");
        assert_eq!(win_local["font_size"], 50);
        // Both sides agree on what the window edited.
        assert_eq!(
            modified_keys(&win_local, &win_base),
            modified_keys(&s.prev, &s.base)
        );
        assert_eq!(s.prev, win_local);
        // Only the window's own key is restored on Cancel.
        assert_eq!(
            s.cancel(&current).unwrap(),
            with(
                &with(&hud(), "theme", json!("classic")),
                "font_size",
                json!(50)
            )
        );
    }

    #[test]
    fn no_push_when_the_window_is_current() {
        let mut s = Session::new(&hud());
        assert_eq!(s.hud_changed(&hud()), None);
        // A merge result the window already holds needs no push.
        let preview = with(&hud(), "theme", json!("nord"));
        let current = s.apply(&hud(), preview, false);
        assert_eq!(s.hud_changed(&current), None);
        // A HUD change to a key only the window edited is not pushed either.
        let current = with(&current, "theme", json!("amber"));
        assert_eq!(s.hud_changed(&current), None);
    }
}
