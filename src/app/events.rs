//! Platform-neutral input events and the effects the controller asks for.
use crate::engine::Beep;
use serde::{Deserialize, Serialize};

/// A snapshot of the timer for `timehud status` and the tray tooltip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// `stopwatch`, `countdown` or `interval`.
    pub mode: String,
    /// `running`, `paused` or `idle` (stopped at the start).
    pub state: String,
    /// The time as the HUD shows it, e.g. `04:12`.
    pub display: String,
    /// The mode label, e.g. `COUNTDOWN`, `WORK 2/8` or `TEA`.
    pub label: String,
    /// The active preset's name, empty for none.
    pub preset: String,
    /// Whether the overlay is shown.
    pub visible: bool,
}

impl Status {
    /// One line for scripts: mode, state, display, then the preset (which
    /// may contain spaces) if any.
    pub fn line(&self) -> String {
        let mut s = format!("{} {} {}", self.mode, self.state, self.display);
        if !self.preset.is_empty() {
            s.push(' ');
            s.push_str(&self.preset);
        }
        s
    }

    /// The tray tooltip, e.g. `TimeHUD \u{2014} Countdown 04:12 (running) \u{b7} Tea`.
    pub fn tooltip(&self) -> String {
        let mut mode = self.mode.clone();
        if let Some(first) = mode.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        let mut s = format!("TimeHUD \u{2014} {mode} {} ({})", self.display, self.state);
        if !self.preset.is_empty() {
            s.push_str(" \u{b7} ");
            s.push_str(&self.preset);
        }
        s
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Space,
    R,
    Escape,
    Q,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    /// Window-local (`x`, `y`) and root coordinates.
    Press {
        x: i32,
        y: i32,
        root_x: i32,
        root_y: i32,
        button: Button,
    },
    Release {
        button: Button,
    },
    Motion {
        x: i32,
        y: i32,
        root_x: i32,
        root_y: i32,
    },
    Wheel {
        x: i32,
        y: i32,
        up: bool,
    },
    Enter,
    Leave,
    Key {
        key: Key,
        ctrl: bool,
    },
}

/// The overlay's pointer shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    /// The parent's (the desktop's) cursor.
    #[default]
    Default,
    /// Pointing hand over things that react to a click.
    Hand,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Beep(Beep),
    /// Config changed; save after the 300 ms debounce.
    SaveSoon,
    SaveNow,
    /// New window position (`window_pos()`).
    Move {
        x: i32,
        y: i32,
    },
    /// Window size changed; the next `frame()` has the new size.
    Resize,
    Redraw,
    SetClickThrough(bool),
    /// Pointer shape over the overlay changed (see `cursor_for`).
    SetCursor(Cursor),
    SetVisible(bool),
    SetTrayVisible(bool),
    OpenMenu {
        root_x: i32,
        root_y: i32,
        include_window_actions: bool,
    },
    OpenSettings {
        tab: Option<String>,
    },
    OpenSavePreset,
    Notify(String),
    Quit,
}
