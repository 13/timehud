use clap::{Parser, Subcommand};
use std::process::ExitCode;
use timehud::config::{Config, config_path};
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

#[derive(Subcommand)]
enum Cmd {
    /// Start or pause the running timer
    Toggle,
    /// Reset the running timer
    Reset,
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

fn main() -> ExitCode {
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

    let path = ipc::socket_path();
    let control = match cli.command {
        Some(Cmd::Settings { tab }) => {
            settings::run_settings(tab)?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Cmd::SavePreset) => {
            settings::run_save_preset()?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Cmd::Toggle) => Some(Request::Toggle),
        Some(Cmd::Reset) => Some(Request::Reset),
        Some(Cmd::Show) => Some(Request::Show),
        Some(Cmd::Hide) => Some(Request::Hide),
        Some(Cmd::Quit) => Some(Request::Quit),
        None => None,
    };

    if let Some(req) = control {
        return Ok(match ipc::send(&path, &req) {
            Ok(Event::Error { message }) => {
                eprintln!("timehud: {message}");
                ExitCode::FAILURE
            }
            Ok(_) => ExitCode::SUCCESS,
            Err(_) => {
                eprintln!("TimeHUD is not running");
                ExitCode::FAILURE
            }
        });
    }

    match ipc::bind_or_detect(&path)? {
        Bind::AlreadyRunning => {
            ipc::send(&path, &Request::ToggleVisibility)?;
        }
        Bind::Server(listener) => {
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
