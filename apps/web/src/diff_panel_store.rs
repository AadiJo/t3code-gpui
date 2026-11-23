//! Diff panel state that outlives the panel view. Ports `diffPanelStore.ts`:
//! the persisted per-thread scope selection, plus the session-only folds,
//! scroll positions and whitespace override.

use std::collections::{HashMap, HashSet};

use gpui_kit::{App, Global, SharedString};
use serde::{Deserialize, Serialize};

/// What the panel compares, per thread. Ports `DiffPanelSelection`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DiffPanelSelection {
    /// One tool call's recorded patch.
    #[serde(rename_all = "camelCase")]
    ToolCall { thread_id: String, item_id: String, revision: String },
    /// The thread's branch against a base ref (automatic when `None`).
    #[serde(rename_all = "camelCase")]
    Branch { base_ref: Option<String> },
    /// Uncommitted working tree changes.
    Unstaged,
    /// One commit on the branch.
    #[serde(rename_all = "camelCase")]
    Commit { sha: String, base_ref: Option<String> },
    /// One turn's checkpoint diff, optionally revealing a file.
    #[serde(rename_all = "camelCase")]
    Turn { turn_id: String, file_path: Option<String>, reveal_request_id: u32 },
}

impl Default for DiffPanelSelection {
    fn default() -> Self {
        DiffPanelSelection::Branch { base_ref: None }
    }
}

/// The git scopes the scope menu switches between directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitScope {
    Branch,
    Unstaged,
}

/// Diff appearance settings (`settings.ts` diff group). Defaults are the
/// owner's: stacked, wrapped, whitespace hidden, files collapsed, red-green.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSettings {
    pub diff_layout: DiffLayout,
    pub word_wrap: bool,
    pub diff_ignore_whitespace: bool,
    pub diff_files_collapsed: bool,
    pub diff_color_scheme: DiffColorScheme,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            diff_layout: DiffLayout::Stacked,
            word_wrap: true,
            diff_ignore_whitespace: true,
            diff_files_collapsed: true,
            diff_color_scheme: DiffColorScheme::RedGreen,
        }
    }
}

/// `diffLayout`: stacked is Pierre's unified view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffLayout {
    Stacked,
    Split,
}

/// `diffColorScheme`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffColorScheme {
    RedGreen,
    BlueOrange,
}

/// Persisted part of the store, in the shape the web keeps in localStorage.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedDiffPanelState {
    pub by_thread_key: HashMap<String, DiffPanelSelection>,
    pub branch_base_ref_by_thread_key: HashMap<String, Option<String>>,
}

/// The app-wide diff panel store.
#[derive(Default)]
pub struct DiffPanelStore {
    pub persisted: PersistedDiffPanelState,
    /// Files the user folded or unfolded, per review scope. Absent means the default.
    collapsed_by_scope: HashMap<SharedString, HashSet<SharedString>>,
    /// Last scroll offset per review scope.
    scroll_by_scope: HashMap<SharedString, f32>,
    /// Overrides `diffIgnoreWhitespace` once the toolbar toggle is used.
    pub ignore_whitespace: Option<bool>,
    pub hide_test_files: bool,
    pub file_tree_open: bool,
    pub settings: DiffSettings,
}

impl Global for DiffPanelStore {}

impl DiffPanelStore {
    pub fn global(cx: &mut App) -> &mut Self {
        cx.default_global::<Self>()
    }

    pub fn selection(&self, thread_key: &str) -> DiffPanelSelection {
        self.persisted.by_thread_key.get(thread_key).cloned().unwrap_or_default()
    }

    fn branch_base_ref(&self, thread_key: &str) -> Option<String> {
        match self.persisted.by_thread_key.get(thread_key) {
            Some(DiffPanelSelection::Branch { base_ref } | DiffPanelSelection::Commit { base_ref, .. }) => base_ref.clone(),
            _ => self.persisted.branch_base_ref_by_thread_key.get(thread_key).cloned().flatten(),
        }
    }

