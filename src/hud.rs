//! HUD process: the event loop tying the controller to X11, IPC and sound.
//!
//! One thread, one `poll(2)` over the X connection, the signal pipe, the IPC
//! listener and its clients. Each iteration drains X events, runs the timer
//! tick when due, applies the controller's `Effect`s and presents a frame
//! when something changed or an animation runs.

use crate::app::{App, Button, Effect, Input, resolved_family};
use crate::config::Config;
use crate::hotkey::{Combo, format_combo, parse_combo};
use crate::ipc::{Conn, Event as IpcEvent, Request};
use crate::layout::Rect;
use crate::menu::{MenuAction, build_menu};
use crate::sound::Sound;
use crate::text::FontSet;
use crate::themes::get_theme;
use crate::x11::X;
use crate::x11::grabs::{Grabs, HotkeyAction};
use crate::x11::overlay::{OverlayWin, restack_needed, translate};
use crate::x11::popup::{MenuPopup, PopupOutcome};
use crate::x11::tray::{Tray, TrayEvent};
use chrono::Timelike;
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask};

/// Config writes are debounced this long after the last `SaveSoon`.
const SAVE_DEBOUNCE: f64 = 0.3;
/// `Sound::idle_release` cadence.
const IDLE_RELEASE: f64 = 1.0;

static START: OnceLock<Instant> = OnceLock::new();

/// Records the process start for the `TIMEHUD_TRACE=1` startup timing.
/// Call first thing in `main`.
pub fn mark_start() {
    let _ = START.set(Instant::now());
}

/// Keys that together define where the HUD sits: if settings changed any of
/// them, all of them come from settings (a preset pick must beat a drag).
const POSITION_GROUP: [&str; 3] = ["position", "custom_x", "custom_y"];

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

/// At most this many keep-on-top raises per second ([`RaiseLimiter`]).
const MAX_RAISES_PER_SEC: usize = 4;

/// Rate limit for keep-on-top raises, so a self-raising always-on-top
/// client cannot drag the HUD into a tight raise loop.
#[derive(Debug, Default)]
pub struct RaiseLimiter {
    /// Times of the recent raises, oldest first (at most `MAX_RAISES_PER_SEC`).
    recent: VecDeque<f64>,
}

impl RaiseLimiter {
    /// Records and allows a raise at `now` unless `MAX_RAISES_PER_SEC`
    /// raises already happened within the last second.
    pub fn try_raise(&mut self, now: f64) -> bool {
        while self.recent.front().is_some_and(|&t| now - t >= 1.0) {
            self.recent.pop_front();
        }
        if self.recent.len() >= MAX_RAISES_PER_SEC {
            return false;
        }
        self.recent.push_back(now);
        true
    }

    /// When the next raise can be allowed if the limit is reached now,
    /// `None` while fewer raises than the limit were recorded.
    pub fn next_allowed(&self) -> Option<f64> {
        (self.recent.len() >= MAX_RAISES_PER_SEC)
            .then(|| self.recent.front().map(|t| t + 1.0))
            .flatten()
    }
}

/// Hotkey bindings and messages for unparsable hotkey fields.
pub type HotkeySetup = (Vec<(Combo, HotkeyAction)>, Vec<String>);

/// Hotkey bindings from the three `hotkey_*` fields, plus messages for
/// fields that do not parse.
pub fn hotkey_binds(cfg: &Config) -> HotkeySetup {
    let fields = [
        (&cfg.hotkey_toggle, HotkeyAction::Toggle),
        (&cfg.hotkey_reset, HotkeyAction::Reset),
        (&cfg.hotkey_visibility, HotkeyAction::Visibility),
    ];
    let mut binds = Vec::new();
    let mut errors = Vec::new();
    for (text, action) in fields {
        match parse_combo(text) {
            Ok(Some(combo)) => binds.push((combo, action)),
            Ok(None) => {}
            Err(e) => errors.push(format!("Invalid hotkey '{text}': {e}")),
        }
    }
    (binds, errors)
}

