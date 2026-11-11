//! HUD process: the event loop tying the controller to X11, IPC and sound.
//!
//! One thread, one `poll(2)` over the X connection, the signal pipe, the IPC
//! listener and its clients. Each iteration drains X events, runs the timer
//! tick when due, applies the controller's `Effect`s and presents a frame
//! when something changed or an animation runs.

mod deadlines;
mod ipc_server;

use crate::app::{App, Button, Effect, Input, resolved_family};
use crate::config::Config;
use crate::hotkey::{Combo, format_combo, parse_combo};
use crate::ipc::Event as IpcEvent;
use crate::layout::Rect;
use crate::menu::{MenuAction, build_menu};
use crate::sound::Sound;
use crate::text::FontSet;
use crate::themes::get_theme;
use crate::x11::grabs::{Grabs, HotkeyAction};
use crate::x11::overlay::{OverlayWin, StackWatch, covered_now, restack_needed};
use crate::x11::popup::{MenuPopup, PopupOutcome};
use crate::x11::tray::{Tray, TrayEvent};
use crate::x11::{Canvas, Screens, X};
use chrono::Timelike;
use deadlines::Deadlines;
use ipc_server::{Client, SettingsSession};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use std::collections::VecDeque;
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::Event;

static START: OnceLock<Instant> = OnceLock::new();

