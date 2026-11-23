//! Saved list views for the pull requests and issues pages: a name and the
//! search they restore. Ports `savedViews.ts`.

use serde::{Deserialize, Serialize};

use crate::pull_request_query::js_trim;

/// The longest a view's name may be.
pub const SAVED_VIEW_NAME_MAX_LENGTH: usize = 80;

/// A saved view: its id, its name, and the page's search it restores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedView<Search> {
    pub id: String,
    pub name: String,
    pub search: Search,
}

/// Why a view name cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedViewNameError {
    Empty,
    TooLong,
    Duplicate,
}

impl SavedViewNameError {
    /// The English message, also the translation source.
    pub fn message(self) -> String {
        match self {
            Self::Empty => "Enter a name for this view.".into(),
            Self::TooLong => format!("Use {SAVED_VIEW_NAME_MAX_LENGTH} characters or fewer."),
            Self::Duplicate => "A view with this name already exists.".into(),
        }
    }
}

/// Checks a name against the views already saved, ignoring the one being renamed.
pub fn saved_view_name_error<'a>(
    name: &str,
    views: impl IntoIterator<Item = (&'a str, &'a str)>,
    editing_id: Option<&str>,
) -> Option<SavedViewNameError> {
    let trimmed = js_trim(name);
    if trimmed.is_empty() {
        return Some(SavedViewNameError::Empty);
    }
    if trimmed.encode_utf16().count() > SAVED_VIEW_NAME_MAX_LENGTH {
        return Some(SavedViewNameError::TooLong);
    }
    let lower = trimmed.to_lowercase();
    views
        .into_iter()
        .any(|(id, view_name)| Some(id) != editing_id && view_name.to_lowercase() == lower)
        .then_some(SavedViewNameError::Duplicate)
}
