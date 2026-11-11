//! HUD process entry point (filled in by a later task).

use crate::config::Config;
use std::os::unix::net::UnixListener;

pub fn run_hud(_cfg: Config, _listener: UnixListener, _no_tray: bool) -> anyhow::Result<()> {
    eprintln!("not yet implemented");
    Ok(())
}
