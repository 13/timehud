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
        for k in settings_changed_keys(prev, new) {
            if let Some(v) = new_map.get(&k) {
                out_map.insert(k, v.clone());
            }
        }
    }
    out
}

/// The keys of `new` that [`merge_settings_changes`] takes from settings:
/// those differing from `prev`, plus all of [`POSITION_GROUP`] if any of it.
fn settings_changed_keys(prev: &Value, new: &Value) -> BTreeSet<String> {
    let Some(new_map) = new.as_object() else {
        return BTreeSet::new();
    };
    let changed = |k: &str| new_map.get(k).is_some_and(|v| prev.get(k) != Some(v));
    let group_changed = POSITION_GROUP.iter().any(|k| changed(k));
    new_map
        .keys()
        .filter(|k| changed(k) || (group_changed && POSITION_GROUP.contains(&k.as_str())))
        .cloned()
        .collect()
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
    /// Revision of the last push ([`Session::hud_changed`]); the window
    /// echoes the last one it received in every Preview/Commit.
    rev: u64,
    /// Pushes the window may not have received yet: (revision, key, the
    /// key's value in `prev` before that push; `None` = absent).
    pending: Vec<(u64, String, Option<Value>)>,
}

impl Session {
    /// A session that just received the HUD config `hud` (the hello reply,
    /// revision 0).
    pub fn new(hud: &Value) -> Session {
        Session {
            prev: hud.clone(),
            base: hud.clone(),
            rev: 0,
            pending: Vec::new(),
        }
    }

