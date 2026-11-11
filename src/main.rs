use clap::{Parser, Subcommand};
use std::process::ExitCode;
use timehud::config::{Config, Mode, config_path};
use timehud::format::parse_duration;
use timehud::ipc::{self, Bind, Event, Request};
use timehud::layout::POSITIONS;
use timehud::{hud, settings};

#[derive(Parser)]
#[command(
    name = "timehud",
    about = "Always-on-top timer overlay",
    disable_version_flag = true
)]
struct Cli {
    /// Print version
    #[arg(short = 'V', long)]
    version: bool,
    /// Start at this screen position (resets any custom placement)
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(POSITIONS))]
    position: Option<String>,
    /// Overwrite the config file with defaults and exit
    #[arg(long)]
    reset_config: bool,
    /// Accepted for compatibility; TimeHUD always runs as an X11 client
    #[arg(long)]
    wayland: bool,
    /// Do not show the tray icon (this run only)
    #[arg(long)]
    no_tray: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Debug, PartialEq)]
enum Cmd {
    /// Start or pause the running timer
    Toggle,
    /// Start the timer (no-op if it runs)
    Start,
    /// Pause the timer (no-op if it is stopped)
    Pause,
    /// Reset the running timer
    Reset,
    /// Switch to a countdown of DURATION, stopped (ends the active preset)
    Set {
        /// 90 (seconds), 25m, 1h30m, 1h 5m 10s, mm:ss or h:mm:ss
        #[arg(value_parser = parse_duration)]
        duration: i64,
    },
    /// Apply the preset with this name (case-insensitive fallback)
    Preset {
        /// Preset name; several words are joined with spaces
        #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
        name: Vec<String>,
    },
    /// Switch the timer mode (stopped; ends the active preset)
    Mode {
        #[arg(value_parser = parse_mode)]
        mode: Mode,
    },
    /// Print the timer state: mode, state, display, preset
    Status {
        /// Print a JSON object instead of one line
        #[arg(long)]
        json: bool,
    },
    /// Show the HUD
    Show,
    /// Hide the HUD
    Hide,
    /// Quit the running instance
    Quit,
    #[command(hide = true)]
    Settings {
        #[arg(long)]
        tab: Option<String>,
    },
    #[command(hide = true)]
    SavePreset,
}

fn parse_mode(s: &str) -> Result<Mode, String> {
    s.parse()
}

/// The IPC request for a control subcommand (`None` for the ones that run
/// a window here).
fn control_request(cmd: &Cmd) -> Option<Request> {
    Some(match cmd {
        Cmd::Toggle => Request::Toggle,
        Cmd::Start => Request::Start,
        Cmd::Pause => Request::Pause,
        Cmd::Reset => Request::Reset,
        Cmd::Set { duration } => Request::SetCountdown { seconds: *duration },
        Cmd::Preset { name } => Request::ApplyPreset {
            name: name.join(" "),
        },
        Cmd::Mode { mode } => Request::SetMode { mode: *mode },
        Cmd::Status { .. } => Request::Status,
        Cmd::Show => Request::Show,
        Cmd::Hide => Request::Hide,
        Cmd::Quit => Request::Quit,
        Cmd::Settings { .. } | Cmd::SavePreset => return None,
    })
}

/// What to print for the HUD's reply: `Ok(text for stdout)` or
/// `Err(message for stderr)` (exit status 1).
fn reply_output(ev: Event, json: bool) -> Result<Option<String>, String> {
    match ev {
        Event::Error { message } => Err(message),
        Event::Status { status } if json => serde_json::to_string(&status)
            .map(Some)
            .map_err(|e| e.to_string()),
        Event::Status { status } => Ok(Some(status.line())),
        _ => Ok(None),
    }
}

