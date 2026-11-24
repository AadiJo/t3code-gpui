//! UI state that survives a relaunch. Ports `uiStateStore.ts` (project
//! expansion and order, thread visit times, changed-files expansion, merge
//! method) and adds the window layout the web app keeps in other
//! `localStorage` keys (sidebar width, panel widths, the last route). Each
//! store is one JSON file in the app's data directory, written after a short
//! pause so drags and typing do not write on every frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// How long after the last change a file is written.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(300);

/// `t3code:ui-state:v1`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiState {
    pub project_expanded_by_id: HashMap<String, bool>,
    pub project_order: Vec<String>,
    pub sidebar_project_scope_keys: Vec<String>,
    /// The sidebar's Working, Snoozed and Settled shelves are open
    /// (`t3code:sidebar:{working,snoozed,settled}-expanded`).
    pub sidebar_working_expanded: bool,
    pub sidebar_snoozed_expanded: bool,
    pub sidebar_settled_expanded: bool,
    /// ISO-8601 visit time per scoped thread key.
    pub thread_last_visited_at_by_id: HashMap<String, String>,
    pub thread_changed_files_expanded_by_id: HashMap<String, HashMap<String, bool>>,
    pub default_advertised_endpoint_key: Option<String>,
    pub pull_request_merge_method: MergeMethod,
}

/// `PullRequestMergeMethod`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl UiState {
    /// `markThreadVisited`.
    pub fn mark_thread_visited(&mut self, thread: &str, visited_at: String) {
        self.thread_last_visited_at_by_id.insert(thread.to_string(), visited_at);
    }

    /// `markThreadUnread`: forget the visit so the row shows unread.
    pub fn mark_thread_unread(&mut self, thread: &str) {
        self.thread_last_visited_at_by_id.remove(thread);
    }

    /// `setProjectExpanded`.
    pub fn set_project_expanded(&mut self, project: &str, expanded: bool) {
        self.project_expanded_by_id.insert(project.to_string(), expanded);
    }
}

/// The window layout and the page to reopen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LayoutState {
    /// The route path at quit (`/env/thread`).
    pub last_route: String,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub right_panel_width: f32,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self { last_route: "/".into(), sidebar_open: true, sidebar_width: 256., right_panel_width: 560. }
    }
}

/// Where the JSON files live. `None` keeps everything in memory (scenes, checks).
#[derive(Clone, Debug, Default)]
pub struct StateDir(pub Option<PathBuf>);

impl StateDir {
    fn path(&self, name: &str) -> Option<PathBuf> {
        self.0.as_ref().map(|dir| dir.join(name))
    }

    /// Reads `name`, or `None` when it is missing.
    pub fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path(name)?).ok()
    }

    /// Reads and decodes `name`; a missing or unreadable file gives the default.
    pub fn load<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        self.read(name).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
    }

    /// Writes `contents` to `name` (through a temp file, so a crash never leaves half a file).
    pub fn write(&self, name: &str, contents: &str) {
        let Some(path) = self.path(name) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, contents).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.0.as_deref()
    }
}

/// File names in the state directory.
pub mod files {
    pub const UI_STATE: &str = "ui-state.json";
    pub const LAYOUT: &str = "layout.json";
    pub const RIGHT_PANEL: &str = "right-panel-state.json";
    pub const TERMINAL: &str = "terminal-state.json";
}

/// The UI state plus pending writes. A GPUI global; edit it with [`UiStateStore::update`].
pub struct UiStateStore {
    pub dir: StateDir,
    pub ui: UiState,
    pub layout: LayoutState,
    writes: HashMap<&'static str, Task<()>>,
}

impl Global for UiStateStore {}

impl UiStateStore {
    /// Loads both files from `dir`.
    pub fn load(dir: StateDir) -> Self {
        Self { ui: dir.load(files::UI_STATE), layout: dir.load(files::LAYOUT), dir, writes: HashMap::new() }
    }

    /// Edits the UI state and schedules its write.
    pub fn update_ui(cx: &mut App, edit: impl FnOnce(&mut UiState)) {
        let json = cx.update_global::<Self, _>(|store, _| {
            let before = store.ui.clone();
            edit(&mut store.ui);
            (store.ui != before).then(|| serde_json::to_string(&store.ui).ok()).flatten()
        });
        if let Some(json) = json {
            Self::schedule_write(files::UI_STATE, json, cx);
        }
    }

    /// Edits the layout and schedules its write.
    pub fn update_layout(cx: &mut App, edit: impl FnOnce(&mut LayoutState)) {
        let json = cx.update_global::<Self, _>(|store, _| {
            let before = store.layout.clone();
            edit(&mut store.layout);
            (store.layout != before).then(|| serde_json::to_string(&store.layout).ok()).flatten()
        });
        if let Some(json) = json {
            Self::schedule_write(files::LAYOUT, json, cx);
        }
    }

    /// Writes `json` to `name` after [`PERSIST_DEBOUNCE`]; a newer write replaces a pending one.
    pub fn schedule_write(name: &'static str, json: String, cx: &mut App) {
        let dir = cx.global::<Self>().dir.clone();
        if dir.dir().is_none() {
            return;
        }
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(PERSIST_DEBOUNCE).await;
            cx.background_executor().spawn(async move { dir.write(name, &json) }).await;
        });
        cx.global_mut::<Self>().writes.insert(name, task);
    }

    /// Writes every pending file now (on quit).
    pub fn flush(cx: &mut App) {
        let store = cx.global_mut::<Self>();
        store.writes.clear();
        if let Ok(json) = serde_json::to_string(&store.ui) {
            store.dir.write(files::UI_STATE, &json);
        }
        if let Ok(json) = serde_json::to_string(&store.layout) {
            store.dir.write(files::LAYOUT, &json);
        }
    }
}
