//! The saved inbox: `notifications.json` in the app's data directory, written
//! atomically after every change. The persistence half of
//! `ElectronNotifications.ts`, with no platform code, so the fixture driver
//! runs the same store the app does.

use std::io;
use std::path::{Path, PathBuf};

use gpui_kit::SharedString;
use t3code_web::desktop_notifications::{
    DesktopNotification, DesktopNotificationCommand, DesktopNotificationSnapshot,
};

use super::model::{NotificationState, read_notifications, reconcile_notifications};

/// `<data dir>/t3code-gpui/notifications.json`.
pub fn default_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("t3code-gpui").join("notifications.json"))
}

/// What a change did, for the host to mirror natively.
#[derive(Default)]
pub struct Commit {
    /// Entries that are no longer unread: remove their native notifications.
    pub removed: Vec<SharedString>,
    /// New unread entries to announce natively.
    pub added: Vec<DesktopNotification>,
    /// Whether anything changed.
    pub changed: bool,
}

/// The inbox state and the file it lives in.
pub struct InboxStore {
    path: PathBuf,
    state: NotificationState,
}

impl InboxStore {
    /// Loads the inbox from `path`. A missing file is an empty inbox; an
    /// unreadable one is an error, so a bad file is never silently overwritten.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let state = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => NotificationState::default(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, state })
    }

    /// Every entry, newest first.
    pub fn items(&self) -> &[DesktopNotification] {
        &self.state.items
    }

    /// The full state, as saved.
    pub fn state(&self) -> &NotificationState {
        &self.state
    }

    /// The file the inbox is saved to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reconciles a snapshot at `now` (ISO time) and saves the result.
    pub fn sync(&mut self, snapshot: &DesktopNotificationSnapshot, now: &SharedString) -> io::Result<Commit> {
        let result = reconcile_notifications(&self.state, snapshot, now);
        let mut commit = if result.changed { self.commit(result.state)? } else { Commit::default() };
        commit.added = result.added;
        Ok(commit)
    }

    /// Applies a read command and saves the result.
    pub fn update(&mut self, command: &DesktopNotificationCommand) -> io::Result<Commit> {
        match read_notifications(&self.state, command) {
            Some(next) => self.commit(next),
            None => Ok(Commit::default()),
        }
    }

    fn commit(&mut self, next: NotificationState) -> io::Result<Commit> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&next).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, &self.path)?;
        let unread: Vec<&SharedString> =
            next.items.iter().filter(|item| !item.read).map(|item| &item.id).collect();
        let removed = self
            .state
            .items
            .iter()
            .filter(|item| !unread.contains(&&item.id))
            .map(|item| item.id.clone())
            .collect();
        self.state = next;
        Ok(Commit { removed, added: Vec::new(), changed: true })
    }
}
