//! Recently visited preview URLs per project. Ports the parts of
//! `browserHistoryStore.ts` the streamed preview uses: normalized URLs,
//! loopback hosts treated as one ("local"), titles from settled pages, 50
//! entries per project and 20 projects, persisted as JSON.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::Url;

/// Entries kept per project.
pub const MAX_ENTRIES_PER_PROJECT: usize = 50;
/// Projects kept; the least recently visited go first.
pub const MAX_PROJECTS: usize = 20;
const MAX_URL_LENGTH: usize = 2048;
const MAX_TITLE_LENGTH: usize = 512;

/// One visited page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserHistoryEntry {
    pub url: String,
    pub last_visited_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// `normalizePreviewUrl` then credentials dropped: a bare host gets `http://`.
pub fn normalize_history_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let with_scheme = if trimmed.contains("://") { trimmed.to_owned() } else { format!("http://{trimmed}") };
    let mut url = Url::parse(&with_scheme).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    _ = url.set_username("");
    _ = url.set_password(None);
    let href = url.to_string();
    (href.len() <= MAX_URL_LENGTH).then_some(href)
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_lowercase();
    host == "localhost" || host == "::1" || host == "0.0.0.0" || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.octets()[0] == 127)
}

/// The key two visits share when they are the same page: loopback hosts
/// and the environment's own host read as `local`.
fn visit_key(url: &str, environment_host: Option<&str>) -> String {
    let Ok(mut parsed) = Url::parse(url) else { return url.to_owned() };
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    if is_loopback(&host) || Some(host.as_str()) == environment_host {
        _ = parsed.set_host(Some("local"));
    }
    parsed.to_string()
}

/// Visits by project key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserHistory {
    pub by_project_key: BTreeMap<String, Vec<BrowserHistoryEntry>>,
}

impl BrowserHistory {
    /// Reads saved history; anything unreadable starts empty.
    pub fn read(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// Saves history; a failed write only loses it for the next launch.
    pub fn save(&self, path: &Path) {
        if let Ok(bytes) = serde_json::to_vec(self)
            && let Err(error) = std::fs::write(path, bytes)
        {
            tracing::warn!(%error, "could not save browser history");
        }
    }

    /// `upsertHistoryEntry`: the visit moves to the front; a stable local
    /// URL already stored is kept over a remote spelling of it.
    pub fn record_visit(&mut self, project: &str, url: &str, at: i64, environment_host: Option<&str>) {
        let Some(url) = normalize_history_url(url) else { return };
        let entries = self.by_project_key.entry(project.to_owned()).or_default();
        let key = visit_key(&url, environment_host);
        let existing = entries.iter().position(|e| visit_key(&e.url, environment_host) == key).map(|i| entries.remove(i));
        let stable = |u: &str| Url::parse(u).ok().and_then(|p| p.host_str().map(is_loopback)).unwrap_or(false);
        let entry = match existing {
            Some(existing) => BrowserHistoryEntry {
                url: if stable(&existing.url) || !stable(&url) { existing.url } else { url },
                last_visited_at: at,
                title: existing.title,
            },
            None => BrowserHistoryEntry { url, last_visited_at: at, title: None },
        };
        entries.insert(0, entry);
        entries.truncate(MAX_ENTRIES_PER_PROJECT);
        self.evict_projects();
    }

    /// Titles a visited page once it settles.
    pub fn set_title(&mut self, project: &str, url: &str, title: &str, environment_host: Option<&str>) {
        let (Some(url), Some(entries)) = (normalize_history_url(url), self.by_project_key.get_mut(project)) else { return };
        let key = visit_key(&url, environment_host);
        if let Some(entry) = entries.iter_mut().find(|e| visit_key(&e.url, environment_host) == key) {
            entry.title = Some(title.chars().take(MAX_TITLE_LENGTH).collect());
        }
    }

    /// The project's most recent visits.
    pub fn recent(&self, project: &str, limit: usize) -> &[BrowserHistoryEntry] {
        self.by_project_key.get(project).map_or(&[], |entries| &entries[..entries.len().min(limit)])
    }

    fn evict_projects(&mut self) {
        if self.by_project_key.len() <= MAX_PROJECTS {
            return;
        }
        let mut keys: Vec<(String, i64)> =
            self.by_project_key.iter().map(|(k, v)| (k.clone(), v.first().map_or(0, |e| e.last_visited_at))).collect();
        keys.sort_by(|a, b| b.1.cmp(&a.1));
        for (key, _) in keys.into_iter().skip(MAX_PROJECTS) {
            self.by_project_key.remove(&key);
        }
    }
}
