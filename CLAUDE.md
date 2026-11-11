# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

TimeHUD is a Rust (edition 2024) transparent HUD overlay for Linux/X11 showing a clock plus stopwatch/countdown/interval timer above fullscreen apps. It is a port of the Python/PyQt6 TimeHUD; config files stay compatible in both directions. Always an X11 client (XWayland on Wayland sessions); `--wayland` is a warning-only no-op.

## Commands

```bash
cargo run --release                 # run the HUD (cargo run -- toggle etc. for subcommands)
# control subcommands (IPC to the running HUD): toggle start pause reset show hide quit,
#   set <duration>, preset <name>, mode <stopwatch|countdown|interval>, status [--json]
cargo test                          # full suite (headless; no X server needed)
cargo test engine::                 # one module
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test   # the gate; must be clean before every commit
./install.sh                        # release build + install to $PREFIX (default ~/.local)
PREFIX=/tmp/x ./install.sh          # install elsewhere (use this when testing the script)
TIMEHUD_TRACE=1 timehud             # prints "first frame presented N ms after start" to stderr
```

Run manual checks with `XDG_CONFIG_HOME` pointed at a scratch dir so the real `~/.config/timehud` is untouched. Version lives only in `Cargo.toml` (`--version` reads `CARGO_PKG_VERSION`). Releases: pushing a `v*` tag runs `.github/workflows/release.yml` (tarball + GitHub or Gitea release, branching on `github.server_url`). CI (`ci.yml`) installs Rust via rustup on both hosts.

## Architecture

Two processes, one binary:

- **HUD process** (`timehud`): x11rb + tiny-skia + fontdue software rendering into an override-redirect ARGB window, driven by a `poll(2)` loop (X connection fd, IPC socket, signals) with no toolkit. Idle means blocked in poll; the loop wakes for the 100 ms tick, the 1 s clock and animation frames only. Owns the timer engine, tray (XEmbed), popup menus, sound and global hotkey grabs.
- **Settings process** (`timehud settings`, hidden subcommand; also `save-preset`): a separate `eframe`/egui window so the HUD never links a GUI toolkit's idle cost. Talks to the HUD over the Unix socket in `$XDG_RUNTIME_DIR/timehud.sock` (`ipc.rs`): live config edits go HUD-ward; the HUD merges only the keys the settings process changed (`merge_settings_changes`), so HUD-side edits (drag, wheel font size) survive.
- Single instance via the same socket: a second `timehud` toggles visibility; `toggle|start|pause|reset|set|preset|mode|status|show|hide|quit` subcommands send control requests (`main.rs::control_request`; HUD errors go to stderr with exit 1).

Module map (`src/`):

- `main.rs` — clap CLI, subcommand dispatch, single-instance bind.
- `engine.rs` — `TimerEngine`, the pure timer state machine (stopwatch/countdown/interval/cycling stopwatch, beep scheduling). Injectable clock; no X/IO. Port of Python `timer_engine.py`.
- `app.rs` + `app/` — `App`, the pure HUD state: input -> `Effect`s (redraw, save, quit...). Everything testable without X lives here (`app/tests.rs`).
- `config.rs` — `Config` + tolerant per-field JSON codec (wrong type falls back to that field's default; unknown keys preserved in `extra`). `config_path()` honors `XDG_CONFIG_HOME`.
- `themes.rs` — six themes (classic, terminal, glass, mono, nord, amber); `apply_theme` stamps colors into config, structure is read live via `get_theme`.
- `layout.rs`, `render.rs`, `text.rs`, `format.rs`, `anim.rs` — geometry/hit regions, tiny-skia drawing, fontdue text (bundled JetBrains Mono Bold + fontconfig fallback), time formatting, animations.
- `hud.rs` + `hud/` — the poll loop, X event dispatch, `TIMEHUD_TRACE`; `deadlines.rs` (tick/frame/save/raise/accept deadlines → poll timeout), `ipc_server.rs` (clients, non-blocking event queues, settings session).
- `x11/` — `mod.rs` (connection, windows, present), `overlay.rs`, `popup.rs` (menus), `tray.rs`, `tooltip.rs` (tray tooltip), `grabs.rs` (hotkey grabs).
- `menu.rs` — context-menu model shared by window and tray.
- `hotkey.rs` — combo parsing/formatting, egui key -> X keysym.
- `sound.rs` — in-memory beeps via rodio, lazy device open + release after idle, player-chain fallback.
- `ipc.rs` — socket path, `Request`/`Event` protocol, bind-or-detect.
- `settings/` — the egui settings window (`mod.rs`), presets tab, save-preset dialog.
- `settings_session.rs` — pure settings-session bookkeeping shared by both processes: merge only keys Settings changed, Cancel restores only the user's edited keys, HUD-side edits are pushed to the window (`Event::Config`) and adopted for unedited keys.
- `examples/snapshot.rs` — renders the HUD to a PNG for visual checks.

## Conventions

- Engine quirks are deliberate 1:1 ports from the Python engine and are pinned by tests; don't "fix" without intent: warn window only when remaining > 6 s; auto-restart not resetting `sound_alert_before` beats; display reverting to the full duration one tick after a countdown finishes. Ported tests that look wrong are still right.
- Keep the README settings-JSON example in sync with every `Config` field when adding config keys. A new field needs a default; old config files must keep loading, and a Rust-saved file must still load in the Python app.
- `active_preset` must be cleared whenever `countdown_duration` changes by means other than applying a preset (wheel handler and settings already do this); the HUD clears it whenever the setup no longer matches the preset (`config::active_preset_matches`), which also covers mode changes.
- Numeric config fields are clamped into working ranges on load and in `preview_config` (`config.rs`); new numeric fields need a range too.
- Beep timing: last-5 shorts at displayed 5..1, long beep exactly at 00:00 (countdown finish, every interval/cycle phase end, session end); only the interval/cycle phase-end beeps are gated by `phase_beeps`, the countdown-finish beep is not (same as Python). The fast double beep is exclusively the optional `halfway_beep` cue at half of each work phase. Cycle modes never play periodic `sound_interval` beeps or the `sound_alert_before` pre-beep. Pinned by tests.
- Preset shapes: countdown `{name, duration}`, interval `{name, type, work, rest, total}` (rounds derived from total), stopwatch `{name, type, work, rest}` (sets `stopwatch_work/rest`; count-up cycling, endless; work=0 means plain stopwatch).
- `progress_style`: `line` (bar under timer) | `border` (traced around the window outline) | `off`.
- Classic theme values must stay identical to the Python ones (guarded by tests).
- Global hotkeys are off by default (`""`); grabs are exclusive X11 grabs.
- Performance targets: launch to visible < 150 ms, HUD idle RSS < 40 MB, ~0% CPU while paused. Keep the HUD process free of GUI-toolkit dependencies.
- Add dependencies with `cargo add`, not hand-pinned versions.
