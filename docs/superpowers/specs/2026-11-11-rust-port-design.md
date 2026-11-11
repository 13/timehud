# TimeHUD in Rust — Design

Date: 2026-11-11
Source of truth for behavior: the Python app at `/home/ben/repo/timehud` (v0.8.1).
Target: a new Rust project in `/home/ben/repo/timehud2`.

## 1. Intent and why

TimeHUD is a small transparent HUD that sits above fullscreen apps (video,
games) and shows a clock plus a stopwatch / countdown / interval timer with
beeps. The Python/PyQt6 version works, but the user wants out of three costs:

- **Startup time** (Python + PyQt6 import is slow),
- **Memory** (a full Qt + interpreter process for a 200×120 px HUD),
- **Packaging hassle** (venv, AppImage via python-appimage, PYTHONPATH tricks).

The user asked for **full feature parity, plus improvements**: "enhance, make
better, optimize, more beautiful". They chose a **native Rust UI** (no Qt
bindings).

Success = one small binary that starts near-instantly, idles at a fraction of
the Python app's memory and CPU, behaves like TimeHUD in every way the user
relies on, reads the existing `~/.config/timehud/config.json` unchanged, and
looks noticeably more polished.

## 2. Environment (observed)

- Arch Linux, **X11** session (`DISPLAY=:0`), **AwesomeWM** + **picom**
  compositor. Rust 1.98 installed.
- Awesome's systray is **XEmbed-only** (no StatusNotifierItem host). A tray
  icon built on libappindicator/SNI (e.g. the `tray-icon` crate) would not
  appear. → The tray must speak XEmbed.
- JetBrains Mono is installed system-wide.

## 3. Goals

1. **Behavioral parity** with the Python app (sections 6–9 enumerate it).
2. **Config compatibility both ways**: the Rust app loads any config the Python
   app wrote; the Python app still loads a config the Rust app wrote (Python
   ignores unknown keys, so Rust may add keys but must not rename/retype
   existing ones).
3. **Performance targets** (measured on this machine, release build):
   - launch → overlay visible: **< 150 ms**
   - idle RSS (overlay only, settings closed): **< 40 MB**
   - idle CPU with timer paused: **~0%** (no 10 Hz polling when nothing moves)
4. **Packaging**: `cargo build --release` produces a single `timehud` binary;
   `install.sh` installs binary + icon + desktop entry into `~/.local`; AUR
   PKGBUILD builds from source. No venv, no AppImage.
5. **More beautiful** — see section 10.

## 4. Non-goals

- Windows / macOS.
- A native Wayland window of any kind. The app always runs as an X11 client
  (XWayland on Wayland sessions). `--wayland` is still accepted for
  compatibility but only prints a warning that it is ignored. *(My call after
  the builder check: frees the overlay to use any X11-only rendering approach.)*
- Recreating Qt's widget look in the settings window.

## 5. Anti-goals (failure even if it "works")

- Any change to **beep timing or timer semantics** vs. the Python engine
  (including the deliberate quirks listed in 6.3).
- Breaking or silently resetting the user's existing config/presets.
- Startup or idle footprint that isn't clearly better than the Python app.
- Overlay that doesn't stay above fullscreen video in Awesome.
- Tray icon that doesn't show up in Awesome's systray.

## 6. Timer engine (exact port)

### 6.1 Shape
A UI-free module (`engine.rs`) — no GUI/audio imports — with an injectable
monotonic clock (trait or closure returning `f64` seconds). The UI calls
`tick()` and receives a `TickResult { display, state, beeps, finished,
restarted, phase, round, rounds, progress }` with `state ∈ {Run, Pause, Warn,
End}`, `phase ∈ {None, Work, Rest}`, `Beep { short, double }`.
Commands: `toggle`, `reset`, `set_mode`, `adjust_countdown(delta)`,
`is_idle`, `elapsed`, `remaining`. `fmt_seconds` (MM:SS, HH:MM:SS ≥ 1 h).

