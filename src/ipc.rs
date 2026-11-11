//! Single-instance IPC: newline-delimited JSON over a Unix socket.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `$XDG_RUNTIME_DIR/timehud.sock`, else `/tmp/timehud-<uid>.sock`.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("timehud.sock"),
        _ => {
            use std::os::unix::fs::MetadataExt;
            let uid = std::fs::metadata("/proc/self")
                .map(|m| m.uid())
                .unwrap_or(0);
            PathBuf::from(format!("/tmp/timehud-{uid}.sock"))
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    Toggle,
    Reset,
    Show,
    Hide,
    ToggleVisibility,
    Quit,
    SettingsHello,
    Preview { config: Value },
    Commit { config: Value },
    Cancel,
    SavePreset { name: String },
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    Ok,
    Error { message: String },
    Config { config: Value, tab: Option<String> },
    Focus { tab: Option<String> },
}

pub enum Bind {
    Server(UnixListener),
    AlreadyRunning,
}

/// Connect first: a live listener means another instance runs. A refused or
/// missing socket is stale; remove it and bind (mode 0600).
pub fn bind_or_detect(path: &Path) -> io::Result<Bind> {
    use io::ErrorKind::*;
    match UnixStream::connect(path) {
        Ok(_) => return Ok(Bind::AlreadyRunning),
        // Not a socket / nobody listening / nothing there: stale.
        Err(e) if matches!(e.kind(), ConnectionRefused | NotFound) => {}
        Err(e) => return Err(e),
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == NotFound => {}
        Err(e) => return Err(e),
    }
    match UnixListener::bind(path) {
        Ok(l) => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Bind::Server(l))
        }
        // Lost a race with another starter that bound first.
        Err(e) if e.kind() == AddrInUse => Ok(Bind::AlreadyRunning),
        Err(e) => Err(e),
    }
}

/// Send one request and read one line reply (2 s timeout).
pub fn send(path: &Path, req: &Request) -> io::Result<Event> {
    use std::io::{BufRead, BufReader, Write};
    let timeout = Some(Duration::from_secs(2));
    let mut s = UnixStream::connect(path)?;
    s.set_read_timeout(timeout)?;
    s.set_write_timeout(timeout)?;
    let mut line = serde_json::to_vec(req).map_err(io::Error::other)?;
    line.push(b'\n');
    s.write_all(&line)?;
    let mut reply = String::new();
    if BufReader::new(s).read_line(&mut reply)? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    serde_json::from_str(reply.trim_end())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Nonblocking server-side connection with a line buffer.
pub struct Conn {
    stream: UnixStream,
    buf: Vec<u8>,
    eof: bool,
}

impl Conn {
    pub fn new(stream: UnixStream) -> io::Result<Conn> {
        stream.set_nonblocking(true)?;
        Ok(Conn {
            stream,
            buf: Vec::new(),
            eof: false,
        })
    }

    /// For poll(2) by the HUD loop.
    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    /// Parse every complete line available now. Returns `Err(UnexpectedEof)`
    /// once the peer has closed and all buffered requests were delivered.
    pub fn read_requests(&mut self) -> io::Result<Vec<Request>> {
        use std::io::Read;
        let mut chunk = [0u8; 4096];
        while !self.eof {
            match self.stream.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = &line[..line.len() - 1];
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            out.push(
                serde_json::from_slice(line)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            );
        }
        if out.is_empty() && self.eof {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(out)
    }

    /// Write one event line; tolerates a full socket buffer for up to 2 s.
    pub fn send(&mut self, ev: &Event) -> io::Result<()> {
        use std::io::Write;
        let mut line = serde_json::to_vec(ev).map_err(io::Error::other)?;
        line.push(b'\n');
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut rest = &line[..];
        while !rest.is_empty() {
            match self.stream.write(rest) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    fn sock(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("t.sock")
    }

    #[test]
    fn request_json_shape() {
        let r = Request::Preview {
            config: serde_json::json!({"a": 1}),
        };
        let v: Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["cmd"], "preview");
        assert_eq!(v["config"]["a"], 1);
        assert_eq!(
            serde_json::to_string(&Request::ToggleVisibility).unwrap(),
            r#"{"cmd":"toggle-visibility"}"#
        );
        let back: Request = serde_json::from_str(r#"{"cmd":"save-preset","name":"x"}"#).unwrap();
        assert_eq!(back, Request::SavePreset { name: "x".into() });
    }

    #[test]
    fn event_json_shape() {
        let v = serde_json::to_value(Event::Focus { tab: None }).unwrap();
        assert_eq!(v["event"], "focus");
    }

    #[test]
    fn stale_file_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        std::fs::write(&p, b"junk").unwrap();
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(_)));
    }

    #[test]
    fn stale_socket_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        drop(UnixListener::bind(&p).unwrap()); // leaves the file behind
        assert!(p.exists());
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(_)));
    }

    #[test]
    fn missing_path_binds_with_private_mode() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(_)));
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn live_socket_detected() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        let _l = UnixListener::bind(&p).unwrap();
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::AlreadyRunning));
        assert!(p.exists(), "live socket must not be removed");
    }

    #[test]
    fn send_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        let l = UnixListener::bind(&p).unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut line = Vec::new();
            let mut b = [0u8; 1];
            while s.read(&mut b).unwrap() == 1 && b[0] != b'\n' {
                line.push(b[0]);
            }
            assert_eq!(line, br#"{"cmd":"toggle"}"#);
            s.write_all(b"{\"event\":\"ok\"}\n").unwrap();
        });
        assert_eq!(send(&p, &Request::Toggle).unwrap(), Event::Ok);
        t.join().unwrap();
    }

    #[test]
    fn send_without_server_errors() {
        let d = tempfile::tempdir().unwrap();
        assert!(send(&sock(&d), &Request::Toggle).is_err());
    }

    #[test]
    fn conn_splits_lines() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        b.write_all(b"{\"cmd\":\"show\"}\n{\"cmd\":\"hide\"}\n{\"cmd\":\"qu")
            .unwrap();
        assert_eq!(
            c.read_requests().unwrap(),
            vec![Request::Show, Request::Hide]
        );
        b.write_all(b"it\"}\n").unwrap();
        assert_eq!(c.read_requests().unwrap(), vec![Request::Quit]);
    }

    #[test]
    fn conn_is_nonblocking_and_reports_eof() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        assert!(c.read_requests().unwrap().is_empty()); // would block otherwise
        drop(b);
        let e = c.read_requests().unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn conn_send_writes_a_line() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        c.send(&Event::Ok).unwrap();
        let mut buf = [0u8; 64];
        let n = b.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"{\"event\":\"ok\"}\n");
    }
}
