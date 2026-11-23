//! Whether a thread shows "Open in editor". Ports `OpenInPicker.logic.ts`.

use crate::remote_open::RemoteOpenState;

/// `shouldShowOpenInPicker`: the app's own environments always can; others
/// need an SSH route.
pub fn should_show_open_in_picker(has_active_project: bool, is_own_environment: bool, remote: &RemoteOpenState) -> bool {
    has_active_project && (is_own_environment || *remote != RemoteOpenState::LocalExec)
}
