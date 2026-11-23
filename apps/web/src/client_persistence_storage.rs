//! Files the client keeps in its own data directory. Ports
//! `clientPersistenceStorage.ts` and the desktop's `DesktopClientSettings`
//! file handling: JSON documents written atomically (temporary file, then
//! rename) and debounced, plus [`LocalStorage`], the stand-in for the web's
//! `localStorage` keys that live outside client settings (themes, the
//! typography flag).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, Global, Task};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// How long a burst of changes waits before one write.
pub const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);

/// `client-settings.json`, same shape as the Electron app's file.
pub const CLIENT_SETTINGS_FILE: &str = "client-settings.json";
/// `desktop-settings.json`, only the keys that mean something here.
pub const DESKTOP_SETTINGS_FILE: &str = "desktop-settings.json";
/// The `localStorage` stand-in.
pub const LOCAL_STORAGE_FILE: &str = "local-storage.json";

/// Reads a JSON object from `path`. A missing file is `Ok(None)`; a file
/// that is not a JSON object is an error so defaults never overwrite it.
pub fn read_json_object(path: &Path) -> anyhow::Result<Option<Map<String, Value>>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match serde_json::from_slice(&bytes)? {
        Value::Object(map) => Ok(Some(map)),
        _ => anyhow::bail!("{} is not a JSON object", path.display()),
    }
}

/// Writes `value` as pretty JSON with a trailing newline, through a
/// temporary file in the same directory and a rename, so a crash leaves
/// either the old file or the new one.
pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let directory = target.parent().ok_or_else(|| anyhow::anyhow!("{} has no parent", target.display()))?;
    std::fs::create_dir_all(directory)?;
    let mut encoded = serde_json::to_vec_pretty(value)?;
    encoded.push(b'\n');
    let temporary = target.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = std::fs::write(&temporary, &encoded).and_then(|()| std::fs::rename(&temporary, &target));
    if result.is_err() {
        _ = std::fs::remove_file(&temporary);
    }
    Ok(result?)
}

/// A debounced writer for one file. Each [`Self::schedule`] replaces the
/// pending write, so a burst of changes costs one write.
pub struct DebouncedFile {
    path: PathBuf,
    pending: Option<Task<()>>,
}

impl DebouncedFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path, pending: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `value` after [`WRITE_DEBOUNCE`] on the background executor.
    pub fn schedule<T: Serialize + Send + 'static>(&mut self, value: T, cx: &App) {
        let path = self.path.clone();
        let executor = cx.background_executor().clone();
        self.pending = Some(cx.background_executor().spawn(async move {
            executor.timer(WRITE_DEBOUNCE).await;
            if let Err(error) = write_json_atomic(&path, &value) {
                tracing::warn!(%error, path = %path.display(), "could not save settings");
            }
        }));
    }

    /// Writes now and drops any pending write. Used at quit.
    pub fn flush(&mut self, value: &impl Serialize) {
        if self.pending.take().is_some()
            && let Err(error) = write_json_atomic(&self.path, value)
        {
            tracing::warn!(%error, path = %self.path.display(), "could not save settings");
        }
    }
}

/// The app's data directory for settings files, or `None` when the app
/// runs without persistence (scenes, scripted checks that opt out).
#[derive(Clone, Debug, Default)]
pub struct DataDir(pub Option<Arc<Path>>);

impl Global for DataDir {}

impl DataDir {
    pub fn file(cx: &App, name: &str) -> Option<PathBuf> {
        cx.try_global::<Self>().and_then(|dir| dir.0.as_ref().map(|dir| dir.join(name)))
    }
}

/// `localStorage` keys outside client settings, persisted to
/// `local-storage.json`. Values are JSON so each owner keeps its own type.
pub struct LocalStorage {
    values: Map<String, Value>,
    file: Option<DebouncedFile>,
}

impl Global for LocalStorage {}

impl LocalStorage {
    /// Loads from the data directory, or starts empty without one.
    pub fn init(cx: &mut App) {
        let path = DataDir::file(cx, LOCAL_STORAGE_FILE);
        let values = path
            .as_deref()
            .map(read_json_object)
            .transpose()
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "local storage unreadable; starting empty");
                None
            })
            .flatten()
            .unwrap_or_default();
        let first = !cx.has_global::<Self>();
        cx.set_global(Self { values, file: path.map(DebouncedFile::new) });
        if !first {
            return;
        }
        cx.on_app_quit(|cx| {
            let this = cx.global_mut::<Self>();
            let values = this.values.clone();
            if let Some(file) = &mut this.file {
                file.flush(&values);
            }
            async {}
        })
        .detach();
    }

    /// The value at `key`, decoded, or `None` when absent or malformed.
    pub fn get<T: DeserializeOwned>(cx: &App, key: &str) -> Option<T> {
        let value = cx.try_global::<Self>()?.values.get(key)?;
        T::deserialize(value).ok()
    }

    /// Stores `value` at `key` (`None` removes it) and schedules a write.
    pub fn set<T: Serialize>(cx: &mut App, key: &str, value: Option<&T>) {
        if !cx.has_global::<Self>() {
            return;
        }
        let encoded = value.and_then(|value| serde_json::to_value(value).ok());
        let this = cx.global::<Self>();
        if this.values.get(key) == encoded.as_ref() {
            return;
        }
        let mut values = this.values.clone();
        match encoded {
            Some(encoded) => values.insert(key.to_owned(), encoded),
            None => values.remove(key),
        };
        let snapshot = values.clone();
        let mut file = cx.global_mut::<Self>().file.take();
        if let Some(file) = &mut file {
            file.schedule(snapshot, cx);
        }
        cx.set_global(Self { values, file });
    }
}