/// Poll timeout in ms until the earliest deadline (seconds, relative to now);
/// `None` deadlines are ignored. Rounded up, never negative, at most 1000.
pub fn poll_timeout_ms(now: f64, deadlines: &[Option<f64>]) -> u16 {
    let earliest = deadlines.iter().flatten().fold(now + 1.0, |a, &d| a.min(d));
    ((earliest - now) * 1000.0).ceil().clamp(0.0, 1000.0) as u16
}

pub fn run_hud(cfg: Config, listener: UnixListener, no_tray: bool) -> anyhow::Result<()> {
    let socket: Option<PathBuf> = listener
        .local_addr()
        .ok()
        .and_then(|a| a.as_pathname().map(Into::into));
    let result = Hud::start(cfg, listener, no_tray).and_then(|mut hud| hud.run());
    if let Some(p) = socket {
        let _ = std::fs::remove_file(p);
    }
    result
}

struct Client {
    id: u64,
    conn: Conn,
}

/// The connected settings process.
struct SettingsSession {
    conn_id: u64,
    /// The config of its previous message (first: the one we sent it).
    prev: Value,
    /// What Cancel restores; `None` after Commit until the next Preview.
    snapshot: Option<Config>,
}

struct Hud {
    x: X,
    overlay: OverlayWin,
    app: App,
    t0: Instant,
    no_tray: bool,
    /// Second font set for menu popups (the overlay's belongs to `App`).
    menu_fonts: Option<FontSet>,
    popup: Option<MenuPopup>,
    tray: Option<Tray>,
    grabs: Grabs,
    sound: Sound,
    listener: UnixListener,
    signals: UnixStream,
    clients: Vec<Client>,
    next_client: u64,
    settings: Option<SettingsSession>,
    settings_child: Option<Child>,
    /// Tab requested when spawning settings, handed over in its `Config`.
    settings_tab: Option<String>,
    children: Vec<Child>,
    /// The work area the app positions in.
    area: Rect,
    /// Geometry last sent to the X server: x, y, w, h.
    geom: (i32, i32, i32, i32),
    /// Window size of the last rendered layout.
    size: (i32, i32),
    mapped: bool,
    dirty: bool,
    /// When the next animation frame is due (`App::next_frame`).
    ///
    /// Invariant: every `App` mutation goes through [`Hud::apply`] (which
    /// pulls `frame_due` earlier when an animation starts) or is followed by
    /// [`Hud::render`] (which recomputes it); otherwise a started animation
    /// would not wake the poll loop until the next tick.
    frame_due: Option<f64>,
    /// Another window went above ours; raise when `raises` allows.
    raise_pending: bool,
    raises: RaiseLimiter,
    /// The hotkey setup last applied (binds and parse errors).
    hotkeys: Option<HotkeySetup>,
    next_tick: f64,
    save_at: Option<f64>,
    next_idle: f64,
    /// An X event read while checking for pending events before `poll`.
    backlog: Option<Event>,
    quit: bool,
}

fn trace_enabled() -> bool {
    std::env::var_os("TIMEHUD_TRACE").is_some_and(|v| v == "1")
}

fn wall_frac(t: &chrono::NaiveTime) -> f64 {
    f64::from(t.nanosecond().min(999_999_999)) / 1e9
}

fn is_input(ev: &Event) -> bool {
    matches!(
        ev,
        Event::ButtonPress(_)
            | Event::ButtonRelease(_)
            | Event::MotionNotify(_)
            | Event::KeyPress(_)
            | Event::KeyRelease(_)
    )
}

