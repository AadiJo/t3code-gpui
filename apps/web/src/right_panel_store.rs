//! The right panel's tabs, per thread. Ports `rightPanelStore.ts`: each
//! thread remembers which surfaces (tabs) it has open, which is active,
//! whether the panel shows and whether it is maximized; closed tabs can be
//! reopened (`closedViewStore`). Persisted in the web app's
//! `t3code:right-panel-state:v2` shape (version 15).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The surface kinds (`RIGHT_PANEL_KINDS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RightPanelKind {
    Diff,
    Files,
    File,
    Preview,
    Device,
    Terminal,
    PullRequest,
    PullRequests,
    Issue,
    SideChat,
}

/// One tab (`RightPanelSurface`). The id encodes the resource, so opening
/// the same thing twice activates the existing tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RightPanelSurface {
    Diff { id: String },
    Files {
        id: String,
        #[serde(rename = "folderTrail", default, skip_serializing_if = "Option::is_none")]
        folder_trail: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    File { id: String, relative_path: String, reveal_line: Option<u32> },
    #[serde(rename_all = "camelCase")]
    Preview { id: String, resource_id: Option<String> },
    Device {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Terminal { id: String, resource_id: String, terminal_ids: Vec<String>, active_terminal_id: String },
    #[serde(rename_all = "camelCase")]
    PullRequest { id: String, project_id: String, repository: String, number: u32 },
    PullRequests { id: String },
    #[serde(rename_all = "camelCase")]
    Issue { id: String, project_id: String, tracker: String, scope: String, key: String },
    #[serde(rename_all = "camelCase")]
    SideChat { id: String, thread_id: String },
}

impl RightPanelSurface {
    pub fn id(&self) -> &str {
        match self {
            Self::Diff { id }
            | Self::Files { id, .. }
            | Self::File { id, .. }
            | Self::Preview { id, .. }
            | Self::Device { id, .. }
            | Self::Terminal { id, .. }
            | Self::PullRequest { id, .. }
            | Self::PullRequests { id }
            | Self::Issue { id, .. }
            | Self::SideChat { id, .. } => id,
        }
    }

    pub fn kind(&self) -> RightPanelKind {
        match self {
            Self::Diff { .. } => RightPanelKind::Diff,
            Self::Files { .. } => RightPanelKind::Files,
            Self::File { .. } => RightPanelKind::File,
            Self::Preview { .. } => RightPanelKind::Preview,
            Self::Device { .. } => RightPanelKind::Device,
            Self::Terminal { .. } => RightPanelKind::Terminal,
            Self::PullRequest { .. } => RightPanelKind::PullRequest,
            Self::PullRequests { .. } => RightPanelKind::PullRequests,
            Self::Issue { .. } => RightPanelKind::Issue,
            Self::SideChat { .. } => RightPanelKind::SideChat,
        }
    }

    /// The surface `open`/`toggle` adds for a kind that has one per thread.
    pub fn singleton(kind: SingletonKind) -> Self {
        match kind {
            SingletonKind::Diff => Self::Diff { id: "diff".into() },
            SingletonKind::Files => Self::Files { id: "files".into(), folder_trail: None },
            SingletonKind::PullRequests => Self::PullRequests { id: "pull-requests".into() },
            SingletonKind::Device => Self::Device { id: "device".into(), title: None },
            SingletonKind::Preview => Self::Preview { id: "browser:new".into(), resource_id: None },
        }
    }

    /// A pull request's tab; one tab per project, repository and number.
    pub fn pull_request(project_id: &str, repository: &str, number: u64) -> Self {
        Self::PullRequest {
            id: format!("pull-request:{project_id}:{repository}#{number}"),
            project_id: project_id.to_owned(),
            repository: repository.to_owned(),
            number: number as u32,
        }
    }
}

/// The kinds `open` and `toggle` accept (one tab per thread, or a new browser tab).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SingletonKind {
    Diff,
    Files,
    Preview,
    Device,
    PullRequests,
}

impl SingletonKind {
    pub const ALL: [SingletonKind; 5] = [Self::Diff, Self::Files, Self::Preview, Self::Device, Self::PullRequests];

    pub fn kind(self) -> RightPanelKind {
        match self {
            Self::Diff => RightPanelKind::Diff,
            Self::Files => RightPanelKind::Files,
            Self::Preview => RightPanelKind::Preview,
            Self::Device => RightPanelKind::Device,
            Self::PullRequests => RightPanelKind::PullRequests,
        }
    }
}