### 6.2 Porting rule
Port `timer_engine.py` line-for-line in logic. **Port every test in
`tests/test_timer_engine.py` (65 tests), `test_config.py` and
`test_themes.py`** to Rust unit tests with the same scenarios and assertions.
These tests are the spec; if a Rust test disagrees with intuition, the Python
behavior wins.

### 6.3 Deliberate quirks to preserve (pinned by those tests)
- Warn window only when remaining > 6 s.
- Auto-restart does not reset `_sound_alert_before_beats`.
- Display reverts to full duration one tick after countdown finish.
- Beeps: last-5 shorts at displayed 5..1, long beep exactly at 00:00
  (countdown finish, every interval/cycle phase end, session end — gated by
  `phase_beeps`). Fast double beep only for `halfway_beep`. Cycle modes never
  play periodic `sound_interval` beeps or the `sound_alert_before` pre-beep.
- Countdown/interval labels display `ceil(display - 1e-9)`.

### 6.4 Tick cadence (optimization, not a semantic change)
While the timer runs: tick every **100 ms** (beep timing granularity stays as
in Python). While paused/idle: tick only at the next wall-clock second boundary
(clock update). Animations request frames only while animating.

## 7. Config

- Path `~/.config/timehud/config.json` (respect `$XDG_CONFIG_HOME`; default
  resolves to the same path).
- All existing fields, names, types and defaults exactly as in
  `src/timehud/config.py` (position, custom_x/y, margin, opacity, font_size,
  font_family, color_*, show_clock, show_timer, timer_mode,
  countdown_duration, interval_*, stopwatch_*, phase_beeps, halfway_beep,
  sound_*, show_tray_icon, show_controls, click_through,
  auto_restart_countdown, alert_last_5_seconds, presets, active_preset, theme,
  progress_style, padding, padding_top, row_spacing).
- **Improvements**:
  - Per-field tolerance: a malformed value falls back to that field's default
    instead of discarding the whole file (Python discards everything).
  - Unknown keys are preserved on save (so future/other-version keys survive).
  - Atomic save (write temp file + rename); saves are coalesced (debounced
    ~300 ms) so wheel-scrolling doesn't hammer the disk; always flushed on quit.
- Presets: same three shapes and validation as `valid_presets` /
  `interval_preset_rounds` (countdown `{name,duration}`, interval
  `{name,type,work,rest,total}`, stopwatch `{name,type,work,rest}`, optional
  `last5`/`boundary`/`halfway` bools). Presets are stored as raw JSON values so
  unknown per-preset keys survive.
- `active_preset` must be cleared whenever `countdown_duration` changes other
  than by applying a preset.
- New keys (all optional, defaults keep current behavior):
  - `clock_format`: `"24h"` (default, `HH:MM:SS`) | `"24h-short"` (`HH:MM`) |
    `"12h"`.
  - `hotkey_toggle` / `hotkey_reset` / `hotkey_visibility`: default `""`
    (disabled). Combo strings like `"Ctrl+Shift+Space"`.
  - `notify_on_finish`: `false` — desktop notification (via `notify-send`)
    when a countdown/interval session ends.
- README's settings JSON example updated to match.

## 8. Overlay window

### 8.1 Window behavior (parity)
- Frameless, transparent (per-pixel alpha via picom), always on top, no
  taskbar entry, does not steal focus when shown.