    /// The revision of the last push (0 before any).
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// A Preview (or with `commit` a Commit) of `new`, sent by a window
    /// that had received the pushes up to revision `seen`: returns the
    /// config to apply, `current` with only what settings changed merged in.
    /// A key pushed after `seen` that still holds its pre-push value in
    /// `new` is stale, not an edit (the window adopts the push on arrival).
    /// A commit makes `new` the baseline Cancel returns to.
    pub fn apply(&mut self, current: &Value, new: Value, seen: u64, commit: bool) -> Value {
        self.pending.retain(|(r, _, _)| *r > seen);
        // What the window held for each key when it sent `new`: the value
        // before the earliest push it has not seen, else `prev`'s.
        let mut window_prev = self.prev.clone();
        if let Some(map) = window_prev.as_object_mut() {
            for (_, k, old) in self.pending.iter().rev() {
                match old {
                    Some(v) => map.insert(k.clone(), v.clone()),
                    None => map.remove(k),
                };
            }
        }
        let taken = settings_changed_keys(&window_prev, &new);
        let merged = merge_settings_changes(current, &window_prev, &new);
        // Stale keys: the window will hold the pushed value once it adopts it.
        let mut next = new;
        if let (Some(next_map), Some(prev_map)) = (next.as_object_mut(), self.prev.as_object()) {
            for (_, k, _) in &self.pending {
                if !taken.contains(k) {
                    match prev_map.get(k) {
                        Some(v) => next_map.insert(k.clone(), v.clone()),
                        None => next_map.remove(k),
                    };
                }
            }
        }
        self.prev = next;
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

    /// After a HUD-side config change: the config to push to the window
    /// and its revision, if the window holds a stale value for any key the
    /// user has not edited there, else `None`. Records the push the way the
    /// window adopts it.
    pub fn hud_changed(&mut self, current: &Value) -> Option<(Value, u64)> {
        let modified = modified_keys(&self.prev, &self.base);
        let stale = current
            .as_object()?
            .iter()
            .any(|(k, v)| !modified.contains(k) && self.prev.get(k) != Some(v));
        if !stale {
            return None;
        }
        let before = self.prev.clone();
        self.rev += 1;
        for k in adopt_hud_config(&mut self.prev, &mut self.base, current) {
            let old = before.get(&k).cloned();
            self.pending.push((self.rev, k, old));
        }
        Some((current.clone(), self.rev))
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
        let current = s.apply(&hud(), preview, s.rev(), false);
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
        let current = s.apply(&hud(), preview, s.rev(), false);
        let restored = s.cancel(&current).unwrap();
        assert_eq!(restored, hud());
    }

    #[test]
    fn cancel_returns_to_last_commit() {
        let mut s = Session::new(&hud());
        let applied = with(&hud(), "theme", json!("nord"));
        let current = s.apply(&hud(), applied.clone(), s.rev(), true);
        assert_eq!(s.cancel(&current), None);
        let preview = with(&applied, "font_size", json!(50));
        let current = s.apply(&current, preview, s.rev(), false);
        assert_eq!(s.cancel(&current).unwrap(), applied);
    }

    #[test]
    fn cancel_restores_a_value_the_hud_set_before_settings_touched_it() {
        let mut s = Session::new(&hud());
        let current = with(&hud(), "font_size", json!(44)); // wheel
        let (pushed, _) = s.hud_changed(&current).unwrap();
        // The window adopts the push, then edits the font size itself.
        let preview = with(&pushed, "font_size", json!(60));
        let current = s.apply(&current, preview, s.rev(), false);
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
        let (pushed, _) = s.hud_changed(&current).expect("push the new presets");
        assert_eq!(
            adopt_hud_config(&mut win_local, &mut win_base, &pushed),
            ["presets"]
        );
        // Now an edit in the window (a font change) does not bring back
        // the stale presets list...
        let preview = with(&win_local, "font_size", json!(40));
        let current = s.apply(&current, preview.clone(), s.rev(), false);
        assert_eq!(current["presets"], presets);
        // ... and neither does a preset edit in the window: it starts from
        // the adopted list.
        let edited = json!([{"name": "a", "duration": 90}, {"name": "b", "duration": 5}]);
        let current = s.apply(
            &current,
            with(&preview, "presets", edited.clone()),
            s.rev(),
            false,
        );
        assert_eq!(current["presets"], edited);
    }

    #[test]
    fn push_keeps_the_windows_unsaved_edits() {
        let mut s = Session::new(&hud());
        let mut win_local = with(&hud(), "theme", json!("nord"));
        let mut win_base = hud();
        let current = s.apply(&hud(), win_local.clone(), s.rev(), false);
        // The HUD changes the theme too (tray menu) and the font size.
        let current = with(
            &with(&current, "theme", json!("amber")),
            "font_size",
            json!(50),
        );
        let (pushed, _) = s.hud_changed(&current).unwrap();
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
    fn stale_preview_after_a_push_keeps_the_hud_value() {
        let mut s = Session::new(&hud());
        // The HUD is dragged and pushes the new position ...
        let current = with(&hud(), "custom_x", json!(500));
        assert!(s.hud_changed(&current).is_some());
        // ... but the window sent a Preview (a theme edit) before it got
        // the push: its custom_x is the old -1, which is not an edit.
        let stale = with(&hud(), "theme", json!("nord"));
        let current = s.apply(&current, stale, 0, false);
        assert_eq!(current["custom_x"], 500);
        assert_eq!(current["theme"], "nord");
        // The window adopts the push, so nothing needs pushing again.
        assert_eq!(s.hud_changed(&current), None);
        // Its next Preview, having seen the push, carries the drag too.
        let mut win = with(&current, "font_size", json!(44));
        let current = s.apply(&current, win.clone(), s.rev(), false);
        assert_eq!(current, win);
        // A Preview from before a second push but after the first one.
        let dragged = with(&current, "custom_x", json!(700));
        let (_, rev) = s.hud_changed(&dragged).unwrap();
        win["opacity"] = json!(0.5);
        let current = s.apply(&dragged, win.clone(), rev - 1, false);
        assert_eq!(current["custom_x"], 700);
        assert_eq!(current["opacity"], 0.5);
    }

    #[test]
    fn stale_preview_still_carries_real_edits_of_pushed_keys() {
        let mut s = Session::new(&hud());
        let current = with(&hud(), "custom_x", json!(500));
        s.hud_changed(&current).unwrap();
        // The user picked a position before the drag's push arrived: the
        // pick wins, as a whole group.
        let stale = with(&hud(), "position", json!("bottom-left"));
        let current = s.apply(&current, stale.clone(), 0, false);
        assert_eq!(current, stale);
        // The window keeps its edit (the push hits an edited key), and the
        // HUD knows: no push, and Cancel restores the dragged position.
        assert_eq!(s.hud_changed(&current), None);
        assert_eq!(s.cancel(&current).unwrap()["custom_x"], 500);
    }

    #[test]
    fn no_push_when_the_window_is_current() {
        let mut s = Session::new(&hud());
        assert_eq!(s.hud_changed(&hud()), None);
        // A merge result the window already holds needs no push.
        let preview = with(&hud(), "theme", json!("nord"));
        let current = s.apply(&hud(), preview, s.rev(), false);
        assert_eq!(s.hud_changed(&current), None);
        // A HUD change to a key only the window edited is not pushed either.
        let current = with(&current, "theme", json!("amber"));
        assert_eq!(s.hud_changed(&current), None);
    }
}
