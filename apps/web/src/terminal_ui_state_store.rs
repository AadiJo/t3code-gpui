//! Per-thread terminal drawer state: which terminals a thread has, how they
//! are grouped into splits, which is active, whether the drawer is open and
//! how tall it is. Ports `terminalUiStateStore.ts`, including how local ids
//! follow the server's terminal list. Persisted as JSON in the web app's
//! `t3code:terminal-state:v1` shape.

use std::collections::{HashMap, HashSet};

use gpui_kit::Global;
use serde::{Deserialize, Serialize};

/// Default drawer height in px.
pub const DEFAULT_THREAD_TERMINAL_HEIGHT: f32 = 280.;
/// The id the first terminal of a thread gets.
pub const DEFAULT_THREAD_TERMINAL_ID: &str = "term-1";
/// Most terminals one split group shows side by side.
pub const MAX_TERMINALS_PER_GROUP: usize = 4;

/// A thread's scoped key (`environmentId` and `threadId`), as the client runtime formats it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadKey(pub String);

/// How a split group lays out its terminals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    /// Side by side.
    #[default]
    Horizontal,
    /// Stacked.
    Vertical,
}

impl SplitDirection {
    fn is_horizontal(&self) -> bool {
        *self == SplitDirection::Horizontal
    }
}

/// Terminals shown together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTerminalGroup {
    pub id: String,
    pub terminal_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "SplitDirection::is_horizontal")]
    pub split_direction: SplitDirection,
}

/// One thread's drawer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTerminalUiState {
    pub terminal_open: bool,
    pub terminal_height: f32,
    pub terminal_ids: Vec<String>,
    pub active_terminal_id: String,
    pub terminal_groups: Vec<ThreadTerminalGroup>,
    pub active_terminal_group_id: String,
}

impl Default for ThreadTerminalUiState {
    fn default() -> Self {
        Self {
            terminal_open: false,
            terminal_height: DEFAULT_THREAD_TERMINAL_HEIGHT,
            terminal_ids: Vec::new(),
            active_terminal_id: String::new(),
            terminal_groups: Vec::new(),
            active_terminal_group_id: String::new(),
        }
    }
}

fn normalize_ids<'a>(ids: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && seen.insert(id.to_string()))
        .map(str::to_string)
        .collect()
}

fn fallback_group_id(terminal_id: &str) -> String {
    format!("group-{terminal_id}")
}

fn unique_group_id(base: String, used: &mut HashSet<String>) -> String {
    let mut candidate = base.clone();
    let mut index = 2;
    while used.contains(&candidate) {
        candidate = format!("{base}-{index}");
        index += 1;
    }
    used.insert(candidate.clone());
    candidate
}

/// Groups with unknown or repeated ids dropped, and loose terminals given their own group.
fn normalize_groups(groups: &[ThreadTerminalGroup], terminal_ids: &[String]) -> Vec<ThreadTerminalGroup> {
    if terminal_ids.is_empty() {
        return Vec::new();
    }
    let valid: HashSet<&String> = terminal_ids.iter().collect();
    let mut assigned = HashSet::new();
    let mut used = HashSet::new();
    let mut next = Vec::new();
    for group in groups {
        let ids: Vec<String> = normalize_ids(&group.terminal_ids)
            .into_iter()
            .filter(|id| valid.contains(id) && !assigned.contains(id))
            .collect();
        let Some(first) = ids.first() else { continue };
        assigned.extend(ids.iter().cloned());
        let base = if group.id.trim().is_empty() { fallback_group_id(first) } else { group.id.trim().to_string() };
        next.push(ThreadTerminalGroup {
            id: unique_group_id(base, &mut used),
            terminal_ids: ids,
            split_direction: group.split_direction,
        });
    }
    for id in terminal_ids {
        if !assigned.contains(id) {
            next.push(ThreadTerminalGroup {
                id: unique_group_id(fallback_group_id(id), &mut used),
                terminal_ids: vec![id.clone()],
                split_direction: SplitDirection::Horizontal,
            });
        }
    }
    next
}