- **Above fullscreen**: on X11 use an **override-redirect** window (equivalent
  of Qt's `X11BypassWindowManagerHint`). Always use the X11 backend, even if
  `WAYLAND_DISPLAY` is set.
- Click-through (`click_through`): empty X11 input shape — all mouse input
  passes to the app below; toggled live from the menu / tray.
- Window opacity = `opacity`; fade-in from 0 on start.
- Position presets: top-left/right, bottom-left/right, top/bottom-center at
  `margin` px inside the available area of the screen under the cursor.
  Dragged position saved in `custom_x/custom_y`; `-1` = use preset.
- On content resize: preset positions re-anchor exactly; dragged positions keep
  their anchor per axis (center within 24 px of screen center → keep center;
  else keep the nearer edge) — port `_resize_anchor_delta` and test it.
- Drag with magnetic snap (20 px) to screen edges (snaps to `margin`) and to
  horizontal/vertical center.

### 8.2 Layout (parity)
Rows, top to bottom, centered: clock · optional separator (theme) · timer ·
progress bar (line style) · mode label · control buttons (▶/⏸, ↺, SW/CD/IV).
- Clock px = `font_size × clock_scale`, timer px = `font_size × timer_scale`,
  mode label px = `max(11, font_size × 0.42)`, all bold.
- Rows cropped to digit ink height; uniform `row_spacing` gap between rows;
  padding from `padding` / `padding_top` (−1 = same as padding), +4 px
  horizontal; bottom margin 8 px with buttons, `row_spacing` without, blended
  during fold.
- Mode label texts exactly as `_refresh_mode_label` and the live
  `WORK 3/8` / `REST 3/8` / endless `WORK 3` labels, phase-colored.
- Controls auto-fold (400 ms ease-in-out, height + opacity) 2 s after the
  pointer leaves, unless idle; unfold on hover. Button size from font size.

### 8.3 Interaction (parity)
- Timer label: left click toggle; double-click reset; wheel cycles mode;
  middle click cycles mode.
- Mode label: wheel on right part in countdown mode adjusts duration (x > 80%
  of width: ±1 s; 55–80%: ±60 s; never below 1 s); elsewhere cycles mode.
- Clock label: wheel changes `font_size` by ±2 (10..120).
- Right-click anywhere: context menu (8.4).
- Keys when focused: Space toggle, R reset, Escape hide, Ctrl+Q quit.

### 8.4 Context menu
Same structure and entries as `menus.py`: Settings… · Presets ▸ (Stopwatch,
presets with checkmarks and labels like `name 45/15 ×10`, `name 40/20 ↑`,
`name 05:00`; Save current as preset… (disabled for plain stopwatch); Manage
presets…) · Theme ▸ · Click-Through: ON/OFF (when tray enabled) · Opacity ▸
(30/50/70/85/95/100 + "Current: N%") · Position ▸ · Show/Hide Overlay (when
tray enabled) · Quit.

Because the overlay is tiny, the menu is its **own popup window** (override-
redirect, at the cursor, kept on-screen, submenus open on hover, closes on
outside click/Escape), styled in the active theme. The same menu is used for
the tray's right-click. "Save current as preset…" opens a small name prompt
window (same-name overwrites, captures last5/boundary/halfway).

### 8.5 Rendering of state
- Timer color by state: run → `color_timer_run`, pause →
  `color_timer_pause`, warn → theme warn, end → theme end; rest phase while
  running → theme rest. 200 ms color fade, except instant in countdown final
  6.5 s and on end.
- 4% scale pulse (180 ms) on beep in the countdown's last 6.5 s.
- Progress (`progress_style`): `line` = 3 px rounded bar under the timer;
  `border` = stroke traced along the rounded window outline covering exactly
  `progress` of the perimeter; `off`. Bar color: clock color, warn color in
  warn/end, rest color in rest.

## 9. Themes, sound, tray, hotkeys, CLI

### 9.1 Themes
Port the four themes' values exactly (classic, terminal, glass, mono —
`themes.py`). `apply_theme` stamps colors (+ font family if set) into config;
structure (radius, alphas, scales, separator) read live. Classic's colors,
alphas, radius and scales must stay identical (port `test_themes.py`).

### 9.2 Sound
- Built-in beeps generated in memory exactly as today: 880 Hz sine, amplitude
  20000/32767, 8 ms attack / 25 ms release envelope; long 0.5 s, short 0.1 s,
  double = 0.1 s + 0.1 s silence + 0.1 s.
