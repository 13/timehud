//! The HUD's IPC server: accepting clients, their requests and queued
//! events, and the settings session.

use super::Hud;
use super::deadlines::ACCEPT_BACKOFF;
use crate::app::Effect;
use crate::config::Config;
use crate::ipc::{Conn, Event as IpcEvent, Request};
use crate::settings_session::Session;
use serde_json::Value;
use std::io;

pub(super) struct Client {
    pub(super) id: u64,
    pub(super) conn: Conn,
}

/// The connected settings process.
pub(super) struct SettingsSession {
    pub(super) conn_id: u64,
    pub(super) session: Session,
}

const NOT_SETTINGS: &str = "not the settings connection (send settings-hello first)";
const ALREADY_OPEN: &str = "settings are already open";

/// What happens to a request before dispatch.
#[derive(Debug, PartialEq)]
enum Gate {
    Pass,
    /// A settings-only request from another connection.
    NotSettings,
    /// A second settings window said hello; `other` (the open one) is
    /// raised instead.
    AlreadyOpen {
        other: u64,
    },
}

/// Checks a request from connection `id` against the settings session
/// (held by connection `settings`, if any).
fn gate(req: &Request, settings: Option<u64>, id: u64) -> Gate {
    match (req, settings) {
        (Request::Preview { .. } | Request::Commit { .. } | Request::Cancel, s)
            if s != Some(id) =>
        {
            Gate::NotSettings
        }
        (Request::SettingsHello, Some(other)) if other != id => Gate::AlreadyOpen { other },
        _ => Gate::Pass,
    }
}

