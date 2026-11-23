//! Whether the palette is open, in which mode, and what it opened to. Ports
//! `commandPaletteStore.ts` and `reduceCommandPaletteUiState`.

use gpui_kit::SharedString;

/// Which overlay the palette shows: commands, the file picker, or project search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchOverlayMode {
    #[default]
    Command,
    Files,
    Content,
}

/// The page a palette session starts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenIntent {
    AddProject,
    NewThreadIn,
    ChangeTheme,
    PortForwards,
    /// The root page with this query typed.
    Search(SharedString),
}

/// Palette UI state (`CommandPaletteUiState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandPaletteUiState {
    pub open: bool,
    pub mode: SearchOverlayMode,
    /// Read once when a session starts.
    pub open_intent: Option<OpenIntent>,
    /// Bumped by every intent, so an open palette starts over from the new one.
    pub session: u64,
}

/// A change to the palette UI state (`CommandPaletteUiAction`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandPaletteUiAction {
    SetOpen(bool),
    /// Opens `mode`, or closes the palette when it already shows it.
    ToggleMode(SearchOverlayMode),
    Open(OpenIntent),
    BackToCommand,
}

impl CommandPaletteUiState {
    /// The next state (`reduceCommandPaletteUiState`).
    pub fn reduce(&self, action: CommandPaletteUiAction) -> Self {
        let session = self.session;
        match action {
            CommandPaletteUiAction::SetOpen(true) => {
                Self { open: true, mode: SearchOverlayMode::Command, open_intent: self.open_intent.clone(), session }
            }
            CommandPaletteUiAction::SetOpen(false) => Self { open: false, mode: self.mode, open_intent: None, session },
            CommandPaletteUiAction::ToggleMode(mode) if self.open && self.mode == mode => {
                Self { open: false, mode, open_intent: None, session }
            }
            CommandPaletteUiAction::ToggleMode(mode) => Self { open: true, mode, open_intent: None, session },
            CommandPaletteUiAction::Open(intent) => {
                Self { open: true, mode: SearchOverlayMode::Command, open_intent: Some(intent), session: session + 1 }
            }
            CommandPaletteUiAction::BackToCommand if self.open && self.mode != SearchOverlayMode::Command => {
                Self { open: true, mode: SearchOverlayMode::Command, open_intent: None, session }
            }
            CommandPaletteUiAction::BackToCommand => self.clone(),
        }
    }
}