fn main() -> ExitCode {
    hud::mark_start();
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("timehud: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    if cli.version {
        println!("TimeHUD {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    if cli.wayland {
        eprintln!("warning: --wayland is ignored; TimeHUD runs as an X11 client");
    }
    if cli.reset_config {
        Config::default().save()?;
        println!("Config reset \u{2192} {}", config_path().display());
        return Ok(ExitCode::SUCCESS);
    }

    let path = ipc::socket_path()?;
    let control = match cli.command {
        Some(Cmd::Settings { tab }) => {
            settings::run_settings(tab)?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Cmd::SavePreset) => {
            settings::run_save_preset()?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(cmd) => control_request(&cmd).map(|req| (req, cmd)),
        None => None,
    };

    if let Some((req, cmd)) = control {
        let json = matches!(cmd, Cmd::Status { json: true });
        let reply = match ipc::send(&path, &req) {
            Ok(ev) => reply_output(ev, json),
            Err(_) => Err("TimeHUD is not running".into()),
        };
        return Ok(match reply {
            Ok(out) => {
                if let Some(text) = out {
                    println!("{text}");
                }
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("timehud: {message}");
                ExitCode::FAILURE
            }
        });
    }

    match ipc::bind_or_detect(&path)? {
        Bind::AlreadyRunning => {
            // The other instance may still be starting: give it a moment.
            ipc::send_patiently(
                &path,
                &Request::ToggleVisibility,
                std::time::Duration::from_secs(2),
            )?;
        }
        // `_lock` stays held until the HUD exits.
        Bind::Server(listener, _lock) => {
            let mut cfg = Config::load();
            if let Some(p) = cli.position {
                cfg.position = p;
                cfg.custom_x = -1;
                cfg.custom_y = -1;
            }
            hud::run_hud(cfg, listener, cli.no_tray)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use timehud::app::Status;

    fn cmd(args: &[&str]) -> Result<Cmd, String> {
        let argv = std::iter::once("timehud").chain(args.iter().copied());
        Cli::try_parse_from(argv)
            .map_err(|e| e.to_string())
            .and_then(|c| c.command.ok_or_else(|| "no command".into()))
    }

    #[test]
    fn control_subcommands_map_to_requests() {
        let req = |args: &[&str]| control_request(&cmd(args).unwrap());
        assert_eq!(req(&["start"]), Some(Request::Start));
        assert_eq!(req(&["pause"]), Some(Request::Pause));
        assert_eq!(req(&["toggle"]), Some(Request::Toggle));
        assert_eq!(
            req(&["set", "25m"]),
            Some(Request::SetCountdown { seconds: 1500 })
        );
        assert_eq!(
            req(&["set", "1:30"]),
            Some(Request::SetCountdown { seconds: 90 })
        );
        assert_eq!(
            req(&["preset", "5", "min"]),
            Some(Request::ApplyPreset {
                name: "5 min".into()
            })
        );
        assert_eq!(
            req(&["mode", "Interval"]),
            Some(Request::SetMode {
                mode: Mode::Interval
            })
        );
        assert_eq!(req(&["status", "--json"]), Some(Request::Status));
        assert_eq!(req(&["settings"]), None);
    }

    #[test]
    fn bad_arguments_are_rejected_before_sending() {
        assert!(cmd(&["set", "soon"]).is_err());
        assert!(cmd(&["set"]).is_err());
        assert!(cmd(&["mode", "lap"]).is_err());
        assert!(cmd(&["preset"]).is_err());
    }

    #[test]
    fn replies_print_or_fail() {
        let status = Status {
            mode: "countdown".into(),
            state: "running".into(),
            display: "04:12".into(),
            label: "TEA".into(),
            preset: "Tea".into(),
            visible: true,
        };
        let ev = || Event::Status {
            status: status.clone(),
        };
        assert_eq!(
            reply_output(ev(), false),
            Ok(Some("countdown running 04:12 Tea".into()))
        );
        let json: serde_json::Value =
            serde_json::from_str(&reply_output(ev(), true).unwrap().unwrap()).unwrap();
        assert_eq!(json["display"], "04:12");
        assert_eq!(json["preset"], "Tea");
        assert_eq!(reply_output(Event::Ok, false), Ok(None));
        assert_eq!(
            reply_output(
                Event::Error {
                    message: "no preset named 'x'".into()
                },
                false
            ),
            Err("no preset named 'x'".into())
        );
    }
}