impl Hud {
    pub(super) fn accept(&mut self) {
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
                    eprintln!("timehud: ipc accept: {e} (pausing for {ACCEPT_BACKOFF} s)");
                    self.deadlines.pause_accept(self.now());
                    break;
                }
            }
        }
    }

    pub(super) fn service_client(&mut self, id: u64) {
        let Some(client) = self.clients.iter_mut().find(|c| c.id == id) else {
            return;
        };
        match client.conn.read_requests() {
            Ok(lines) => {
                for line in lines {
                    // A bad line (e.g. a request this HUD does not know) is
                    // answered, not fatal: for the settings connection,
                    // dropping it would mean an implicit Cancel.
                    let reply = match line {
                        Ok(req) => self.handle_request(id, req),
                        Err(e) => Some(IpcEvent::Error {
                            message: format!("bad request: {e}"),
                        }),
                    };
                    if let Some(reply) = reply {
                        self.send_to(id, &reply);
                    }
                }
            }
            Err(e) => {
                // EOF (incl. liveness probes that connect and close at once),
                // an overlong line, or a broken socket: drop the client.
                if e.kind() == io::ErrorKind::InvalidData {
                    let message = format!("bad request: {e}");
                    self.send_to(id, &IpcEvent::Error { message });
                }
                self.drop_client(id);
            }
        }
    }

    /// Writes a client's queued events; drops it if the socket broke.
    pub(super) fn flush_client(&mut self, id: u64) {
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == id)
            && c.conn.flush().is_err()
        {
            self.drop_client(id);
        }
    }

    fn drop_client(&mut self, id: u64) {
        let before = self.clients.len();
        self.clients.retain(|c| c.id != id);
        if self.clients.len() != before && self.settings.as_ref().is_some_and(|s| s.conn_id == id) {
            // Closing the settings window = Cancel.
            self.settings_cancel();
            self.settings = None;
        }
    }

    /// Queues an event for a client (never blocks); a client whose socket
    /// broke or that stopped reading is dropped.
    pub(super) fn send_to(&mut self, id: u64, ev: &IpcEvent) {
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == id)
            && let Err(e) = c.conn.send(ev)
        {
            if e.kind() == io::ErrorKind::WouldBlock {
                eprintln!("timehud: ipc: dropping client: {e}");
            }
            self.drop_client(id);
        }
    }

    fn handle_request(&mut self, id: u64, req: Request) -> Option<IpcEvent> {
        let now = self.now();
        match gate(&req, self.settings.as_ref().map(|s| s.conn_id), id) {
            Gate::Pass => {}
            Gate::NotSettings => {
                return Some(IpcEvent::Error {
                    message: NOT_SETTINGS.into(),
                });
            }
            Gate::AlreadyOpen { other } => {
                self.send_to(other, &IpcEvent::Focus { tab: None });
                return Some(IpcEvent::Error {
                    message: ALREADY_OPEN.into(),
                });
            }
        }
        let fx = match req {
            Request::Status => {
                return Some(IpcEvent::Status {
                    status: self.app.status(),
                });
            }
            Request::SettingsHello => {
                let config = self.app.cfg.to_value();
                self.settings = Some(SettingsSession {
                    conn_id: id,
                    session: Session::new(&config),
                });
                let tab = self.settings_tab.take();
                self.send_to(
                    id,
                    &IpcEvent::Config {
                        config,
                        tab,
                        rev: 0,
                    },
                );
                return Some(IpcEvent::HotkeyStatus {
                    errors: self.hotkey_errors.clone(),
                });
            }
            Request::Preview { config, rev } => {
                self.settings_apply(config, rev, false);
                return None;
            }
            Request::Commit { config, rev } => {
                self.settings_apply(config, rev, true);
                return None;
            }
            Request::Cancel => {
                self.settings_cancel();
                return None;
            }
            control => self.app.request(control, now),
        };
        match fx {
            Ok(fx) => {
                self.apply(fx);
                Some(IpcEvent::Ok)
            }
            Err(message) => Some(IpcEvent::Error { message }),
        }
    }

    /// Preview/Commit from the settings process: merge only what it changed.
    fn settings_apply(&mut self, config: Value, seen_rev: u64, commit: bool) {
        let current = self.app.cfg.to_value();
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        let merged = s.session.apply(&current, config, seen_rev, commit);
        self.adopt_config(&merged, commit);
    }

    /// Cancel / window closed: restore what settings changed, nothing else.
    fn settings_cancel(&mut self) {
        let current = self.app.cfg.to_value();
        let Some(restored) = self
            .settings
            .as_mut()
            .and_then(|s| s.session.cancel(&current))
        else {
            return;
        };
        // Saved even though Cancel changes nothing on disk by itself:
        // HUD-side saves may have written previewed values meanwhile.
        self.adopt_config(&restored, true);
    }

    /// Makes `value` (a settings merge or restore) the live config, saving
    /// it at once with `save_now`, and pushes any adjustment the HUD made
    /// (e.g. clearing the preset) back to the settings window.
    fn adopt_config(&mut self, value: &Value, save_now: bool) {
        let mut fx = self
            .app
            .preview_config(Config::from_value(value), self.now());
        if save_now {
            fx.push(Effect::SaveNow);
        }
        // With a save effect `apply` pushes to settings itself.
        self.apply(fx);
        if !save_now {
            self.push_config_to_settings();
        }
        self.sync_hotkeys(false);
        self.sound.config_changed(&self.app.cfg);
    }

    /// Hands HUD-side config changes to an open settings window, so its
    /// later messages do not carry stale values (see `settings_session`).
    pub(super) fn push_config_to_settings(&mut self) {
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        let id = s.conn_id;
        if let Some((config, rev)) = s.session.hud_changed(&self.app.cfg.to_value()) {
            let push = IpcEvent::Config {
                config,
                tab: None,
                rev,
            };
            self.send_to(id, &push);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn preview() -> Request {
        Request::Preview {
            config: json!({}),
            rev: 0,
        }
    }

    #[test]
    fn settings_only_requests_need_the_settings_connection() {
        let commit = Request::Commit {
            config: json!({}),
            rev: 0,
        };
        for req in [preview(), commit, Request::Cancel] {
            assert_eq!(gate(&req, None, 1), Gate::NotSettings, "{req:?}");
            assert_eq!(gate(&req, Some(2), 1), Gate::NotSettings, "{req:?}");
            assert_eq!(gate(&req, Some(1), 1), Gate::Pass, "{req:?}");
        }
    }

    #[test]
    fn a_second_settings_hello_raises_the_open_window() {
        assert_eq!(gate(&Request::SettingsHello, None, 1), Gate::Pass);
        // The same connection saying hello again restarts its session.
        assert_eq!(gate(&Request::SettingsHello, Some(1), 1), Gate::Pass);
        assert_eq!(
            gate(&Request::SettingsHello, Some(1), 2),
            Gate::AlreadyOpen { other: 1 }
        );
    }

    #[test]
    fn control_requests_pass_from_anyone() {
        for req in [Request::Toggle, Request::Status, Request::Quit] {
            assert_eq!(gate(&req, Some(1), 2), Gate::Pass, "{req:?}");
            assert_eq!(gate(&req, None, 2), Gate::Pass, "{req:?}");
        }
    }
}