impl ThreadTerminalUiState {
    /// Repairs ids, groups and the active selection so every field agrees.
    pub fn normalized(&self) -> Self {
        let terminal_ids = normalize_ids(&self.terminal_ids);
        let active_terminal_id = if terminal_ids.contains(&self.active_terminal_id) {
            self.active_terminal_id.clone()
        } else {
            terminal_ids.first().cloned().unwrap_or_default()
        };
        let terminal_groups = normalize_groups(&self.terminal_groups, &terminal_ids);
        let active_terminal_group_id = terminal_groups
            .iter()
            .find(|group| group.id == self.active_terminal_group_id)
            .or_else(|| terminal_groups.iter().find(|group| group.terminal_ids.contains(&active_terminal_id)))
            .or(terminal_groups.first())
            .map(|group| group.id.clone())
            .unwrap_or_default();
        Self {
            terminal_open: self.terminal_open,
            terminal_height: if self.terminal_height.is_finite() && self.terminal_height > 0. {
                self.terminal_height
            } else {
                DEFAULT_THREAD_TERMINAL_HEIGHT
            },
            terminal_ids,
            active_terminal_id,
            terminal_groups,
            active_terminal_group_id,
        }
    }

    fn is_default(&self) -> bool {
        self.normalized() == Self::default()
    }

    /// The group shown in the drawer.
    pub fn active_group(&self) -> Option<&ThreadTerminalGroup> {
        self.terminal_groups
            .iter()
            .find(|group| group.id == self.active_terminal_group_id)
            .or_else(|| self.terminal_groups.iter().find(|group| group.terminal_ids.contains(&self.active_terminal_id)))
            .or(self.terminal_groups.first())
    }

    fn upsert(&self, terminal_id: &str, split: Option<SplitDirection>) -> Self {
        let normalized = self.normalized();
        let terminal_id = terminal_id.trim();
        if terminal_id.is_empty() {
            return normalized;
        }
        let split = if normalized.terminal_ids.is_empty() { None } else { split };
        let is_new = !normalized.terminal_ids.iter().any(|id| id == terminal_id);
        let mut terminal_ids = normalized.terminal_ids.clone();
        if is_new {
            terminal_ids.push(terminal_id.to_string());
        }
        let mut groups = normalized.terminal_groups.clone();
        if let Some(index) = groups.iter().position(|group| group.terminal_ids.iter().any(|id| id == terminal_id)) {
            groups[index].terminal_ids.retain(|id| id != terminal_id);
            if groups[index].terminal_ids.is_empty() {
                groups.remove(index);
            }
        }

        let Some(direction) = split else {
            let mut used: HashSet<String> = groups.iter().map(|group| group.id.clone()).collect();
            let group_id = unique_group_id(fallback_group_id(terminal_id), &mut used);
            groups.push(ThreadTerminalGroup {
                id: group_id.clone(),
                terminal_ids: vec![terminal_id.to_string()],
                split_direction: SplitDirection::Horizontal,
            });
            return Self {
                terminal_open: true,
                terminal_ids,
                active_terminal_id: terminal_id.to_string(),
                terminal_groups: groups,
                active_terminal_group_id: group_id,
                ..normalized
            }
            .normalized();
        };

        let mut active_index = groups
            .iter()
            .position(|group| group.id == normalized.active_terminal_group_id)
            .or_else(|| groups.iter().position(|group| group.terminal_ids.contains(&normalized.active_terminal_id)));
        if active_index.is_none() {
            let mut used: HashSet<String> = groups.iter().map(|group| group.id.clone()).collect();
            groups.push(ThreadTerminalGroup {
                id: unique_group_id(fallback_group_id(&normalized.active_terminal_id), &mut used),
                terminal_ids: vec![normalized.active_terminal_id.clone()],
                split_direction: SplitDirection::Horizontal,
            });
            active_index = Some(groups.len() - 1);
        }
        let destination = &mut groups[active_index.expect("set above")];
        let contains = destination.terminal_ids.iter().any(|id| id == terminal_id);
        if is_new && !contains && destination.terminal_ids.len() >= MAX_TERMINALS_PER_GROUP {
            return normalized;
        }
        if !contains {
            match destination.terminal_ids.iter().position(|id| *id == normalized.active_terminal_id) {
                Some(anchor) => destination.terminal_ids.insert(anchor + 1, terminal_id.to_string()),
                None => destination.terminal_ids.push(terminal_id.to_string()),
            }
        }
        destination.split_direction = direction;
        let group_id = destination.id.clone();
        Self {
            terminal_open: true,
            terminal_ids,
            active_terminal_id: terminal_id.to_string(),
            terminal_groups: groups,
            active_terminal_group_id: group_id,
            ..normalized
        }
        .normalized()
    }