impl Hud {
    /// Window first (connect → fonts → `App` → window → first frame), then
    /// the tray, hotkeys and everything else, for a fast first paint.
    fn start(cfg: Config, listener: UnixListener, no_tray: bool) -> anyhow::Result<Hud> {
        let t0 = Instant::now();
        let x = X::connect()?;
        let scale = x.scale();
        let (px, py) = x.pointer();
        let area = x.work_area_at(px, py);
        let fonts = FontSet::load(&resolved_family(&cfg));
        let clock = Box::new(move || t0.elapsed().as_secs_f64());
        let now = t0.elapsed().as_secs_f64();
        let mut app = App::new(cfg, clock, fonts, scale, area, now);
        let (pm, layout) = app.frame(now);
        let (wx, wy) = app.window_pos();
        let overlay = OverlayWin::create(&x, layout.w, layout.h, wx, wy)?;
        if app.cfg.click_through {
            overlay.set_click_through(&x, true);
        }
        overlay.map(&x);
        x.present(overlay.id, overlay.gc, &pm)?;
        if trace_enabled()
            && let Some(start) = START.get()
        {
            eprintln!(
                "timehud: first frame presented {:.1} ms after start",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }

        // Root: screen resizes (StructureNotify) and _NET_WORKAREA changes.
        let mask = x
            .conn
            .get_window_attributes(x.root)?
            .reply()?
            .your_event_mask;
        x.conn.change_window_attributes(
            x.root,
            &ChangeWindowAttributesAux::new()
                .event_mask(mask | EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
        )?;
        listener.set_nonblocking(true)?;
        let (signals, sig_writer) = UnixStream::pair()?;
        signals.set_nonblocking(true)?;
        sig_writer.set_nonblocking(true)?;
        for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            signal_hook::low_level::pipe::register(sig, sig_writer.try_clone()?)?;
        }

        let mut hud = Hud {
            x,
            overlay,
            app,
            t0,
            no_tray,
            menu_fonts: None,
            popup: None,
            tray: None,
            grabs: Grabs::default(),
            sound: Sound::new(),
            listener,
            signals,
            clients: Vec::new(),
            next_client: 0,
            settings: None,
            settings_child: None,
            settings_tab: None,
            children: Vec::new(),
            area,
            geom: (wx, wy, layout.w, layout.h),
            size: (layout.w, layout.h),
            mapped: true,
            dirty: false,
            frame_due: None,
            raise_pending: false,
            raises: RaiseLimiter::default(),
            hotkeys: None,
            next_tick: now,
            save_at: None,
            next_idle: now + IDLE_RELEASE,
            backlog: None,
            quit: false,
        };
        if hud.app.cfg.show_tray_icon && !no_tray {
            hud.set_tray(true);
        }
        hud.frame_due = hud.app.next_frame(hud.now());
        hud.sync_hotkeys(true);
        hud.x.conn.flush()?;
        Ok(hud)
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn run(&mut self) -> anyhow::Result<()> {
        let result = self.event_loop();
        if result.is_err() {
            // e.g. the X server went away: keep the user's settings.
            self.save();
        }
        result
    }

    fn event_loop(&mut self) -> anyhow::Result<()> {
        loop {
            self.drain_x()?;
            let now = self.now();
            self.service(now);
            if self.quit {
                self.shutdown();
                return Ok(());
            }
            self.render(now);
            self.x.conn.flush()?;
            // Replies read above may have queued events: don't sleep on them.
            let timeout = if self.peek_x()? {
                0
            } else {
                self.timeout(self.now())
            };
            self.wait(timeout)?;
        }
    }

    // ══ X events ════════════════════════════════════════════════════════════

    fn next_x(&mut self) -> anyhow::Result<Option<Event>> {
        if let Some(ev) = self.backlog.take() {
            return Ok(Some(ev));
        }
        Ok(self.x.conn.poll_for_event()?)
    }

    fn peek_x(&mut self) -> anyhow::Result<bool> {
        if self.backlog.is_none() {
            self.backlog = self.x.conn.poll_for_event()?;
        }
        Ok(self.backlog.is_some())
    }

    fn drain_x(&mut self) -> anyhow::Result<()> {
        while let Some(ev) = self.next_x()? {
            self.handle_x(&ev);
        }
        Ok(())
    }

    fn handle_x(&mut self, ev: &Event) {
        let now = self.now();
        match ev {
            Event::Error(e) => {
                if trace_enabled() {
                    eprintln!("timehud: X error {e:?}");
                }
                return;
            }
            Event::MappingNotify(_) => {
                self.x.reload_keymap();
                self.sync_hotkeys(true);
                return;
            }
            _ => {}
        }

        // Keep on top of windows that map or restack above us.
        let mut own = vec![self.overlay.id];
        if let Some(p) = &self.popup {
            own.extend(p.windows());
        }
        if self.mapped && restack_needed(ev, self.x.root, &own) {
            self.raise_pending = true;
            self.raise_if_allowed(now);
        }

        // An open menu sees every event first and swallows pointer/keyboard input.
        if let Some(popup) = self.popup.as_mut() {
            let fonts = self.menu_fonts.get_or_insert_with(FontSet::bundled);
            match popup.handle(&self.x, ev, fonts) {
                PopupOutcome::Continue => {}
                PopupOutcome::Closed => self.popup = None,
                PopupOutcome::Chosen(action) => {
                    self.popup = None;
                    self.perform(action);
                }
            }
            if is_input(ev) {
                return;
            }
        }

        if let Some(tray) = self.tray.as_mut() {
            match tray.handle(&self.x, ev) {
                Some(TrayEvent::Activate) => {
                    let fx = self.app.toggle_visibility();
                    self.apply(fx);
                }
                Some(TrayEvent::Menu { root_x, root_y }) => self.open_menu(root_x, root_y, true),
                None => {}
            }
        }

        match ev {
            Event::Expose(e) if e.window == self.overlay.id && e.count == 0 => self.dirty = true,
            Event::KeyPress(e) if e.event == self.x.root => {
                if let Some(action) = self.grabs.match_event(e) {
                    let fx = match action {
                        HotkeyAction::Toggle => self.app.toggle_timer(now),
                        HotkeyAction::Reset => self.app.reset_timer(now),
                        HotkeyAction::Visibility => self.app.toggle_visibility(),
                    };
                    self.apply(fx);
                }
            }
            Event::ConfigureNotify(e) if e.window == self.x.root => self.refresh_area_here(),
            Event::PropertyNotify(e)
                if e.window == self.x.root && e.atom == self.x.atoms._NET_WORKAREA =>
            {
                self.refresh_area_here();
            }
            _ => {}
        }

        if let Some(input) = translate(ev, self.overlay.id) {
            // No SetInputFocus on click: like the Python app, the overlay
            // never takes keyboard focus from the (fullscreen) app below.
            if let Input::Press {
                button: Button::Left,
                root_x,
                root_y,
                ..
            } = input
            {
                // A drag snaps to the monitor under the pointer.
                self.refresh_area(root_x, root_y);
            }
            let fx = self.app.handle(input, now);
            self.apply(fx);
        }
    }

    /// Re-reads the work area of the monitor at the point; repositions if it changed.
    fn refresh_area(&mut self, x: i32, y: i32) {
        let area = self.x.work_area_at(x, y);
        if area != self.area {
            self.area = area;
            self.app.set_area(area);
            self.sync_geometry();
            self.dirty = true;
        }
    }

    /// `refresh_area` at the overlay's center.
    fn refresh_area_here(&mut self) {
        let (x, y, w, h) = self.geom;
        self.refresh_area(x + w / 2, y + h / 2);
    }

    // ══ Effects ═════════════════════════════════════════════════════════════

    fn apply(&mut self, fx: Vec<Effect>) {
        for effect in fx {
            match effect {
                Effect::Beep(b) => self.sound.play(b, &self.app.cfg),
                Effect::SaveSoon => self.save_at = Some(self.now() + SAVE_DEBOUNCE),
                Effect::SaveNow => self.save(),
                Effect::Move { .. } => self.sync_geometry(),
                Effect::Resize | Effect::Redraw => self.dirty = true,
                Effect::SetClickThrough(on) => self.overlay.set_click_through(&self.x, on),
                Effect::SetVisible(v) => self.set_mapped(v),
                Effect::SetTrayVisible(v) => {
                    if !self.no_tray {
                        self.set_tray(v);
                    }
                }
                Effect::OpenMenu {
                    root_x,
                    root_y,
                    include_window_actions,
                } => self.open_menu(root_x, root_y, include_window_actions && !self.no_tray),
                Effect::OpenSettings { tab } => self.open_settings(tab),
                Effect::OpenSavePreset => {
                    if let Some(child) = spawn_self(&["save-preset"]) {
                        self.children.push(child);
                    }
                }
                Effect::Notify(text) => {
                    if let Ok(child) = Command::new("notify-send")
                        .args(["TimeHUD", &text])
                        .stdin(Stdio::null())
                        .spawn()
                    {
                        self.children.push(child);
                    }
                }
                Effect::Quit => self.quit = true,
            }
        }
        // An action may need an earlier tick (e.g. the timer just started).
        let now = self.now();
        let wall = chrono::Local::now().time();
        self.next_tick = self
            .next_tick
            .min(now + self.app.next_wakeup(now, wall_frac(&wall)));
        // ... or start an animation (e.g. the auto-fold begins in a tick).
        if let Some(t) = self.app.next_frame(now) {
            self.frame_due = Some(self.frame_due.map_or(t, |d| d.min(t)));
        }
    }

    fn perform(&mut self, action: MenuAction) {
        if matches!(action, MenuAction::SetPosition(_)) {
            // Presets place on the monitor under the cursor.
            let (px, py) = self.x.pointer();
            self.refresh_area(px, py);
        }
        let fx = self.app.perform(&action, self.now());
        self.apply(fx);
    }

    fn save(&mut self) {
        self.save_at = None;
        if let Err(e) = self.app.cfg.save() {
            eprintln!("timehud: cannot save config: {e}");
        }
    }

    /// Applies `window_pos()` and the last layout size, if they changed.
    fn sync_geometry(&mut self) {
        let (x, y) = self.app.window_pos();
        let geom = (x, y, self.size.0, self.size.1);
        if geom != self.geom {
            self.geom = geom;
            self.overlay.set_geometry(&self.x, x, y, geom.2, geom.3);
        }
    }

    /// Performs a pending keep-on-top raise unless rate-limited (then
    /// `timeout` wakes the loop when it is allowed).
    fn raise_if_allowed(&mut self, now: f64) {
        if !self.raise_pending || !self.mapped {
            self.raise_pending = false;
            return;
        }
        if self.raises.try_raise(now) {
            self.raise_pending = false;
            self.overlay.raise(&self.x);
            if let Some(p) = &self.popup {
                p.raise(&self.x);
            }
        }
    }

    fn set_mapped(&mut self, on: bool) {
        self.mapped = on;
        if on {
            self.overlay.map(&self.x);
            self.overlay.raise(&self.x);
            if let Some(p) = &self.popup {
                p.raise(&self.x);
            }
            self.dirty = true;
        } else {
            self.overlay.unmap(&self.x);
        }
    }

    fn set_tray(&mut self, on: bool) {
        match (&mut self.tray, on) {
            (Some(t), _) => t.set_visible(&self.x, on),
            (None, true) => match Tray::new(&self.x) {
                Ok(t) => self.tray = Some(t),
                Err(e) => eprintln!("timehud: cannot create tray icon: {e:#}"),
            },
            (None, false) => {}
        }
    }

    fn open_menu(&mut self, root_x: i32, root_y: i32, include_window_actions: bool) {
        if let Some(p) = self.popup.take() {
            p.close(&self.x);
        }
        let fonts = self.menu_fonts.get_or_insert_with(FontSet::bundled);
        let items = build_menu(&self.app.cfg, include_window_actions);
        let theme = get_theme(&self.app.cfg.theme);
        match MenuPopup::open(&self.x, items, root_x, root_y, theme, fonts) {
            Ok(p) => self.popup = Some(p),
            Err(e) => eprintln!("timehud: {e:#}"),
        }
    }

    fn open_settings(&mut self, tab: Option<String>) {
        if let Some(id) = self.settings.as_ref().map(|s| s.conn_id) {
            self.send_to(id, &IpcEvent::Focus { tab });
            return;
        }
        if self.settings_child.is_some() {
            return; // still starting up
        }
        let mut args = vec!["settings"];
        if let Some(t) = &tab {
            args.extend(["--tab", t]);
        }
        self.settings_child = spawn_self(&args);
        self.settings_tab = tab;
    }

    /// Re-grabs the global hotkeys when their setup changed (or `force`, e.g.
    /// after a keymap change); failures go to the settings window.
    fn sync_hotkeys(&mut self, force: bool) {
        let setup = hotkey_binds(&self.app.cfg);
        if !force && self.hotkeys.as_ref() == Some(&setup) {
            return;
        }
        self.hotkeys = Some(setup.clone());
        let (binds, mut errors) = setup;
        for e in &errors {
            eprintln!("timehud: {e}");
        }
        for (combo, reason) in self.grabs.set(&self.x, &binds) {
            errors.push(format!("Cannot grab {}: {reason}", format_combo(&combo)));
        }
        if let Some(id) = self.settings.as_ref().map(|s| s.conn_id) {
            for message in errors {
                self.send_to(id, &IpcEvent::Error { message });
            }
        }
    }

    // ══ Timers, rendering, waiting ══════════════════════════════════════════

    fn service(&mut self, now: f64) {
        if now >= self.next_tick {
            let wall = chrono::Local::now().time();
            let fx = self.app.tick(now, wall);
            self.next_tick = now + self.app.next_wakeup(now, wall_frac(&wall));
            self.apply(fx);
        }
        if self.save_at.is_some_and(|t| now >= t) {
            self.save();
        }
        if now >= self.next_idle {
            self.sound.idle_release();
            self.next_idle = now + IDLE_RELEASE;
        }
        self.raise_if_allowed(now);
        self.reap();
    }

    fn reap(&mut self) {
        if let Some(c) = &mut self.settings_child
            && !matches!(c.try_wait(), Ok(None))
        {
            self.settings_child = None;
        }
        self.children
            .retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }

    fn render(&mut self, now: f64) {
        if !self.mapped {
            return;
        }
        let due = self.frame_due.is_some_and(|t| now >= t - 0.001);
        if !self.dirty && !due {
            return;
        }
        let (pm, layout) = self.app.frame(now);
        self.size = (layout.w, layout.h);
        self.sync_geometry();
        if let Err(e) = self.x.present(self.overlay.id, self.overlay.gc, &pm) {
            eprintln!("timehud: present failed: {e:#}");
        }
        self.dirty = false;
        self.frame_due = self.app.next_frame(now);
    }

    fn timeout(&self, now: f64) -> u16 {
        let frame = self.frame_due.filter(|_| self.mapped);
        let raise = self
            .raises
            .next_allowed()
            .filter(|_| self.raise_pending && self.mapped);
        poll_timeout_ms(
            now,
            &[
                Some(self.next_tick),
                frame,
                self.save_at,
                Some(self.next_idle),
                raise,
            ],
        )
    }

    fn wait(&mut self, timeout_ms: u16) -> anyhow::Result<()> {
        let ready: Vec<bool> = {
            let in_ = PollFlags::POLLIN;
            // SAFETY: the X connection and every client outlive this poll call.
            let mut fds = vec![
                PollFd::new(unsafe { BorrowedFd::borrow_raw(self.x.fd()) }, in_),
                PollFd::new(self.signals.as_fd(), in_),
                PollFd::new(self.listener.as_fd(), in_),
            ];
            for c in &self.clients {
                fds.push(PollFd::new(
                    unsafe { BorrowedFd::borrow_raw(c.conn.fd()) },
                    in_,
                ));
            }
            match poll(&mut fds, timeout_ms) {
                Ok(_) => {}
                Err(Errno::EINTR) => return Ok(()),
                Err(e) => return Err(e.into()),
            }
            fds.iter()
                .map(|f| f.revents().is_some_and(|r| !r.is_empty()))
                .collect()
        };
        if ready[1] {
            let mut buf = [0u8; 64];
            while matches!((&self.signals).read(&mut buf), Ok(n) if n > 0) {}
            self.quit = true;
        }
        let ids: Vec<u64> = self.clients.iter().map(|c| c.id).collect();
        for (id, _) in ids.iter().zip(&ready[3..]).filter(|(_, r)| **r) {
            self.service_client(*id);
        }
        if ready[2] {
            self.accept();
        }
        Ok(())
    }

    // ══ IPC ═════════════════════════════════════════════════════════════════

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => match Conn::new(stream) {
                    Ok(conn) => {
                        self.next_client += 1;
                        self.clients.push(Client {
                            id: self.next_client,
                            conn,
                        });
                    }
                    Err(e) => eprintln!("timehud: ipc: {e}"),
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    eprintln!("timehud: ipc accept: {e}");
                    break;
                }
            }
        }
    }

    fn service_client(&mut self, id: u64) {
        let Some(client) = self.clients.iter_mut().find(|c| c.id == id) else {
            return;
        };
        match client.conn.read_requests() {
            Ok(reqs) => {
                for req in reqs {
                    if let Some(reply) = self.handle_request(id, req) {
                        self.send_to(id, &reply);
                    }
                }
            }
            Err(e) => {
                // EOF (incl. liveness probes that connect and close at once),
                // garbage, or a broken socket: drop the client.
                if e.kind() == io::ErrorKind::InvalidData {
                    let message = format!("bad request: {e}");
                    self.send_to(id, &IpcEvent::Error { message });
                }
                self.clients.retain(|c| c.id != id);
                if self.settings.as_ref().is_some_and(|s| s.conn_id == id) {
                    // Closing the settings window = Cancel.
                    self.settings_cancel();
                    self.settings = None;
                }
            }
        }
    }

    fn send_to(&mut self, id: u64, ev: &IpcEvent) {
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == id) {
            // A failed write shows up as EOF on the next read.
            let _ = c.conn.send(ev);
        }
    }

    fn handle_request(&mut self, id: u64, req: Request) -> Option<IpcEvent> {
        let now = self.now();
        let is_settings = self.settings.as_ref().is_some_and(|s| s.conn_id == id);
        let settings_only = matches!(
            req,
            Request::Preview { .. } | Request::Commit { .. } | Request::Cancel
        );
        if settings_only && !is_settings {
            return Some(IpcEvent::Error {
                message: "not the settings connection (send settings-hello first)".into(),
            });
        }
        let fx = match req {
            Request::Toggle => self.app.toggle_timer(now),
            Request::Reset => self.app.reset_timer(now),
            Request::Show => self.app.set_visible(true),
            Request::Hide => self.app.set_visible(false),
            Request::ToggleVisibility => self.app.toggle_visibility(),
            Request::Quit => vec![Effect::Quit],
            Request::SavePreset { name } => self.app.save_preset(&name),
            Request::SettingsHello => {
                if let Some(other) = self.settings.as_ref().map(|s| s.conn_id)
                    && other != id
                {
                    self.send_to(other, &IpcEvent::Focus { tab: None });
                    return Some(IpcEvent::Error {
                        message: "settings are already open".into(),
                    });
                }
                let config = self.app.cfg.to_value();
                self.settings = Some(SettingsSession {
                    conn_id: id,
                    prev: config.clone(),
                    snapshot: Some(self.app.cfg.clone()),
                });
                return Some(IpcEvent::Config {
                    config,
                    tab: self.settings_tab.take(),
                });
            }
            Request::Preview { config } => {
                self.settings_apply(config, false);
                return None;
            }
            Request::Commit { config } => {
                self.settings_apply(config, true);
                return None;
            }
            Request::Cancel => {
                self.settings_cancel();
                return None;
            }
        };
        self.apply(fx);
        Some(IpcEvent::Ok)
    }

    /// Preview/Commit from the settings process: merge only what it changed.
    fn settings_apply(&mut self, config: Value, commit: bool) {
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        let merged = merge_settings_changes(&self.app.cfg.to_value(), &s.prev, &config);
        s.prev = config;
        if commit {
            s.snapshot = None;
        } else if s.snapshot.is_none() {
            // First change after an Apply: that state is the new baseline.
            s.snapshot = Some(self.app.cfg.clone());
        }
        let mut fx = self
            .app
            .preview_config(Config::from_value(&merged), self.now());
        if commit {
            fx.push(Effect::SaveNow);
        }
        self.apply(fx);
        self.sync_hotkeys(false);
    }

    fn settings_cancel(&mut self) {
        let Some(snapshot) = self.settings.as_mut().and_then(|s| s.snapshot.take()) else {
            return;
        };
        let mut fx = self.app.preview_config(snapshot, self.now());
        fx.push(Effect::SaveNow);
        self.apply(fx);
        self.sync_hotkeys(false);
    }

    // ══ Shutdown ════════════════════════════════════════════════════════════

    fn shutdown(&mut self) {
        if let Some(p) = self.popup.take() {
            p.close(&self.x);
        }
        self.save();
        self.grabs.set(&self.x, &[]);
        self.set_tray(false);
        self.overlay.unmap(&self.x);
        let _ = self.x.conn.flush();
    }
}

