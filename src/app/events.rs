//! Platform-neutral input events and the effects the controller asks for.
use crate::engine::Beep;

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
        root_x: i32,
        root_y: i32,
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