    pub fn select_git_scope(&mut self, thread_key: &str, scope: GitScope) {
        let base_ref = self.branch_base_ref(thread_key);
        if let Some(DiffPanelSelection::Branch { base_ref } | DiffPanelSelection::Commit { base_ref, .. }) =
            self.persisted.by_thread_key.get(thread_key)
        {
            self.persisted.branch_base_ref_by_thread_key.insert(thread_key.into(), base_ref.clone());
        }
        let selection = match scope {
            GitScope::Branch => DiffPanelSelection::Branch { base_ref },
            GitScope::Unstaged => DiffPanelSelection::Unstaged,
        };
        self.persisted.by_thread_key.insert(thread_key.into(), selection);
    }

    pub fn select_branch_base_ref(&mut self, thread_key: &str, base_ref: Option<&str>) {
        let base_ref = base_ref.map(str::trim).filter(|r| !r.is_empty()).map(str::to_owned);
        self.persisted.by_thread_key.insert(thread_key.into(), DiffPanelSelection::Branch { base_ref: base_ref.clone() });
        self.persisted.branch_base_ref_by_thread_key.insert(thread_key.into(), base_ref);
    }

    pub fn select_turn(&mut self, thread_key: &str, turn_id: &str, file_path: Option<&str>) {
        let reveal_request_id = match self.persisted.by_thread_key.get(thread_key) {
            Some(DiffPanelSelection::Turn { reveal_request_id, .. }) => reveal_request_id + 1,
            _ => 1,
        };
        self.persisted.by_thread_key.insert(
            thread_key.into(),
            DiffPanelSelection::Turn { turn_id: turn_id.into(), file_path: file_path.map(Into::into), reveal_request_id },
        );
    }

    pub fn select_commit(&mut self, thread_key: &str, sha: &str) {
        let base_ref = self.branch_base_ref(thread_key);
        self.persisted
            .by_thread_key
            .insert(thread_key.into(), DiffPanelSelection::Commit { sha: sha.into(), base_ref: base_ref.clone() });
        self.persisted.branch_base_ref_by_thread_key.insert(thread_key.into(), base_ref);
    }

    pub fn select_tool_call(&mut self, thread_key: &str, thread_id: &str, item_id: &str, revision: &str) {
        self.persisted.by_thread_key.insert(
            thread_key.into(),
            DiffPanelSelection::ToolCall { thread_id: thread_id.into(), item_id: item_id.into(), revision: revision.into() },
        );
    }

    /// Moves a turn selection whose turn disappeared to the latest turn.
    pub fn reconcile_turn_selection(&mut self, thread_key: &str, available_turn_ids: &[&str]) {
        let Some(DiffPanelSelection::Turn { turn_id, .. }) = self.persisted.by_thread_key.get_mut(thread_key) else {
            return;
        };
        if let Some(latest) = available_turn_ids.first().filter(|_| !available_turn_ids.contains(&turn_id.as_str())) {
            *turn_id = (*latest).into();
        }
    }

    pub fn remove_thread(&mut self, thread_key: &str) {
        self.persisted.by_thread_key.remove(thread_key);
        self.persisted.branch_base_ref_by_thread_key.remove(thread_key);
    }

    /// The folded files of a scope, or `None` while it follows the default.
    pub fn collapsed_files(&self, scope_key: &str) -> Option<&HashSet<SharedString>> {
        self.collapsed_by_scope.get(scope_key)
    }

    pub fn set_collapsed_files(&mut self, scope_key: SharedString, collapsed: HashSet<SharedString>) {
        self.collapsed_by_scope.insert(scope_key, collapsed);
    }

    pub fn scroll_offset(&self, scope_key: &str) -> Option<f32> {
        self.scroll_by_scope.get(scope_key).copied()
    }

    pub fn set_scroll_offset(&mut self, scope_key: SharedString, offset: f32) {
        self.scroll_by_scope.insert(scope_key, offset);
    }

    pub fn discard_scroll_offset(&mut self, scope_key: &str) {
        self.scroll_by_scope.remove(scope_key);
    }

    /// The effective whitespace setting: the toolbar override, else the setting.
    pub fn effective_ignore_whitespace(&self) -> bool {
        self.ignore_whitespace.unwrap_or(self.settings.diff_ignore_whitespace)
    }
}