/// Spawns this executable with `args` (settings / save-preset windows).
fn spawn_self(args: &[&str]) -> Option<Child> {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("timehud: cannot find own executable: {e}");
            return None;
        }
    };
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .spawn()
        .inspect_err(|e| eprintln!("timehud: cannot start {}: {e}", args[0]))
        .ok()
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
    fn raise_limiter_allows_four_per_second_then_defers() {
        let mut lim = RaiseLimiter::default();
        assert_eq!(lim.next_allowed(), None);
        for t in [10.0, 10.1, 10.2, 10.3] {
            assert!(lim.try_raise(t), "raise at {t}");
        }
        assert!(!lim.try_raise(10.4));
        assert!(!lim.try_raise(10.99));
        // The oldest raise (10.0) leaves the 1 s window at 11.0.
        assert_eq!(lim.next_allowed(), Some(11.0));
        assert!(lim.try_raise(11.0));
        assert!(!lim.try_raise(11.05));
        assert_eq!(lim.next_allowed(), Some(11.1));
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

    #[test]
    fn hotkey_binds_from_config() {
        let cfg = Config {
            hotkey_toggle: "Ctrl+Shift+Space".into(),
            hotkey_reset: String::new(),
            hotkey_visibility: "Ctrl+Bogus".into(),
            ..Config::default()
        };
        let (binds, errors) = hotkey_binds(&cfg);
        assert_eq!(
            binds,
            vec![(
                parse_combo("Ctrl+Shift+Space").unwrap().unwrap(),
                HotkeyAction::Toggle
            )]
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Ctrl+Bogus"), "{}", errors[0]);
    }

    #[test]
    fn hotkey_binds_default_is_empty() {
        assert_eq!(hotkey_binds(&Config::default()), (Vec::new(), Vec::new()));
    }

    #[test]
    fn poll_timeout_picks_earliest_and_rounds_up() {
        assert_eq!(
            poll_timeout_ms(10.0, &[Some(10.5), None, Some(10.0161)]),
            17
        );
        assert_eq!(poll_timeout_ms(10.0, &[Some(9.0)]), 0);
        assert_eq!(poll_timeout_ms(10.0, &[None]), 1000);
        assert_eq!(poll_timeout_ms(10.0, &[Some(99.0)]), 1000);
    }
}