/// One thread's panel (`ThreadRightPanelState`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRightPanelState {
    pub is_open: bool,
    pub active_surface_id: Option<String>,
    pub surfaces: Vec<RightPanelSurface>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub maximized: bool,
}

impl ThreadRightPanelState {
    pub fn active(&self) -> Option<&RightPanelSurface> {
        let id = self.active_surface_id.as_deref()?;
        self.surfaces.iter().find(|s| s.id() == id)
    }

    fn is_empty(&self) -> bool {
        !self.is_open && self.active_surface_id.is_none() && self.surfaces.is_empty() && !self.maximized
    }

    /// `upsertSurface`: adds the tab if missing, opens the panel and activates it.
    fn upsert(&mut self, surface: RightPanelSurface) {
        self.is_open = true;
        self.active_surface_id = Some(surface.id().to_string());
        if !self.surfaces.iter().any(|s| s.id() == surface.id()) {
            self.surfaces.push(surface);
        }
    }
}

/// A closed tab, for reopen (`closedViewStore`).
#[derive(Clone, Debug)]
struct ClosedTab {
    thread: String,
    surface: RightPanelSurface,
    index: usize,
}

/// Most closed tabs remembered.
const CLOSED_LIMIT: usize = 20;

/// The persisted file's contents.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    by_thread_key: HashMap<String, ThreadRightPanelState>,
}

#[derive(Serialize, Deserialize)]
struct PersistEnvelope {
    state: PersistedState,
    version: u32,
}

const STORAGE_VERSION: u32 = 15;

/// Every thread's right panel. Keys are scoped thread keys (`env:thread`).
#[derive(Debug, Default)]
pub struct RightPanelStore {
    by_thread_key: HashMap<String, ThreadRightPanelState>,
    closed: Vec<ClosedTab>,
}

impl RightPanelStore {
    pub fn state(&self, thread: &str) -> ThreadRightPanelState {
        self.by_thread_key.get(thread).cloned().unwrap_or_default()
    }

    pub fn is_open(&self, thread: &str) -> bool {
        self.by_thread_key.get(thread).is_some_and(|s| s.is_open)
    }

    fn update(&mut self, thread: &str, edit: impl FnOnce(&mut ThreadRightPanelState)) {
        let mut state = self.state(thread);
        edit(&mut state);
        if state.is_empty() {
            self.by_thread_key.remove(thread);
        } else {
            self.by_thread_key.insert(thread.to_string(), state);
        }
    }

    /// `open`: adds or activates the kind's tab.
    pub fn open(&mut self, thread: &str, kind: SingletonKind) {
        self.update(thread, |state| {
            let existing = (kind == SingletonKind::Preview)
                .then(|| state.surfaces.iter().find(|s| s.kind() == RightPanelKind::Preview).cloned())
                .flatten();
            state.upsert(existing.unwrap_or_else(|| RightPanelSurface::singleton(kind)));
        });
    }

    /// `toggle`: closes the panel when that kind is showing, else opens it.
    pub fn toggle(&mut self, thread: &str, kind: SingletonKind) {
        let showing = self.by_thread_key.get(thread).is_some_and(|s| s.is_open && s.active().is_some_and(|a| a.kind() == kind.kind()));
        if showing {
            self.update(thread, |state| state.is_open = false);
        } else {
            self.open(thread, kind);
        }
    }

    /// Adds any surface (file, terminal, pull request…) and activates it.
    pub fn open_surface(&mut self, thread: &str, surface: RightPanelSurface) {
        self.update(thread, |state| state.upsert(surface));
    }

    /// `openPullRequest`: adds or activates the pull request's tab.
    pub fn open_pull_request(&mut self, thread: &str, project_id: &str, repository: &str, number: u64) {
        let surface = RightPanelSurface::pull_request(project_id, repository, number);
        self.update(thread, |state| state.upsert(surface));
    }

    /// `openSideChat`: adds or activates the side chat's tab on its parent's panel.
    pub fn open_side_chat(&mut self, thread: &str, side_chat_thread_id: &str) {
        let surface = RightPanelSurface::SideChat { id: format!("side-chat:{side_chat_thread_id}"), thread_id: side_chat_thread_id.to_owned() };
        self.update(thread, |state| state.upsert(surface));
    }

    /// `toggleVisibility`: the titlebar toggle. An empty panel shows the surface launcher.
    pub fn toggle_visibility(&mut self, thread: &str) {
        self.update(thread, |state| {
            state.is_open = !state.is_open;
            if !state.is_open {
                state.maximized = false;
            }
        });
    }

    pub fn show(&mut self, thread: &str) {
        self.update(thread, |state| state.is_open = true);
    }

