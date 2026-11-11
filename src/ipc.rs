//! Single-instance IPC: newline-delimited JSON over a Unix socket.

use crate::app::Status;
use crate::config::Mode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `$XDG_RUNTIME_DIR/timehud.sock`, else `/tmp/timehud-<uid>/timehud.sock`
/// in a private directory (created on demand; see [`private_dir`]).
pub fn socket_path() -> io::Result<PathBuf> {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => Ok(PathBuf::from(d).join("timehud.sock")),
        _ => {
            let uid = nix::unistd::getuid().as_raw();
            let dir = PathBuf::from(format!("/tmp/timehud-{uid}"));
            private_dir(&dir, uid)?;
            Ok(dir.join("timehud.sock"))
        }
    }
}

/// Makes sure `dir` is a real directory (not a symlink) owned by `uid` and
/// accessible to nobody else, creating it with mode 0700 if missing. In a
/// shared directory like `/tmp` another user could otherwise pre-create it
/// and impersonate or hijack the HUD's socket.
pub fn private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    let unsafe_dir = |why: &str| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: {why}", dir.display()),
        )
    };
    if !meta.file_type().is_dir() {
        return Err(unsafe_dir("not a directory"));
    }
    if meta.uid() != uid {
        return Err(unsafe_dir("owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    Toggle,
    Start,
    Pause,
    Reset,
    /// Countdown of this many seconds (`timehud set`).
    SetCountdown {
        seconds: i64,
    },
    /// Apply the preset of this name (`timehud preset`).
    ApplyPreset {
        name: String,
    },
    SetMode {
        mode: Mode,
    },
    /// Answered with [`Event::Status`].
    Status,
    Show,
    Hide,
    ToggleVisibility,
    Quit,
    SettingsHello,
    Preview {
        config: Value,
    },
    Commit {
        config: Value,
    },
    Cancel,
    SavePreset {
        name: String,
    },
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    Ok,
    Error {
        message: String,
    },
    /// The global hotkey problems after the last (re)grab; empty = all fine.
    /// Sent to the settings window, which shows them on its Hotkeys tab.
    HotkeyStatus {
        errors: Vec<String>,
    },
    Config {
        config: Value,
        tab: Option<String>,
    },
    Focus {
        tab: Option<String>,
    },
    Status {
        status: Status,
    },
}

pub enum Bind {
    /// We are the instance: the listener, and the lock file that must stay
    /// open (locked) for as long as the process serves the socket.
    Server(UnixListener, Lock),
    AlreadyRunning,
}

/// The held single-instance lock (unlocked on drop or process exit).
pub type Lock = nix::fcntl::Flock<std::fs::File>;

/// `<socket>.lock`, the single-instance lock next to the socket.
fn lock_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".lock");
    PathBuf::from(p)
}

/// Takes the single-instance lock without blocking: an exclusive `flock` on
/// `<socket>.lock`, released by the kernel when the process exits. `None`
/// while another process holds it.
fn try_lock(path: &Path) -> io::Result<Option<Lock>> {
    use nix::fcntl::{Flock, FlockArg};
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path(path))?;
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(Some(lock)),
        Err((_, nix::errno::Errno::EWOULDBLOCK)) => Ok(None),
        Err((_, e)) => Err(e.into()),
    }
}

/// Single instance: whoever holds the `<socket>.lock` flock owns the socket,
/// so two simultaneous starters cannot both decide the socket is stale (the
/// loser would delete the winner's freshly bound socket). With the lock, a
/// live listener still means another instance runs (one predating the lock);
/// a refused or missing socket is stale: remove it and bind (mode 0600).
pub fn bind_or_detect(path: &Path) -> io::Result<Bind> {
    use io::ErrorKind::*;
    let Some(lock) = try_lock(path)? else {
        return Ok(Bind::AlreadyRunning);
    };
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
            Ok(Bind::Server(l, lock))
        }
        Err(e) if e.kind() == AddrInUse => Ok(Bind::AlreadyRunning),
        Err(e) => Err(e),
    }
}