    /// Opens or hides the drawer; opening an empty drawer creates `term-1`.
    pub fn with_open(&self, open: bool) -> Self {
        let normalized = self.normalized();
        if open && normalized.terminal_ids.is_empty() {
            return normalized.upsert(DEFAULT_THREAD_TERMINAL_ID, None);
        }
        Self { terminal_open: open, ..normalized }
    }

    pub fn with_height(&self, height: f32) -> Self {
        let normalized = self.normalized();
        if !height.is_finite() || height <= 0. {
            return normalized;
        }
        Self { terminal_height: height, ..normalized }
    }

    /// Adds a terminal to the active group, beside (or below) the active one.
    pub fn with_split(&self, terminal_id: &str, direction: SplitDirection) -> Self {
        self.upsert(terminal_id, Some(direction))
    }

    /// Adds a terminal in a new group of its own.
    pub fn with_new(&self, terminal_id: &str) -> Self {
        self.upsert(terminal_id, None)
    }

    pub fn with_active(&self, terminal_id: &str) -> Self {
        let normalized = self.normalized();
        if !normalized.terminal_ids.iter().any(|id| id == terminal_id) {
            return normalized;
        }
        let group_id = normalized
            .terminal_groups
            .iter()
            .find(|group| group.terminal_ids.iter().any(|id| id == terminal_id))
            .map(|group| group.id.clone())
            .unwrap_or_else(|| normalized.active_terminal_group_id.clone());
        Self { active_terminal_id: terminal_id.to_string(), active_terminal_group_id: group_id, ..normalized }
    }

    /// Removes a terminal; the active one passes to a split neighbour first.
    pub fn without(&self, terminal_id: &str) -> Self {
        let normalized = self.normalized();
        let Some(closed_index) = normalized.terminal_ids.iter().position(|id| id == terminal_id) else {
            return normalized;
        };
        let remaining: Vec<String> = normalized.terminal_ids.iter().filter(|id| *id != terminal_id).cloned().collect();
        if remaining.is_empty() {
            return Self::default();
        }
        let group_ids = normalized
            .terminal_groups
            .iter()
            .find(|group| group.terminal_ids.iter().any(|id| id == terminal_id))
            .map(|group| group.terminal_ids.clone())
            .unwrap_or_default();
        let in_group = group_ids.iter().position(|id| id == terminal_id);
        let active = if normalized.active_terminal_id == terminal_id {
            in_group
                .and_then(|index| group_ids.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| group_ids.get(i))))
                .or_else(|| remaining.get(closed_index.min(remaining.len() - 1)))
                .cloned()
                .unwrap_or_default()
        } else {
            normalized.active_terminal_id.clone()
        };
        let groups: Vec<ThreadTerminalGroup> = normalized
            .terminal_groups
            .iter()
            .filter_map(|group| {
                let ids: Vec<String> = group.terminal_ids.iter().filter(|id| *id != terminal_id).cloned().collect();
                (!ids.is_empty()).then(|| ThreadTerminalGroup { terminal_ids: ids, ..group.clone() })
            })
            .collect();
        let group_id = groups
            .iter()
            .find(|group| group.terminal_ids.contains(&active))
            .or(groups.first())
            .map(|group| group.id.clone())
            .unwrap_or_else(|| fallback_group_id(&active));
        Self {
            terminal_ids: remaining,
            active_terminal_id: active,
            terminal_groups: groups,
            active_terminal_group_id: group_id,
            ..normalized
        }
        .normalized()
    }

    /// Adds terminals the server has and the client does not; closes ones the server dropped.
    fn reconciled(&self, server_ids: &[String], previous_server_ids: &[String]) -> Self {
        let normalized = self.normalized();
        let server: HashSet<&String> = server_ids.iter().collect();
        let added: Vec<String> = server_ids.iter().filter(|id| !normalized.terminal_ids.contains(id)).cloned().collect();
        let mut next = if added.is_empty() {
            normalized.clone()
        } else {
            Self {
                terminal_ids: normalized.terminal_ids.iter().cloned().chain(added).collect(),
                ..normalized.clone()
            }
            .normalized()
        };
        for id in previous_server_ids {
            if *id != normalized.active_terminal_id && !server.contains(id) {
                next = next.without(id);
            }
        }
        if !server.contains(&normalized.active_terminal_id) && previous_server_ids.contains(&normalized.active_terminal_id) {
            next = next.without(&normalized.active_terminal_id);
        }
        next
    }
}

