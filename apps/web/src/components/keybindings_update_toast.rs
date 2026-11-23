//! Toasts when the server reloads `keybindings.json`. Ports
//! `KeybindingsUpdateToast.logic.ts` and `KeybindingsUpdateToast.tsx`: a
//! warning names the first keybindings issue; a clean reload says nothing.
//! The same change arriving twice inside two seconds toasts once.

use std::rc::Rc;

use gpui_kit::{App, Window};
use t3code_shared::keybindings::ServerConfigIssue;

use super::keybindings_config_warning::keybindings_config_warning;

const COOLDOWN_MS: i64 = 2_000;

/// What a keybindings update should show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindingsUpdateDecision {
    Success,
    InvalidConfiguration { message: String },
}

/// Deduplicates update toasts (`createKeybindingsUpdateToastController`).
#[derive(Default)]
pub struct KeybindingsUpdateToastController {
    last: Option<(String, i64)>,
}

impl KeybindingsUpdateToastController {
    /// The decision for a `keybindingsUpdated` event at `now_ms`, or `None`
    /// when the same toast showed less than two seconds ago. A save and the
    /// server's file watcher each publish the same snapshot.
    pub fn handle(&mut self, issues: &[ServerConfigIssue], now_ms: i64) -> Option<KeybindingsUpdateDecision> {
        let decision = match issues.first() {
            Some(issue) => KeybindingsUpdateDecision::InvalidConfiguration { message: issue.message().to_owned() },
            None => KeybindingsUpdateDecision::Success,
        };
        let key = match &decision {
            KeybindingsUpdateDecision::Success => "success".to_owned(),
            KeybindingsUpdateDecision::InvalidConfiguration { message } => format!("invalid:{message}"),
        };
        if let Some((last_key, at)) = &self.last
            && *last_key == key
            && now_ms - at < COOLDOWN_MS
        {
            return None;
        }
        self.last = Some((key, now_ms));
        Some(decision)
    }
}

/// Opens a file in the user's editor; `None` when no editor is available.
pub type OpenInEditor = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Shows the toast for a keybindings update (`startKeybindingsUpdateToasts`).
/// `config_path` and `open_in_editor` feed the warning's Open button.
pub fn show_keybindings_update_toast(
    controller: &mut KeybindingsUpdateToastController,
    issues: &[ServerConfigIssue],
    now_ms: i64,
    config_path: Option<&str>,
    open_in_editor: Option<OpenInEditor>,
    cx: &mut App,
) -> Option<u64> {
    match controller.handle(issues, now_ms)? {
        KeybindingsUpdateDecision::Success => None,
        KeybindingsUpdateDecision::InvalidConfiguration { message } => {
            Some(keybindings_config_warning(&message, config_path, open_in_editor, cx))
        }
    }
}
