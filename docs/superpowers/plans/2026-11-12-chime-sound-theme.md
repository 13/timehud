# Chime Sound Theme Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `sound_theme` setting (`"classic"` default, `"chime"`) whose chime set replaces the harsh 880 Hz beeps with soft synthesized bell tones, selectable and previewable in Settings → Sound.

**Architecture:** `sound.rs` gains a `SoundTheme` enum and a chime synthesizer; `builtin_samples` takes the theme, and the theme travels with every play request (`Msg::Play`, `Source::Builtin`) so a change applies at the next beep. Config stores the plain string (tolerant: unknown → classic). The settings process plays the preview itself through its own `Sound` instance (same binary).

**Tech Stack:** Rust 2024, rodio (existing), egui/eframe settings (existing). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-12-chime-sound-theme-design.md`

## Global Constraints

- Default `sound_theme` is `"classic"`; classic output must stay bit-identical to today's `beep_samples(880, 0.1)`, `beep_samples(880, 0.5)`, `double_beep_samples()`.
- Unknown `sound_theme` values play classic. Old configs load; a Rust-saved config must still load in the Python app (it ignores the key).
- A non-empty, decodable `sound_file` overrides every built-in sound regardless of theme.
- Beep timing and which beeps fire are unchanged (no `engine.rs` changes).
- No bundled audio assets; no new crates.
- Gate before every commit: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.

## Review Focus

1. Unknown/odd-cased `sound_theme` (`"Chime"`, `"bell"`, `""`) → classic, never a panic or silence. (Task 2 test `theme_from_name_is_tolerant`.)
2. `sound_file` set but missing/undecodable with chime theme → plays the **chime**, not classic. (Task 2 test `missing_file_falls_back_to_themed_builtin`.)
3. Player fallback (no rodio device): classic and chime WAVs cached in `$XDG_RUNTIME_DIR` must not collide. (Task 2 test `fallback_wav_names_differ_per_theme`.)
4. Switching theme while the HUD runs takes effect at the next beep without restart. (Task 2 test `play_sends_current_theme`.)
5. Last-5 chime ticks are 1 s apart and must not overlap/blur. (Task 2 test `chime_short_fits_tick_spacing`.)

---

### Task 1: `sound_theme` config field

**Files:**
- Modify: `src/config.rs` (in `config_fields!`, after `sound_file` at line ~179; tests module)
- Modify: `README.md` (settings JSON example, after `"sound_file": "",` at line ~200)
- Also commit: `docs/superpowers/specs/2026-11-12-chime-sound-theme-design.md`, this plan

**Interfaces:**
- Produces: `Config.sound_theme: String`, default `"classic"`, key `"sound_theme"`.

- [ ] **Step 1: Write the failing test** in `config.rs` tests:

```rust
#[test]
fn sound_theme_defaults_and_round_trips() {
    assert_eq!(Config::default().sound_theme, "classic");
    // Old config without the key loads with the default.
    let old: Config = Config::from_json_str(r#"{"sound_enabled": true}"#);
    assert_eq!(old.sound_theme, "classic");
    // Unknown string values are kept verbatim (playback treats them as classic).
    let odd: Config = Config::from_json_str(r#"{"sound_theme": "bell"}"#);
    assert_eq!(odd.sound_theme, "bell");
    // Wrong type falls back to the default.
    let bad: Config = Config::from_json_str(r#"{"sound_theme": 3}"#);
    assert_eq!(bad.sound_theme, "classic");
}
```

Use whatever load-from-JSON helper the existing config tests use (e.g. the one in the test near line 1179) in place of `from_json_str`.

- [ ] **Step 2: Run** `cargo test config::` — expected FAIL (no field `sound_theme`).

- [ ] **Step 3: Implement.** Add `sound_theme: String = "classic".into();` with a doc comment `/// "classic" | "chime"; unknown plays classic.` right after `sound_file`. Add `"sound_theme": "classic",` after `"sound_file": "",` in the README JSON example.

- [ ] **Step 4: Run** `cargo test config::` — expected PASS, including `readme_settings_example_is_the_defaults`.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
git add src/config.rs README.md docs/superpowers/specs/2026-11-12-chime-sound-theme-design.md docs/superpowers/plans/2026-11-12-chime-sound-theme.md
git commit -m "feat(config): sound_theme setting (classic default)"
```

---

### Task 2: Chime synthesis and theme plumbing in `sound.rs`

**Files:**
- Modify: `src/sound.rs` (`builtin_samples` line ~79, `beep_name` ~89, `Source` ~230, `resolve_source`/`resolve_file` ~296-308, `Msg` ~310, `Worker::run` ~374, `Worker::play` ~412, `play_with_player` ~448, `Sound::play` ~523; tests)

**Interfaces:**
- Consumes: `Config.sound_theme` (Task 1).
- Produces:
  - `pub enum SoundTheme { Classic, Chime }` (`Clone, Copy, PartialEq, Eq, Debug, Default` = Classic)
  - `impl SoundTheme { pub fn from_name(name: &str) -> SoundTheme }` — exact match `"chime"` → Chime, else Classic.
  - `pub fn chime_samples(beep: Beep) -> Vec<i16>`
  - `fn builtin_samples(beep: Beep, theme: SoundTheme) -> Vec<i16>`
  - `fn wav_name(beep: Beep, theme: SoundTheme) -> String` — classic keeps today's `timehud-beep-{short|long|double}.wav`; chime → `timehud-beep-chime-{…}.wav`.
  - `Source::Builtin(Beep, SoundTheme)`; `Msg::Play(Beep, String, SoundTheme)`; `resolve_file(sound_file, beep, theme)`.
  - `Sound::play(&self, beep, cfg)` unchanged signature (Task 3 calls it for the preview).

- [ ] **Step 1: Write the failing tests** in the `sound.rs` tests module:

```rust
const LONG: Beep = Beep { short: false, double: false };
const SHORT: Beep = Beep { short: true, double: false };
const DOUBLE: Beep = Beep { short: false, double: true };

fn peak(s: &[i16]) -> i32 { s.iter().map(|v| i32::from(*v).abs()).max().unwrap() }
fn secs(s: &[i16]) -> f64 { s.len() as f64 / f64::from(SAMPLE_RATE) }

#[test]
fn classic_is_unchanged() {
    assert_eq!(builtin_samples(SHORT, SoundTheme::Classic), beep_samples(880.0, 0.1));
    assert_eq!(builtin_samples(LONG, SoundTheme::Classic), beep_samples(880.0, 0.5));
    assert_eq!(builtin_samples(DOUBLE, SoundTheme::Classic), double_beep_samples());
}

#[test]
fn theme_from_name_is_tolerant() {
    assert_eq!(SoundTheme::from_name("chime"), SoundTheme::Chime);
    for n in ["classic", "Chime", "bell", ""] {
        assert_eq!(SoundTheme::from_name(n), SoundTheme::Classic);
    }
}

#[test]
fn chime_short_fits_tick_spacing() {
    let s = chime_samples(SHORT);
    assert!(secs(&s) <= 0.4, "decays well within 1 s tick spacing");
}

#[test]
fn chime_lengths() {
    let long = secs(&chime_samples(LONG));
    assert!((1.0..=1.5).contains(&long));
    let double = secs(&chime_samples(DOUBLE));
    assert!(double > secs(&chime_samples(SHORT)) && double <= 0.7);
}

#[test]
fn chime_is_softer_and_click_free() {
    for b in [SHORT, LONG, DOUBLE] {
        let s = chime_samples(b);
        assert!(peak(&s) <= 14000, "softer than classic's 20000");
        assert!(peak(&s) >= 6000, "still audible");
        assert!(i32::from(s[0]).abs() < 100, "silent start");
        let tail = &s[s.len() - (SAMPLE_RATE as usize / 100)..]; // last 10 ms
        assert!(peak(tail) < 200, "decayed tail, no click");
    }
}

#[test]
fn missing_file_falls_back_to_themed_builtin() {
    let _guard = CACHE_TEST.lock().unwrap_or_else(|e| e.into_inner());
    assert!(matches!(
        resolve_file("/nonexistent/x.wav", LONG, SoundTheme::Chime),
        Source::Builtin(_, SoundTheme::Chime)
    ));
}

#[test]
fn fallback_wav_names_differ_per_theme() {
    assert_eq!(wav_name(LONG, SoundTheme::Classic), "timehud-beep-long.wav");
    for b in [SHORT, LONG, DOUBLE] {
        assert_ne!(wav_name(b, SoundTheme::Classic), wav_name(b, SoundTheme::Chime));
    }
}

#[test]
fn play_sends_current_theme() {
    let (tx, rx) = channel::<Msg>();
    let s = Sound { tx, file_cached: Cell::new(false) };
    let cfg = Config { sound_theme: "chime".into(), ..Config::default() };
    s.play(LONG, &cfg);
    assert!(matches!(rx.try_recv(), Ok(Msg::Play(_, _, SoundTheme::Chime))));
}
```

Update existing tests that construct/match `Source::Builtin(..)` / `Msg::Play(..)` to the new arity.

- [ ] **Step 2: Run** `cargo test sound::` — expected FAIL to compile (`SoundTheme`, `chime_samples`, `wav_name` undefined).

- [ ] **Step 3: Implement the synthesizer** `chime_samples(beep)`. Starting recipe (tune by ear afterwards, keeping the tests green):
  - One voice `chime_note(freq, dur, amp) -> Vec<f64>`: partials at frequency ratios `[1.0, 2.0, 3.0]` with relative amplitudes `[1.0, 0.25, 0.08]`, each decaying `exp(-t / tau)` with `tau = dur / 5` for the fundamental and faster for upper partials (`tau / ratio`); 5 ms linear attack; final 20 ms linear fade to 0 (guarantees the click-free tail).
  - short: `chime_note(1046.5, 0.30, 11000)` (C6).
  - long: `chime_note(659.3, 1.2, 9000)` (E5) mixed with `chime_note(880.0, 1.05, 9000)` (A5) starting 0.15 s later; total 1.2 s; clamp the mix into `i16`.
  - double: two `chime_note(1046.5, 0.22, 10000)` with onsets 0.15 s apart, overlapped-added; length ~0.37 s.
  Keep each output's peak within 6000..=14000 (normalize if the mix exceeds 14000).

- [ ] **Step 4: Implement plumbing.**
  - `builtin_samples(beep, theme)`: Classic → today's body; Chime → `chime_samples(beep)`.
  - `wav_name(beep, theme)` from `beep_name`; use it in `play_with_player`.
  - `resolve_source(cfg, beep)` passes `SoundTheme::from_name(&cfg.sound_theme)` into `resolve_file`, which returns `Source::Builtin(beep, theme)` on fallback.
  - `Sound::play` sends `Msg::Play(beep, cfg.sound_file.clone(), SoundTheme::from_name(&cfg.sound_theme))`; `Worker::run`/`Worker::play` thread the theme through to `resolve_file`.

- [ ] **Step 5: Run** `cargo test sound::` — expected PASS.

- [ ] **Step 6: Listen.** Write the three chime WAVs to the scratchpad with a throwaway test or `cargo test -- --ignored` helper using `wav_bytes(&chime_samples(..))`, play with `paplay`, and adjust pitches/levels if anything sounds harsh or muddy. Re-run `cargo test sound::`.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
git add src/sound.rs
git commit -m "feat(sound): soft chime sound theme"
```

---

### Task 3: Settings picker, preview, README

**Files:**
- Modify: `src/settings/mod.rs` (`fn sound` ~1061-1135; `SettingsApp` struct ~595 and its constructor ~651; new `SOUND_THEMES` next to `PROGRESS_STYLES` ~1364; `choice_labels` test ~1666)
- Modify: `README.md` Sound section (~line 258)

**Interfaces:**
- Consumes: `Config.sound_theme` (Task 1); `crate::sound::Sound::{new, play}` (Task 2); `crate::engine::Beep::default()` (= long beep).
- Produces: `const SOUND_THEMES: Choices` with items `[("classic", "Classic beep"), ("chime", "Soft chime")]`, `unknown_as: "classic"`.

- [ ] **Step 1: Write the failing test** — extend `choice_labels`:

```rust
let sound = |v| choice_label(&SOUND_THEMES, v);
assert_eq!(sound("chime"), "Soft chime");
assert_eq!(sound("classic"), "Classic beep");
assert_eq!(sound("bell"), "Classic beep");
```

- [ ] **Step 2: Run** `cargo test settings::` — expected FAIL (`SOUND_THEMES` undefined).

- [ ] **Step 3: Implement the UI.** Inside the `ui.add_enabled_ui(cfg.sound_enabled, …)` block, as the first card (before "Beeps"), add a card `"Sound"` with one row: `choice(ui, "sound-theme", &mut cfg.sound_theme, &SOUND_THEMES)` and a `"▶"` button (hover text `"Play the end-of-timer sound"`). On click, lazily create `self.preview: Option<Sound>` (new field, `None` in the constructor) and call `preview.play(Beep::default(), &cfg)`, so the preview honours `sound_file` exactly like the HUD. When `cfg.sound_file` is non-empty, show `note(ui, "The custom sound below overrides this.")` under the row. Borrow note: `fn sound` holds `let cfg = &mut self.cfg;` — restructure so `self.preview` and `self.cfg` are borrowed disjointly (e.g. destructure `let Self { cfg, preview, .. } = self;`).

- [ ] **Step 4: README.** In the Sound section, after the `sound_file` sentence, add: `` `sound_theme` picks the built-in sounds: `classic` (880 Hz beep, default) or `chime` (softer bell tones); Settings → Sound has a ▶ preview. A `sound_file` replaces either. ``

- [ ] **Step 5: Run** `cargo test settings::` — expected PASS.

- [ ] **Step 6: Manual check.** `XDG_CONFIG_HOME=<scratchpad>/cfg cargo run --release`, then `XDG_CONFIG_HOME=<scratchpad>/cfg cargo run --release -- settings`: pick Soft chime, press ▶ (hears chime), set a countdown of 0:08 with last-5 beeps on and confirm the HUD plays chime ticks and the end chime; switch back to Classic and confirm the old beep. Check `<scratchpad>/cfg/timehud/config.json` contains `"sound_theme"`.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
git add src/settings/mod.rs README.md
git commit -m "feat(settings): sound theme picker with preview"
```