/// The terminals a server listing reported for a thread.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerObservation {
    pub ids: Vec<String>,
    /// Identifies the server terminal manager; only string ids are persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
}

/// Every thread's terminal drawer state. A GPUI global; persist with [`Self::to_json`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalUiStateStore {
    #[serde(default, alias = "terminalStateByThreadKey")]
    terminal_ui_state_by_thread_key: HashMap<ThreadKey, ThreadTerminalUiState>,
    #[serde(default)]
    server_observations_by_thread_key: HashMap<ThreadKey, ServerObservation>,
    /// Ids the user closed locally, so a stale server listing does not bring them back.
    #[serde(skip)]
    suppressed_terminal_ids_by_thread_key: HashMap<ThreadKey, Vec<String>>,
}

impl Global for TerminalUiStateStore {}

/// Whether an update also marks a terminal id as closed (suppressed) or reopened.
enum Suppression<'a> {
    None,
    Suppress(&'a str),
    Unsuppress(&'a str),
}

impl TerminalUiStateStore {
    /// A thread's state, or the default (closed, no terminals).
    pub fn state(&self, thread: &ThreadKey) -> ThreadTerminalUiState {
        self.terminal_ui_state_by_thread_key.get(thread).cloned().unwrap_or_default()
    }

    fn update(&mut self, thread: &ThreadKey, suppression: Suppression, updater: impl FnOnce(&ThreadTerminalUiState) -> ThreadTerminalUiState) {
        let current = self.state(thread);
        let next = updater(&current);
        let was_listed = |id: &str| current.terminal_ids.iter().any(|known| known == id);
        match suppression {
            Suppression::None => {}
            Suppression::Suppress(id) => {
                let ids = self.suppressed_terminal_ids_by_thread_key.entry(thread.clone()).or_default();
                if !ids.iter().any(|known| known == id) {
                    ids.push(id.to_string());
                }
            }
            Suppression::Unsuppress(id) => {
                if let Some(ids) = self.suppressed_terminal_ids_by_thread_key.get_mut(thread) {
                    ids.retain(|known| known != id);
                    if ids.is_empty() {
                        self.suppressed_terminal_ids_by_thread_key.remove(thread);
                    }
                }
                // A listing from before this client placed the id is about an earlier terminal.
                if !was_listed(id) {
                    if let Some(observation) = self.server_observations_by_thread_key.get_mut(thread) {
                        observation.ids.retain(|known| known != id);
                        if observation.ids.is_empty() {
                            self.server_observations_by_thread_key.remove(thread);
                        }
                    }
                }
            }
        }
        if next.is_default() {
            self.terminal_ui_state_by_thread_key.remove(thread);
        } else {
            self.terminal_ui_state_by_thread_key.insert(thread.clone(), next);
        }
    }

    pub fn set_terminal_open(&mut self, thread: &ThreadKey, open: bool) {
        let creates = open && self.state(thread).terminal_ids.is_empty();
        let suppression = if creates { Suppression::Unsuppress(DEFAULT_THREAD_TERMINAL_ID) } else { Suppression::None };
        self.update(thread, suppression, |state| state.with_open(open));
    }

    pub fn set_terminal_height(&mut self, thread: &ThreadKey, height: f32) {
        self.update(thread, Suppression::None, |state| state.with_height(height));
    }

    pub fn split_terminal(&mut self, thread: &ThreadKey, terminal_id: &str, direction: SplitDirection) {
        self.update(thread, Suppression::Unsuppress(terminal_id), |state| state.with_split(terminal_id, direction));
    }

    pub fn new_terminal(&mut self, thread: &ThreadKey, terminal_id: &str) {
        self.update(thread, Suppression::Unsuppress(terminal_id), |state| state.with_new(terminal_id));
    }

    pub fn set_active_terminal(&mut self, thread: &ThreadKey, terminal_id: &str) {
        self.update(thread, Suppression::None, |state| state.with_active(terminal_id));
    }

    pub fn close_terminal(&mut self, thread: &ThreadKey, terminal_id: &str) {
        self.update(thread, Suppression::Suppress(terminal_id), |state| state.without(terminal_id));
    }

    /// Follows the server's terminal list for a thread (`subscribeTerminalMetadata`).
    pub fn reconcile_terminal_ids(&mut self, thread: &ThreadKey, server_ids: &[String], snapshot_id: Option<&str>) {
        let suppressed: HashSet<&String> = self.suppressed_terminal_ids_by_thread_key.get(thread).into_iter().flatten().collect();
        let visible: Vec<String> = server_ids.iter().filter(|id| !suppressed.contains(id)).cloned().collect();
        let previous = self.server_observations_by_thread_key.get(thread).cloned();
        // A different manager can start empty before old sessions reattach.
        let previous_ids = previous
            .as_ref()
            .filter(|observation| observation.snapshot_id.as_deref() == snapshot_id)
            .map(|observation| observation.ids.clone())
            .unwrap_or_default();
        self.update(thread, Suppression::None, |state| state.reconciled(&visible, &previous_ids));
        let mut observed = server_ids.to_vec();
        observed.sort();
        if observed.is_empty() {
            self.server_observations_by_thread_key.remove(thread);
        } else {
            self.server_observations_by_thread_key.insert(
                thread.clone(),
                ServerObservation { ids: observed, snapshot_id: snapshot_id.map(str::to_string) },
            );
        }
    }

    pub fn remove_terminal_ui_state(&mut self, thread: &ThreadKey) {
        self.terminal_ui_state_by_thread_key.remove(thread);
        self.suppressed_terminal_ids_by_thread_key.remove(thread);
        self.server_observations_by_thread_key.remove(thread);
    }

    /// Drops state for threads that no longer exist.
    pub fn remove_orphaned(&mut self, active: &HashSet<ThreadKey>) {
        self.terminal_ui_state_by_thread_key.retain(|key, _| active.contains(key));
        self.suppressed_terminal_ids_by_thread_key.retain(|key, _| active.contains(key));
        self.server_observations_by_thread_key.retain(|key, _| active.contains(key));
    }

    /// The persisted form: drawer states and observations with a string snapshot id.
    pub fn to_json(&self) -> serde_json::Result<String> {
        let mut persisted = self.clone();
        persisted.server_observations_by_thread_key.retain(|_, observation| observation.snapshot_id.is_some());
        serde_json::to_string(&persisted)
    }

    /// Restores persisted state, ignoring anything that does not parse.
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }
}
