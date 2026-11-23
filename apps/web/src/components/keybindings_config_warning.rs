//! The "Invalid keybindings configuration" warning. Ports
//! `KeybindingsConfigWarning.tsx`: the issue's message and a button that
//! opens `keybindings.json` in the user's editor.

use std::rc::Rc;

use gpui_kit::App;

use super::keybindings_update_toast::OpenInEditor;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;

/// Pushes the warning toast and returns its id. Without a config path or an
/// editor the toast has no Open button.
pub fn keybindings_config_warning(
    message: &str,
    config_path: Option<&str>,
    open_in_editor: Option<OpenInEditor>,
    cx: &mut App,
) -> u64 {
    let mut toast = ToastData::new(ToastKind::Warning, t!("Invalid keybindings configuration")).description(message.to_owned());
    if let (Some(path), Some(open)) = (config_path, open_in_editor) {
        let path: Rc<str> = path.into();
        toast = toast.action(t!("Open keybindings.json"), move |window, cx| open(&path, window, cx));
    }
    Toaster::push(toast, cx)
}