- `sound_file` (wav/mp3/ogg/flac) if it exists, else built-in.
- Play **in-process** (lower latency than spawning a player, no temp files).
  Open the audio device lazily and release it after ~10 s of silence so the app
  isn't a permanent audio client. If no audio device can be opened, fall back
  to the old player chain (paplay → aplay → ffplay → mpv) with a temp WAV.
- `sound_enabled=false` mutes everything.

### 9.3 Tray (XEmbed)
- System tray icon via the **XEmbed** protocol (`_NET_SYSTEM_TRAY_S{n}`
  selection owner, `SYSTEM_TRAY_REQUEST_DOCK`), icon rendered from the app SVG
  at the tray's size. Re-dock if the tray manager restarts (Awesome restart).
- Left click: toggle overlay visibility. Right click: the context menu (with
  window actions). Tooltip-free is acceptable.
- `show_tray_icon` toggles live; `--no-tray` overrides for the process.

### 9.4 Global hotkeys
**User decision:** exclusive X11 key grabs on the root window (grab with all
NumLock/CapsLock/ScrollLock mask combinations), **off by default**: nothing is
bound until the user sets combos in Settings → Hotkeys (key-capture field;
shows Ctrl+Shift+Space / R / H as suggestions; warns that a grabbed combo no
longer reaches other apps). Re-grab live on change. A failed grab (combo
already taken) shows an inline error in Settings and logs a warning. The
`timehud toggle|reset|…` subcommands (9.5) are the recommended way to bind
keys from Awesome's rc.lua.

### 9.5 CLI and single instance
- Flags (parity): `--position <preset>`, `--reset-config`, `--wayland` (no-op +
  warning), `--no-tray`, `--version`.
- **New**: single instance via a Unix socket in `$XDG_RUNTIME_DIR`. Running
  `timehud` again while one is running toggles its visibility. Subcommands
  `timehud toggle | reset | show | hide | quit` control the running instance —
  handy for Awesome keybindings. If no instance is running, the subcommand
  prints a message and exits non-zero.
- Ctrl+C / SIGTERM: save config and exit cleanly.

## 10. "More beautiful" and "better" (builder's latitude within these)

Must have:
1. **Frame-accurate animation**: fade-in, fold, color fade, pulse and progress
   are animated per frame from timestamps (not stepped at 10 Hz). The progress
   bar / border reads engine time continuously while running, so it moves
   smoothly instead of jumping between ticks.
