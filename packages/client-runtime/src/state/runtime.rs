//! The state of every enabled environment (`environmentStreams.ts`, `connections.ts`). Follows
//! the connection registry: an enabled environment gets a store, a disabled one loses it but
//! keeps its caches, and a removed one also loses its caches.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use t3code_contracts::EnvironmentId;
use tokio::sync::watch;

use super::cache_store::CacheStore;
use super::environment::{EnvironmentContext, EnvironmentStore};
use crate::authorization::RemoteAuthorization;
use crate::connection::ConnectionRegistry;

/// Every enabled environment's store, in catalog order.
pub type Stores = Arc<Vec<EnvironmentStore>>;

/// Owns the per-environment stores. Dropping it stops them.
pub struct StateRuntime {
    stores: watch::Sender<Stores>,
    cache: CacheStore,
    task: tokio::task::AbortHandle,
    data_dir: PathBuf,
}

impl Drop for StateRuntime {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl StateRuntime {
    /// Starts following `registry`. Must run inside a tokio runtime.
    pub fn start(registry: ConnectionRegistry, auth: Arc<RemoteAuthorization>, data_dir: &Path) -> Self {
        let cache = CacheStore::new(data_dir);
        let stores = watch::Sender::new(Stores::default());
        let task = tokio::spawn(follow_registry(registry, auth, cache.clone(), stores.clone())).abort_handle();
        Self { stores, cache, task, data_dir: data_dir.to_owned() }
    }

    /// The stores, and changes to the set.
    pub fn stores(&self) -> watch::Receiver<Stores> {
        self.stores.subscribe()
    }

    /// The store of one environment, if enabled.
    pub fn store(&self, environment_id: &EnvironmentId) -> Option<EnvironmentStore> {
        self.stores.borrow().iter().find(|store| store.environment_id() == environment_id).cloned()
    }

    /// Deletes an environment's caches; call after removing it from the registry.
    pub fn forget_environment(&self, environment_id: &EnvironmentId) {
        self.cache.remove_environment(environment_id);
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

async fn follow_registry(registry: ConnectionRegistry, auth: Arc<RemoteAuthorization>, cache: CacheStore, stores: watch::Sender<Stores>) {
    let mut entries = registry.entries();
    let runtime = tokio::runtime::Handle::current();
    loop {
        // The registry starts supervisors right after publishing entries.
        tokio::task::yield_now().await;
        let catalog = entries.borrow_and_update().clone();
        let current = stores.borrow().clone();
        let mut next = Vec::new();
        let mut missing = false;
        for entry in catalog.iter().filter(|entry| entry.enabled) {
            if let Some(store) = current.iter().find(|store| store.environment_id() == &entry.environment_id) {
                next.push(store.clone());
                continue;
            }
            let Some(supervisor) = registry.supervisor(&entry.environment_id) else {
                missing = true;
                continue;
            };
            next.push(EnvironmentStore::open(EnvironmentContext {
                environment_id: entry.environment_id.clone(),
                supervisor,
                auth: Arc::clone(&auth),
                cache: cache.clone(),
                runtime: runtime.clone(),
            }));
        }
        let changed = next.len() != current.len() || next.iter().zip(current.iter()).any(|(a, b)| a.environment_id() != b.environment_id());
        if changed {
            stores.send_replace(Arc::new(next));
        }
        if missing {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        if entries.changed().await.is_err() {
            return;
        }
    }
}