/// [`send`], retrying for up to `patience` while nothing listens yet (an
/// instance holding the lock may still be starting up).
pub fn send_patiently(path: &Path, req: &Request, patience: Duration) -> io::Result<Event> {
    let deadline = Instant::now() + patience;
    loop {
        match send(path, req) {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            r => return r,
        }
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

/// Longest accepted request line (without the newline).
const MAX_LINE: usize = 64 * 1024;
/// Read chunk size, and chunks per `read_requests` call, so a chatty client
/// cannot keep the single-threaded HUD loop busy (poll is level-triggered).
const CHUNK: usize = 4096;
const MAX_CHUNKS_PER_CALL: usize = 4;

/// Most event bytes queued for a client that does not read them (a full
/// `Config` is a few KiB).
const MAX_QUEUED: usize = 1 << 20;

fn too_long() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "request line too long")
}

/// Nonblocking server-side connection with a line buffer.
pub struct Conn {
    stream: UnixStream,
    buf: Vec<u8>,
    eof: bool,
    /// Event bytes not yet accepted by the socket.
    out: Vec<u8>,
}

impl Conn {
    pub fn new(stream: UnixStream) -> io::Result<Conn> {
        stream.set_nonblocking(true)?;
        Ok(Conn {
            stream,
            buf: Vec::new(),
            eof: false,
            out: Vec::new(),
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
        let mut chunk = [0u8; CHUNK];
        for _ in 0..MAX_CHUNKS_PER_CALL {
            if self.eof {
                break;
            }
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
            if line.len() > MAX_LINE {
                return Err(too_long());
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            out.push(
                serde_json::from_slice(line)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            );
        }
        if self.buf.len() > MAX_LINE {
            return Err(too_long());
        }
        if out.is_empty() && self.eof {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(out)
    }

    /// Queues one event line and writes as much of the queue as the socket
    /// takes now, never blocking (the HUD loop must keep ticking). The rest
    /// goes out from [`Conn::flush`] when poll reports `POLLOUT`. Errors
    /// when the socket is broken or a stuck peer let more than
    /// `MAX_QUEUED` bytes pile up: drop the client then.
    pub fn send(&mut self, ev: &Event) -> io::Result<()> {
        let mut line = serde_json::to_vec(ev).map_err(io::Error::other)?;
        line.push(b'\n');
        self.out.extend_from_slice(&line);
        self.flush()?;
        if self.out.len() > MAX_QUEUED {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "client is not reading its events",
            ));
        }
        Ok(())
    }

    /// Writes queued output until the socket would block.
    pub fn flush(&mut self) -> io::Result<()> {
        use std::io::Write;
        let mut sent = 0;
        let result = loop {
            if sent == self.out.len() {
                break Ok(());
            }
            match self.stream.write(&self.out[sent..]) {
                Ok(0) => break Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => break Err(e),
            }
        };
        self.out.drain(..sent);
        result
    }

    /// Output is queued: poll this connection for `POLLOUT` too.
    pub fn wants_write(&self) -> bool {
        !self.out.is_empty()
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
        assert_eq!(
            serde_json::to_string(&Request::SetMode {
                mode: Mode::Interval
            })
            .unwrap(),
            r#"{"cmd":"set-mode","mode":"interval"}"#
        );
        assert_eq!(
            serde_json::to_string(&Request::SetCountdown { seconds: 90 }).unwrap(),
            r#"{"cmd":"set-countdown","seconds":90}"#
        );
    }

    #[test]
    fn event_json_shape() {
        let v = serde_json::to_value(Event::Focus { tab: None }).unwrap();
        assert_eq!(v["event"], "focus");
        let v = serde_json::to_value(Event::HotkeyStatus {
            errors: vec!["Cannot grab F5".into()],
        })
        .unwrap();
        assert_eq!(v["event"], "hotkey-status");
        assert_eq!(v["errors"][0], "Cannot grab F5");
    }

    #[test]
    fn stale_file_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        std::fs::write(&p, b"junk").unwrap();
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(..)));
    }

    #[test]
    fn stale_socket_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        drop(UnixListener::bind(&p).unwrap()); // leaves the file behind
        assert!(p.exists());
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(..)));
    }

    #[test]
    fn missing_path_binds_with_private_mode() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(..)));
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
    fn lock_holder_wins_and_its_socket_survives() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        let Bind::Server(_l, lock) = bind_or_detect(&p).unwrap() else {
            panic!("first starter must bind");
        };
        // A racing starter sees the lock, even before anything listens, and
        // must not remove the socket.
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::AlreadyRunning));
        assert!(p.exists());
        drop(_l);
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::AlreadyRunning));
        assert!(p.exists(), "socket of a lock holder is never stale");
        // Once the holder is gone, the stale socket is replaced.
        drop(lock);
        assert!(matches!(bind_or_detect(&p).unwrap(), Bind::Server(..)));
    }

    #[test]
    fn simultaneous_starters_bind_once() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (p, b) = (p.clone(), barrier.clone());
                std::thread::spawn(move || {
                    b.wait();
                    bind_or_detect(&p).unwrap()
                })
            })
            .collect();
        let binds: Vec<Bind> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        let servers = binds
            .iter()
            .filter(|b| matches!(b, Bind::Server(..)))
            .count();
        assert_eq!(servers, 1);
        // The winner's socket is still the live one.
        assert!(UnixStream::connect(&p).is_ok());
    }

    #[test]
    fn private_dir_created_and_checked() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let uid = nix::unistd::getuid().as_raw();
        let dir = d.path().join("timehud-x");
        private_dir(&dir, uid).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        // Ours but too open: tightened.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&dir, uid).unwrap();
        assert_eq!(mode(&dir), 0o700);
        // Someone else's: refused.
        let e = private_dir(&dir, uid + 1).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        // A symlink (even to our own dir) or a file: refused.
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(private_dir(&link, uid).is_err());
        let file = d.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(private_dir(&file, uid).is_err());
    }

    #[test]
    fn send_patiently_waits_for_a_starting_server() {
        let d = tempfile::tempdir().unwrap();
        let p = sock(&d);
        let p2 = p.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let l = UnixListener::bind(&p2).unwrap();
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 64];
            let _ = s.read(&mut b).unwrap();
            s.write_all(b"{\"event\":\"ok\"}\n").unwrap();
        });
        let r = send_patiently(&p, &Request::Toggle, Duration::from_secs(2));
        assert_eq!(r.unwrap(), Event::Ok);
        t.join().unwrap();
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
    fn conn_rejects_overlong_line_and_bounds_each_read() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        b.write_all(&vec![b'x'; 100 * 1024]).unwrap();
        // One call consumes at most a few chunks, not everything pending.
        assert!(c.read_requests().unwrap().is_empty());
        assert!(c.buf.len() <= MAX_CHUNKS_PER_CALL * CHUNK);
        let mut err = None;
        for _ in 0..64 {
            if let Err(e) = c.read_requests() {
                err = Some(e);
                break;
            }
        }
        assert_eq!(
            err.expect("cap not enforced").kind(),
            io::ErrorKind::InvalidData
        );
        assert!(c.buf.len() <= MAX_LINE + MAX_CHUNKS_PER_CALL * CHUNK);
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
        assert!(!c.wants_write());
    }

    fn big_event() -> Event {
        Event::Error {
            message: "x".repeat(32 * 1024),
        }
    }

    /// Reads everything available on a nonblocking stream.
    fn drain(b: &mut UnixStream) -> Vec<u8> {
        b.set_nonblocking(true).unwrap();
        let mut all = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = b.read(&mut buf) {
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n]);
        }
        all
    }

    #[test]
    fn conn_send_queues_instead_of_blocking_and_flushes_later() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        let t = Instant::now();
        // Far more than the socket buffer, less than the cap.
        for _ in 0..16 {
            c.send(&big_event()).unwrap();
        }
        assert!(t.elapsed() < Duration::from_millis(500), "send blocked");
        assert!(c.wants_write(), "socket buffer should be full by now");
        // The peer catches up; flushing delivers every line intact, in order.
        let mut got = Vec::new();
        for _ in 0..1000 {
            got.extend(drain(&mut b));
            c.flush().unwrap();
            if !c.wants_write() {
                break;
            }
        }
        got.extend(drain(&mut b));
        assert!(!c.wants_write());
        let lines: Vec<&[u8]> = got
            .split(|&x| x == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 16);
        for l in lines {
            assert_eq!(serde_json::from_slice::<Event>(l).unwrap(), big_event());
        }
    }

    #[test]
    fn conn_send_gives_up_on_a_stuck_peer() {
        let (a, _b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        let mut sent = 0;
        let err = loop {
            match c.send(&big_event()) {
                Ok(()) => sent += 1,
                Err(e) => break e,
            }
            assert!(sent < 1000, "queue cap not enforced");
        };
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert!(
            sent * 32 * 1024 >= MAX_QUEUED / 2,
            "gave up too early ({sent})"
        );
    }

    #[test]
    fn conn_send_to_closed_peer_errors() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut c = Conn::new(a).unwrap();
        drop(b);
        assert!(c.send(&Event::Ok).is_err());
    }
}