2. **No digit jitter in any font**: digits are laid out in fixed-width cells
   (width of the widest digit), so proportional fonts (Glass's Noto Sans) don't
   wobble.
3. **Crisp vector rendering**: anti-aliased rounded panel, border, progress
   stroke; HiDPI-aware (scale factor from X11/Xft.dpi).
4. **Vector control icons** (play/pause/reset drawn as shapes) instead of
   Unicode glyphs whose look depends on the installed font.
5. **Bundled default font** (JetBrains Mono, OFL) used when `font_family`
   can't be resolved, so the HUD looks right on any machine; system fonts are
   resolved by family name via fontconfig.
6. **Soft shadow** under the panel (theme property; off for Mono), subtle
   hover highlight on buttons.
7. Two **new themes** alongside the four originals: **Nord** (cool
   blue-grey) and **Amber** (retro amber phosphor). Same Theme structure.
8. **Redesigned settings window** (section 11).

## 11. Settings window

Non-modal, a separate normal (WM-managed) window; one instance at a time
(re-opening focuses it and can jump to a section, e.g. Presets from "Manage
presets…"). Sections: **Display**, **Timer**, **Presets**, **Sound**,
**Hotkeys** (new), **About** — sidebar navigation.

- Same controls and ranges as `settings_dialog.py`: theme, position,
  font size 12–80, padding 2–40, padding-top −1–40, row spacing 0–40,
  opacity 20–100%, font family, colors (bg, clock, run, pause) with built-in
  color pickers, show clock/timer/controls/tray, click-through; mode,
  countdown 5–86400 s, interval work 5–3600 / rest 0–3600 / rounds 1–99,
  progress style, auto-restart, last-5; preset list with add/update/remove for
  all three preset types (countdown duration 1 s–24 h; interval work/rest/total
  in minutes 1–1440; stopwatch work/rest) and per-preset sound rules; sound
  enabled, interval 0–3600, alert-before 0–3600, sound file with file picker.
- **Live apply**: every change applies to the running overlay immediately.
  **Cancel** reverts to the snapshot taken when the window opened; **OK/Apply**
  keep and save. Closing the window = Cancel (parity).
- Changing countdown duration here clears `active_preset`; interval changes
  while stopped reset the engine (parity with `_sync_*`).
- Presets: drag to reorder (new).
- File picker: shell out to `zenity`/`kdialog` if present, else a plain text
  path field (no GTK dependency).

## 12. Packaging & repo

- Cargo project at the repo root, binary `timehud`, edition 2024.
- Version **0.9.0** (`Cargo.toml` is the single source; `--version` reads it).
- Copy over from the Python repo: icons (`timehud.svg`, PNG sizes), desktop
  file, screenshot; README rewritten for Rust (build, install, controls,
  config example); CLAUDE.md rewritten for the Rust layout.
- `install.sh`: `cargo build --release`, install binary to `~/.local/bin`,
  icon to `~/.local/share/icons/hicolor/scalable/apps`, desktop entry to
  `~/.local/share/applications`.
- `packaging/aur/PKGBUILD`: builds from source with cargo.
- **User decision: new repo, CI on GitHub and Gitea.** `timehud2` is a fresh
  git repo. Whether it later replaces `13/timehud` or the AUR `timehud`
  package is decided later. The PKGBUILD here is a source build
  (`arch=('x86_64')`) with `url`/`source` as placeholders until a remote
  exists.
- CI (`.github/workflows`, run by GitHub-hosted runners **and** the user's
  self-hosted Gitea runner, branching on `github.server_url` like the Python
  repo's workflows): install a Rust toolchain explicitly via rustup (the Gitea
  runner has none; no GitHub-only actions without a per-host branch), install
  X11/GL/ALSA dev libs, then `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test`. On `v*` tag: build a release binary tarball and publish a
  release on whichever host ran it, mirroring how the Python repo's
  `build-appimage.yml` handles both. The AppImage workflow is dropped.

## 13. Testing

- All engine/config/theme tests ported (6.2) — headless, `cargo test`.
- Pure functions extracted and unit-tested: position presets, resize anchor
  delta, drag snapping, mode-label text, preset menu labels, preset
  validation/rounds, ceil display, wheel-step logic, config round-trip
  (no user config exists on this machine, so fixtures are generated from the
  Python `Config` defaults plus hand-edited/legacy variants — e.g. presets
  with legacy `every`/`before` keys, wrong-typed fields — to test per-field
  fallback; my call).
- Manual verification on this machine: overlay above a fullscreen mpv/browser
  video in Awesome; tray icon in Awesome's systray; global hotkeys; click-
  through; beeps; measure startup time and RSS against the targets in 3.3 and
  against the Python app.

## 14. Left to the builder

- Crate choices and plumbing, provided the requirements above hold. Expected
  shape: `winit` + `egui` with a GL (glow) backend — eframe if it can be made
  to do override-redirect/input-shape/multi-window cleanly, otherwise winit
  directly; `x11rb` for override-redirect, input shape, XEmbed tray, key grabs;
  a lightweight audio crate (e.g. rodio/cpal) with wav/mp3/ogg decoding;
  `serde_json`; an SVG rasterizer (resvg) for the tray icon.
- Module layout, internal APIs, exact animation curves beyond those stated,
  exact Nord/Amber colors, settings window visual design.
- How to measure the performance targets.
