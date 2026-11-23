//! Persisted UI state (`uiStateStore.ts`, `t3code:ui-state:v1`, and the sidebar shelf toggles):
//! project expansion and order, the local visit fallback for servers without visit tracking,
//! changed-files expansion and the merge method.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3code_contracts::EnvironmentId;

use super::client_store::ClientDocument;
use super::models::EpochMs;

/// Every persisted UI preference.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiState {
    /// By physical project key.
    pub project_expanded_by_id: BTreeMap<String, bool>,
    /// Physical project keys in the user's order.
    pub project_order: Vec<String>,
    /// Local visit watermarks by scoped thread key, used only when the server omits its own.
    pub thread_last_visited_at_by_id: BTreeMap<String, EpochMs>,
    /// Changed-files tree expansion by scoped thread key, then path.
    pub thread_changed_files_expanded_by_id: BTreeMap<String, BTreeMap<String, bool>>,
    pub sidebar_project_scope_keys: Vec<String>,
    pub default_advertised_endpoint_key: Option<String>,
    pub pull_request_merge_method: Option<String>,
    /// Sidebar shelves (`t3code:sidebar:*-expanded`) by shelf name.
    pub sidebar_shelf_expanded: BTreeMap<String, bool>,
}

impl UiState {
    /// Records a local visit; a newer watermark wins.
    pub fn mark_local_visit(&mut self, scoped_thread_key: String, visited_at: EpochMs) {
        let entry = self.thread_last_visited_at_by_id.entry(scoped_thread_key).or_insert(visited_at);
        *entry = (*entry).max(visited_at);
    }

    /// Drops everything of a removed environment.
    pub fn remove_environment(&mut self, environment_id: &EnvironmentId) {
        let prefix = format!("{environment_id}:");
        self.project_expanded_by_id.retain(|key, _| !key.starts_with(&prefix));
        self.project_order.retain(|key| !key.starts_with(&prefix));
        self.thread_last_visited_at_by_id.retain(|key, _| !key.starts_with(&prefix));
        self.thread_changed_files_expanded_by_id.retain(|key, _| !key.starts_with(&prefix));
    }
}

impl ClientDocument for UiState {
    const FILE: &'static str = "ui-state.json";
    const SCHEMA_VERSION: u32 = 1;
}