/// Records the process start for the `TIMEHUD_TRACE=1` startup timing.
/// Call first thing in `main`.
pub fn mark_start() {
    let _ = START.set(Instant::now());
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
    /// Monitors and work area, refreshed when the root reports changes.
    screens: Screens,
    /// A root event may have changed `screens` / the DPI scale: re-read
    /// once after the current batch of events.
    screens_stale: bool,
    scale_stale: bool,
    /// The work area the app positions in.
    area: Rect,
    /// Geometry last sent to the X server: x, y, w, h.
    geom: (i32, i32, i32, i32),
    /// Window size of the last rendered layout.
    size: (i32, i32),
    /// The overlay's frame buffer (reused pixmap, last uploaded pixels).
    canvas: Canvas,
    mapped: bool,
    dirty: bool,
    /// When the loop must wake up next, per reason.
    deadlines: Deadlines,
    /// Another window went above ours; raise when `raises` allows.
    raise_pending: bool,
    /// Restacks of other windows, and whether one in the current batch of
    /// events needs the stacking order checked (`covered_now`).
    stack_watch: StackWatch,
    stack_check: bool,
    raises: RaiseLimiter,
    /// The hotkey setup last applied (binds and parse errors).
    hotkeys: Option<HotkeySetup>,
    /// Problems from the last hotkey (re)grab, for the settings window.
    hotkey_errors: Vec<String>,
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
        let screens = x.screens();
        // A HUD saved on another monitor starts there, not under the pointer.
        let custom = (cfg.custom_x >= 0 && cfg.custom_y >= 0).then(|| {
            let c = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            (c(cfg.custom_x), c(cfg.custom_y))
        });
        let area = screens.start_area(custom, || x.pointer());
        let fonts = FontSet::load(&resolved_family(&cfg));
        let clock = Box::new(move || t0.elapsed().as_secs_f64());
        let now = t0.elapsed().as_secs_f64();
        let mut app = App::new(cfg, clock, fonts, scale, area, now);
        let mut canvas = Canvas::default();
        let layout = app.frame_into(now, &mut canvas.frame);
        let (wx, wy) = app.window_pos();
        let overlay = OverlayWin::create(&x, layout.w, layout.h, wx, wy)?;
        if app.cfg.click_through {
            overlay.set_click_through(&x, true);
        }
        overlay.map(&x);
        x.present_canvas(overlay.id, overlay.gc, &mut canvas)?;
        if trace_enabled()
            && let Some(start) = START.get()
        {
            eprintln!(
                "timehud: first frame presented {:.1} ms after start",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }

        x.watch_screens()?;
        listener.set_nonblocking(true)?;
        let (signals, sig_writer) = UnixStream::pair()?;
        signals.set_nonblocking(true)?;
        sig_writer.set_nonblocking(true)?;
        // HUP: the terminal closed; QUIT: Ctrl+\. All flush a pending save.
        use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
        for sig in [SIGINT, SIGTERM, SIGHUP, SIGQUIT] {
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
            screens,
            screens_stale: false,
            scale_stale: false,
            area,
            geom: (wx, wy, layout.w, layout.h),
            size: (layout.w, layout.h),
            canvas,
            mapped: true,
            dirty: false,
            deadlines: Deadlines::new(now),
            raise_pending: false,
            stack_watch: StackWatch::default(),
            stack_check: false,
            raises: RaiseLimiter::default(),
            hotkeys: None,
            hotkey_errors: Vec::new(),
            backlog: None,
            quit: false,
        };
        if hud.app.cfg.show_tray_icon && !no_tray {
            hud.set_tray(true);
        }
        hud.deadlines.frame = hud.app.next_frame(hud.now());
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
            // e.g. the X server went away: keep the user's settings, and
            // forget an open menu (its grabs died with the connection).
            if let Some(p) = self.popup.take() {
                p.abandon();
            }
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
        self.refresh_screens();
        self.check_stacking();
        Ok(())
    }

    /// After a restack that might have covered the overlay indirectly:
    /// raise if a managed window is above it now.
    fn check_stacking(&mut self) {
        if !std::mem::take(&mut self.stack_check) || !self.mapped || self.raise_pending {
            return;
        }
        let mut own = vec![self.overlay.id];
        if let Some(p) = &self.popup {
            own.extend(p.windows());
        }
        if covered_now(&self.x, self.overlay.id, &own) {
            self.raise_pending = true;
            self.raise_if_allowed(self.now());
        }
    }

    /// Re-reads what root events reported as changed: monitors / work area
    /// (then re-places the HUD) and the DPI scale (then re-lays it out).
    fn refresh_screens(&mut self) {
        if std::mem::take(&mut self.screens_stale) {
            let screens = self.x.screens();
            if screens != self.screens {
                self.screens = screens;
                self.refresh_area_here();
            }
        }
        if std::mem::take(&mut self.scale_stale) && self.x.reload_scale() {
            let fx = self.app.set_scale(self.x.scale(), self.now());
            self.apply(fx);
        }
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
        } else if self.stack_watch.restacked(ev, self.x.root, &own) {
            self.stack_check |= self.mapped;
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
            Event::Expose(e) if e.window == self.overlay.id && e.count == 0 => {
                self.canvas.invalidate();
                self.dirty = true;
            }
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
            _ if self.x.changes_screens(ev) => self.screens_stale = true,
            _ if self.x.changes_scale(ev) => self.scale_stale = true,
            _ => {}
        }

        if let Some(input) = self.overlay.translate(ev, |k| self.x.keysym(k)) {
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

    /// Switches to the work area of the monitor at the point (from the
    /// cached `screens`, no round trip); repositions if it changed.
    fn refresh_area(&mut self, x: i32, y: i32) {
        let area = self.screens.work_area_at(x, y);
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
        // Config changes come with a save: tell an open settings window.
        let config_changed = fx
            .iter()
            .any(|e| matches!(e, Effect::SaveSoon | Effect::SaveNow));
        for effect in fx {
            match effect {
                Effect::Beep(b) => self.sound.play(b, &self.app.cfg),
                Effect::SaveSoon => self.deadlines.save_soon(self.now()),
                Effect::SaveNow => self.save(),
                Effect::Move { .. } => self.sync_geometry(),
                Effect::Resize | Effect::Redraw => self.dirty = true,
                Effect::SetClickThrough(on) => self.overlay.set_click_through(&self.x, on),
                Effect::SetCursor(c) => self.overlay.set_cursor(&self.x, c),
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
        if config_changed {
            self.push_config_to_settings();
        }
        // An action may need an earlier tick (e.g. the timer just started).
        let now = self.now();
        let wall = chrono::Local::now().time();
        self.deadlines
            .tick_by(now + self.app.next_wakeup(now, wall_frac(&wall)));
        // ... or start an animation (e.g. the auto-fold begins in a tick).
        if let Some(t) = self.app.next_frame(now) {
            self.deadlines.frame_by(t);
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
        self.deadlines.save = None;
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
        } else if self.raises.try_raise(now) {
            self.raise_pending = false;
            self.overlay.raise(&self.x);
            if let Some(p) = &self.popup {
                p.raise(&self.x);
            }
        }
        self.deadlines.raise = self.raises.next_allowed().filter(|_| self.raise_pending);
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
        let area = self.screens.work_area_at(root_x, root_y);
        match MenuPopup::open(&self.x, items, root_x, root_y, area, theme, fonts) {
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
        self.hotkey_errors = errors;
        if let Some(id) = self.settings.as_ref().map(|s| s.conn_id) {
            let errors = self.hotkey_errors.clone();
            self.send_to(id, &IpcEvent::HotkeyStatus { errors });
        }
    }

    // ══ Timers, rendering, waiting ══════════════════════════════════════════

    fn service(&mut self, now: f64) {
        if now >= self.deadlines.tick {
            let wall = chrono::Local::now().time();
            let fx = self.app.tick(now, wall);
            self.deadlines.tick = now + self.app.next_wakeup(now, wall_frac(&wall));
            self.apply(fx);
        }
        if self.deadlines.save_due(now) {
            self.save();
        }
        if self.deadlines.idle_due(now) {
            self.sound.idle_release();
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
        let due = self.deadlines.frame_due(now);
        if !self.dirty && !due {
            return;
        }
        let layout = self.app.frame_into(now, &mut self.canvas.frame);
        self.size = (layout.w, layout.h);
        self.sync_geometry();
        let (win, gc) = (self.overlay.id, self.overlay.gc);
        if let Err(e) = self.x.present_canvas(win, gc, &mut self.canvas) {
            eprintln!("timehud: present failed: {e:#}");
        }
        self.dirty = false;
        self.deadlines.frame = self.app.next_frame(now);
    }

    fn timeout(&self, now: f64) -> u16 {
        self.deadlines.timeout_ms(now, self.mapped)
    }

    fn wait(&mut self, timeout_ms: u16) -> anyhow::Result<()> {
        let listening = self.deadlines.accepting(self.now());
        // The X connection is drained at the top of the next iteration.
        let (sig_ready, listen_ready, client_ready) = {
            let in_ = PollFlags::POLLIN;
            // SAFETY: the X connection and every client outlive this poll call.
            let mut fds = vec![
                PollFd::new(unsafe { BorrowedFd::borrow_raw(self.x.fd()) }, in_),
                PollFd::new(self.signals.as_fd(), in_),
                // Paused: no events requested (errors would still show).
                PollFd::new(
                    self.listener.as_fd(),
                    if listening { in_ } else { PollFlags::empty() },
                ),
            ];
            for c in &self.clients {
                // POLLOUT only while output is queued: an idle socket is
                // always writable and would spin the loop.
                let flags = if c.conn.wants_write() {
                    in_ | PollFlags::POLLOUT
                } else {
                    in_
                };
                fds.push(PollFd::new(
                    unsafe { BorrowedFd::borrow_raw(c.conn.fd()) },
                    flags,
                ));
            }
            match poll(&mut fds, timeout_ms) {
                Ok(_) => {}
                Err(Errno::EINTR) => return Ok(()),
                Err(e) => return Err(e.into()),
            }
            let revents = |f: &PollFd| f.revents().unwrap_or(PollFlags::empty());
            let readable = |f: &PollFd| !(revents(f) - PollFlags::POLLOUT).is_empty();
            // Per client: (readable or hung up, writable).
            let clients: Vec<(bool, bool)> = fds[3..]
                .iter()
                .map(|f| (readable(f), revents(f).contains(PollFlags::POLLOUT)))
                .collect();
            (readable(&fds[1]), readable(&fds[2]), clients)
        };
        if sig_ready {
            let mut buf = [0u8; 64];
            while matches!((&self.signals).read(&mut buf), Ok(n) if n > 0) {}
            self.quit = true;
        }
        let ids: Vec<u64> = self.clients.iter().map(|c| c.id).collect();
        for (&id, &(readable, writable)) in ids.iter().zip(&client_ready) {
            if writable {
                self.flush_client(id);
            }
            if readable {
                self.service_client(id);
            }
        }
        if listening && listen_ready {
            self.accept();
        }
        Ok(())
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
}
