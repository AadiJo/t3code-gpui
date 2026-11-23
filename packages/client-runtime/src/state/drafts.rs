//! The persisted shape of composer drafts (`composerDraftStore.ts`, persisted fields only),
//! keyed by scoped thread key `${environmentId}:${threadId}`. The composer owns editing; this
//! module owns the file. Removing an environment removes its drafts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use t3code_contracts::{EnvironmentId, ModelSelection, ProjectId, ProviderInteractionMode, RuntimeMode, ThreadId};

use super::client_store::ClientDocument;

/// The persisted key of a thread: `${environmentId}:${threadId}`.
pub fn scoped_thread_key(environment_id: &EnvironmentId, thread_id: &ThreadId) -> String {
    format!("{environment_id}:{thread_id}")
}

/// An image the user attached, as kept across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftImageAttachment {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// The image itself, as a `data:` URL, until it is uploaded.
    pub data_url: String,
}

/// One thread's unsent composer state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDraft {
    pub prompt: String,
    #[serde(default)]
    pub attachments: Vec<DraftImageAttachment>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_selection_by_provider: BTreeMap<String, ModelSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_provider: Option<String>,
    #[serde(default)]
    pub model_selection_explicit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<RuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_mode: Option<ProviderInteractionMode>,
}

/// Where an unsent new thread will run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DraftThreadEnvMode {
    Local,
    Worktree,
}

/// A new thread that has not been sent yet (`PersistedDraftThreadState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftThread {
    pub thread_id: ThreadId,
    pub environment_id: EnvironmentId,
    pub project_id: ProjectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_project_key: Option<String>,
    pub created_at: String,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub env_mode: DraftThreadEnvMode,
    #[serde(default)]
    pub start_from_origin: bool,
    /// Set once sent: the thread it became.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_to: Option<(EnvironmentId, ThreadId)>,
}

/// Every draft (`t3code:composer-drafts:v1`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drafts {
    pub drafts_by_thread_key: BTreeMap<String, ThreadDraft>,
    pub draft_threads_by_thread_key: BTreeMap<String, DraftThread>,
    pub logical_project_draft_thread_key_by_logical_project_key: BTreeMap<String, String>,
    #[serde(default)]
    pub sticky_model_selection_by_provider: BTreeMap<String, ModelSelection>,
    #[serde(default)]
    pub sticky_active_provider: Option<String>,
}

impl Drafts {
    /// Drops everything of a removed environment.
    pub fn remove_environment(&mut self, environment_id: &EnvironmentId) {
        let prefix = format!("{environment_id}:");
        self.drafts_by_thread_key.retain(|key, _| !key.starts_with(&prefix));
        self.draft_threads_by_thread_key.retain(|_, draft| &draft.environment_id != environment_id);
    }
}

impl ClientDocument for Drafts {
    const FILE: &'static str = "composer-drafts.json";
    const SCHEMA_VERSION: u32 = 1;
}
