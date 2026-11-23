//! Client-owned documents in the app's data directory, replacing the web's localStorage:
//! composer drafts and UI state. Each is one JSON file with a schema version; a version change
//! or a file that does not decode starts empty. Writes follow the cache policy (500 ms after the
//! first change, then at most every 10 s) and flush when the store is dropped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::cache_persistence::Persister;

/// A persisted client document type.
pub trait ClientDocument: Serialize + DeserializeOwned + Default + Clone + Send + Sync + 'static {
    /// The file name under `client-state/`.
    const FILE: &'static str;
    /// Bump to discard old files.
    const SCHEMA_VERSION: u32;
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<T> {
    schema_version: u32,
    document: T,
}

/// One persisted document, readable through a watch channel. Edits publish at once and save on
/// the write schedule. Needs a tokio runtime when created.
pub struct ClientStore<T: ClientDocument> {
    value: watch::Sender<Arc<T>>,
    persister: Persister<Arc<T>>,
}

fn path_of<T: ClientDocument>(data_dir: &Path) -> PathBuf {
    data_dir.join("client-state").join(T::FILE)
}

impl<T: ClientDocument> ClientStore<T> {
    /// Loads the document from `data_dir`, or starts empty.
    pub fn open(data_dir: &Path) -> Self {
        let path = path_of::<T>(data_dir);
        let loaded = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Envelope<T>>(&bytes).ok())
            .filter(|envelope| envelope.schema_version == T::SCHEMA_VERSION)
            .map(|envelope| envelope.document)
            .unwrap_or_default();
        let persister = Persister::spawn(move |document: Arc<T>| {
            let bytes = serde_json::to_vec(&Envelope { schema_version: T::SCHEMA_VERSION, document: &*document }).expect("client document encodes");
            if let Some(parent) = path.parent() {
                _ = std::fs::create_dir_all(parent);
            }
            let temp = path.with_extension("tmp");
            if let Err(error) = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, &path)) {
                tracing::warn!(file = T::FILE, %error, "could not save a client document");
            }
        });
        Self { value: watch::Sender::new(Arc::new(loaded)), persister }
    }

    /// The current document and changes to it.
    pub fn subscribe(&self) -> watch::Receiver<Arc<T>> {
        self.value.subscribe()
    }

    pub fn current(&self) -> Arc<T> {
        self.value.borrow().clone()
    }

    /// Edits the document; it publishes now and saves on the write schedule.
    pub fn update(&self, edit: impl FnOnce(&mut T)) {
        let mut next = (*self.current()).clone();
        edit(&mut next);
        let next = Arc::new(next);
        self.value.send_replace(Arc::clone(&next));
        self.persister.offer(next);
    }

    /// Saves now.
    pub fn flush(&self) {
        self.persister.flush();
    }
}