    pub fn close(&mut self, thread: &str) {
        self.update(thread, |state| {
            state.is_open = false;
            state.maximized = false;
        });
    }

    pub fn set_maximized(&mut self, thread: &str, maximized: bool) {
        self.update(thread, |state| state.maximized = maximized);
    }

    pub fn activate_surface(&mut self, thread: &str, surface_id: &str) {
        self.update(thread, |state| {
            if state.surfaces.iter().any(|s| s.id() == surface_id) {
                state.is_open = true;
                state.active_surface_id = Some(surface_id.to_string());
            }
        });
    }

    /// `closeSurface`: removes the tab; the neighbour at its index becomes active.
    pub fn close_surface(&mut self, thread: &str, surface_id: &str) {
        let mut closed = None;
        self.update(thread, |state| {
            let Some(index) = state.surfaces.iter().position(|s| s.id() == surface_id) else { return };
            let surface = state.surfaces.remove(index);
            closed = Some((surface, index));
            state.is_open = state.is_open && !state.surfaces.is_empty();
            if state.active_surface_id.as_deref() == Some(surface_id) {
                state.active_surface_id =
                    state.surfaces.get(index.min(state.surfaces.len().saturating_sub(1))).map(|s| s.id().to_string());
            }
            if state.surfaces.is_empty() {
                state.maximized = false;
            }
        });
        if let Some((surface, index)) = closed {
            self.remember_closed(thread, surface, index);
        }
    }

    /// Closes the active tab (`rightPanel.close`, mod+w).
    pub fn close_active(&mut self, thread: &str) {
        if let Some(id) = self.by_thread_key.get(thread).and_then(|s| s.active_surface_id.clone()) {
            self.close_surface(thread, &id);
        }
    }

    pub fn close_other_surfaces(&mut self, thread: &str, surface_id: &str) {
        let state = self.state(thread);
        for surface in state.surfaces.iter().filter(|s| s.id() != surface_id) {
            self.close_surface(thread, surface.id());
        }
    }

    pub fn close_surfaces_to_right(&mut self, thread: &str, surface_id: &str) {
        let state = self.state(thread);
        let Some(index) = state.surfaces.iter().position(|s| s.id() == surface_id) else { return };
        for surface in state.surfaces[index + 1..].iter().rev() {
            self.close_surface(thread, surface.id());
        }
        if state.active_surface_id.as_deref().is_some_and(|a| !state.surfaces[..=index].iter().any(|s| s.id() == a)) {
            self.activate_surface(thread, surface_id);
        }
    }

    /// `moveSurface`: drag-reorder.
    pub fn move_surface(&mut self, thread: &str, surface_id: &str, to_index: usize) {
        self.update(thread, |state| {
            let Some(from) = state.surfaces.iter().position(|s| s.id() == surface_id) else { return };
            let surface = state.surfaces.remove(from);
            state.surfaces.insert(to_index.min(state.surfaces.len()), surface);
        });
    }

    fn remember_closed(&mut self, thread: &str, surface: RightPanelSurface, index: usize) {
        self.closed.push(ClosedTab { thread: thread.to_string(), surface, index });
        if self.closed.len() > CLOSED_LIMIT {
            self.closed.remove(0);
        }
    }

    /// `view.reopenClosed` (mod+shift+t): reopens the last tab closed in `thread`.
    pub fn reopen_closed(&mut self, thread: &str) -> bool {
        let Some(position) = self.closed.iter().rposition(|c| c.thread == thread) else { return false };
        let closed = self.closed.remove(position);
        self.update(thread, |state| {
            if !state.surfaces.iter().any(|s| s.id() == closed.surface.id()) {
                state.surfaces.insert(closed.index.min(state.surfaces.len()), closed.surface.clone());
            }
            state.is_open = true;
            state.active_surface_id = Some(closed.surface.id().to_string());
        });
        true
    }

    /// Drops a deleted thread's panel.
    pub fn remove_thread(&mut self, thread: &str) {
        self.by_thread_key.remove(thread);
        self.closed.retain(|c| c.thread != thread);
    }

    /// The persisted JSON (`{ state: { byThreadKey }, version }`).
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(&PersistEnvelope {
            state: PersistedState { by_thread_key: self.by_thread_key.clone() },
            version: STORAGE_VERSION,
        })
    }

    /// Reads [`Self::to_json`] output; anything unreadable starts empty.
    pub fn from_json(json: &str) -> Self {
        let state = serde_json::from_str::<PersistEnvelope>(json).map(|e| e.state).unwrap_or_default();
        Self { by_thread_key: state.by_thread_key, closed: Vec::new() }
    }
}
