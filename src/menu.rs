//! Context menu model (shared by the overlay's right-click and the tray).

/// What a menu entry does; `App::perform` executes it.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    OpenSettings,
    ApplyStopwatch,
    /// Index into the raw `config.presets` list.
    ApplyPreset(usize),
    SavePreset,
    ManagePresets,
    SetTheme(&'static str),
    ToggleClickThrough,
    SetOpacity(f64),
    SetPosition(&'static str),
    ToggleVisibility,
    Quit,
}
