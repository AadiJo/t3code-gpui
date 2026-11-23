//! The saved environments and one supervisor per enabled environment.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Weak};

use tokio::sync::watch;

use super::catalog::{CatalogEntry, ConnectionCatalogDocument, ConnectionProfile, ConnectionRoute};
use super::driver::ConnectionContext;
use super::onboarding::PairedRoute;
use super::profile_store::CatalogStore;
use super::routes::{insert_route, merge_learned_routes, route_id, routes_after_removing};
use super::supervisor::{EnvironmentSupervisor, LearnRoutes, SupervisorHandle, SupervisorSignal};
use crate::environment::EnvironmentId;
use crate::platform::SecretStoreError;
use crate::platform::electron_import::ElectronImport;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("could not save the catalog: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Secrets(#[from] SecretStoreError),
}

struct Running {
    entry: watch::Sender<CatalogEntry>,
    supervisor: EnvironmentSupervisor,
}

struct Inner {
    context: Arc<ConnectionContext>,
    store: CatalogStore,
    document: tokio::sync::Mutex<ConnectionCatalogDocument>,
    entries: watch::Sender<Arc<[CatalogEntry]>>,
    running: std::sync::Mutex<HashMap<EnvironmentId, Running>>,
}

/// Owns the catalog and the supervisors. An involuntary disconnect keeps an
/// environment registered; only removal deletes its credentials.
#[derive(Clone)]
pub struct ConnectionRegistry {
    inner: Arc<Inner>,
}

impl ConnectionRegistry {
    /// Loads the catalog from `data_dir` and starts a supervisor, wanting a
    /// connection, for each enabled environment.
    pub async fn open(context: Arc<ConnectionContext>, data_dir: &Path) -> Result<Self, RegistryError> {
        let store = CatalogStore::new(data_dir);
        let document = store.load().await?;
        let (entries, _) = watch::channel(Arc::from(document.entries()));
        let registry = Self {
            inner: Arc::new(Inner {
                context,
                store,
                document: tokio::sync::Mutex::new(document),
                entries,
                running: Default::default(),
            }),
        };
        let entries = registry.inner.entries.borrow().clone();
        registry.sync(&entries);
        Ok(registry)
    }

    /// Every saved environment, enabled or not, in catalog order.
    pub fn entries(&self) -> watch::Receiver<Arc<[CatalogEntry]>> {
        self.inner.entries.subscribe()
    }

    /// The running supervisor of an enabled environment.
    pub fn supervisor(&self, environment_id: &EnvironmentId) -> Option<SupervisorHandle> {
        let running = self.inner.running.lock().expect("registry lock");
        running.get(environment_id).map(|running| running.supervisor.handle().clone())
    }

    /// Sends `signal` to every running supervisor (network change, wakeup).
    pub fn broadcast(&self, signal: SupervisorSignal) {
        for running in self.inner.running.lock().expect("registry lock").values() {
            running.supervisor.handle().signal(signal);
        }
    }

    /// Saves a paired route: its credential first, then the route, which
    /// replaces a route with the same id or goes first among its kind.
    pub async fn save_paired(&self, paired: &PairedRoute) -> Result<(), RegistryError> {
        let connection_id = route_id(&paired.route.target).to_owned();
        self.inner.context.auth.credentials().put(&connection_id, &paired.credential).await?;
        self.edit(|document| {
            let mut routes = existing_routes(document, &paired.environment_id);
            match routes.iter().position(|route| route_id(&route.target) == connection_id) {
                Some(index) => routes[index] = paired.route.clone(),
                None => insert_route(&mut routes, paired.route.clone()),
            }
            document.put_entry(&paired.environment_id, &routes);
        })
        .await
    }

    /// Saves a T3 Connect route for an environment found through the relay.
    pub async fn save_relay(&self, route: ConnectionRoute) -> Result<(), RegistryError> {
        let environment_id = route.target.environment_id.clone();
        self.edit(|document| {
            let mut routes = existing_routes(document, &environment_id);
            if !routes.iter().any(|existing| existing.target.route == route.target.route) {
                insert_route(&mut routes, route);
            }
            document.put_entry(&environment_id, &routes);
        })
        .await
    }

    /// Sets an environment's route order. Ids not in `order` keep their place at the end.
    pub async fn reorder_routes(&self, environment_id: &EnvironmentId, order: &[String]) -> Result<(), RegistryError> {
        self.edit(|document| {
            let mut routes = existing_routes(document, environment_id);
            routes.sort_by_key(|route| order.iter().position(|id| id == route_id(&route.target)).unwrap_or(usize::MAX));
            document.put_entry(environment_id, &routes);
        })
        .await
    }

    /// Removes one route, and learned routes that depend on it. Removing the
    /// last route removes the environment.
    pub async fn remove_route(&self, environment_id: &EnvironmentId, removed_id: &str) -> Result<(), RegistryError> {
        let mut orphaned = Vec::new();
        self.edit(|document| {
            let before = existing_routes(document, environment_id);
            let after = routes_after_removing(&before, removed_id);
            orphaned = before
                .iter()
                .filter(|route| !after.contains(route))
                .map(|route| route_id(&route.target).to_owned())
                .collect();
            if after.is_empty() {
                document.remove_entry(environment_id);
            } else {
                document.put_entry(environment_id, &after);
            }
        })
        .await?;
        for id in orphaned {
            self.inner.context.auth.credentials().delete(&id).await?;
        }
        Ok(())
    }

    /// Removes an environment, its supervisor, and its credentials.
    pub async fn remove_environment(&self, environment_id: &EnvironmentId) -> Result<(), RegistryError> {
        let mut removed = Vec::new();
        self.edit(|document| removed = document.remove_entry(environment_id)).await?;
        for id in removed {
            self.inner.context.auth.credentials().delete(&id).await?;
        }
        Ok(())
    }

    /// Renames an environment on this device. The host keeps its own name.
    pub async fn rename(&self, environment_id: &EnvironmentId, label: &str) -> Result<(), RegistryError> {
        let label = label.trim().to_owned();
        if label.is_empty() {
            return Ok(());
        }
        self.edit(|document| {
            let mut routes = existing_routes(document, environment_id);
            for route in &mut routes {
                route.target.label = label.clone();
                match &mut route.profile {
                    Some(ConnectionProfile::Bearer(profile)) => profile.label = label.clone(),
                    Some(ConnectionProfile::Ssh(profile)) => profile.label = label.clone(),
                    None => {}
                }
            }
            if !routes.is_empty() {
                document.put_entry(environment_id, &routes);
            }
        })
        .await
    }

    /// Adds environments read from the Electron app: credentials first, then
    /// routes this catalog does not have yet. Returns how many routes it added.
    pub async fn import(&self, import: &ElectronImport) -> Result<usize, RegistryError> {
        for (connection_id, credential) in &import.credentials {
            self.inner.context.auth.credentials().put(connection_id, credential).await?;
        }
        let mut added = 0;
        self.edit(|document| {
            for environment in &import.environments {
                let existed = document.entries().iter().any(|entry| entry.environment_id == environment.environment_id);
                let mut routes = existing_routes(document, &environment.environment_id);
                for route in &environment.routes {
                    if !routes.iter().any(|existing| route_id(&existing.target) == route_id(&route.target)) {
                        routes.push(route.clone());
                        added += 1;
                    }
                }
                document.put_entry(&environment.environment_id, &routes);
                if !existed && !environment.enabled {
                    document.set_enabled(&environment.environment_id, false);
                }
            }
        })
        .await?;
        Ok(added)
    }

    /// Switches an environment on or off. Off stops its supervisor.
    pub async fn set_enabled(&self, environment_id: &EnvironmentId, enabled: bool) -> Result<(), RegistryError> {
        self.edit(|document| document.set_enabled(environment_id, enabled)).await
    }

    async fn edit(&self, change: impl FnOnce(&mut ConnectionCatalogDocument)) -> Result<(), RegistryError> {
        let mut document = self.inner.document.lock().await;
        change(&mut document);
        self.inner.store.save(&document).await?;
        let entries: Arc<[CatalogEntry]> = Arc::from(document.entries());
        drop(document);
        // Supervisors start before observers see the entries, so an entry
        // that reads as enabled always has its supervisor.
        self.sync(&entries);
        self.inner.entries.send_replace(entries);
        Ok(())
    }

    /// Starts, updates and stops supervisors to match the entries.
    fn sync(&self, entries: &[CatalogEntry]) {
        let mut running = self.inner.running.lock().expect("registry lock");
        running.retain(|id, _| entries.iter().any(|entry| entry.environment_id == *id && entry.enabled));
        for entry in entries.iter().filter(|entry| entry.enabled) {
            match running.get(&entry.environment_id) {
                Some(existing) => {
                    existing.entry.send_if_modified(|current| {
                        let changed = current != entry;
                        if changed {
                            *current = entry.clone();
                        }
                        changed
                    });
                }
                None => {
                    let (entry_tx, entry_rx) = watch::channel(entry.clone());
                    let supervisor = EnvironmentSupervisor::spawn(
                        self.inner.context.clone(),
                        entry_rx,
                        true,
                        Some(learn_routes(Arc::downgrade(&self.inner))),
                    );
                    running.insert(entry.environment_id.clone(), Running { entry: entry_tx, supervisor });
                }
            }
        }
    }
}

fn existing_routes(document: &ConnectionCatalogDocument, environment_id: &EnvironmentId) -> Vec<ConnectionRoute> {
    document
        .entries()
        .into_iter()
        .find(|entry| entry.environment_id == *environment_id)
        .map(|entry| entry.routes.to_vec())
        .unwrap_or_default()
}

/// Merges a connected server's reported addresses into its routes.
fn learn_routes(inner: Weak<Inner>) -> LearnRoutes {
    Arc::new(move |entry, active, reported| {
        let Some(next) = merge_learned_routes(entry, active, reported) else { return };
        let Some(inner) = inner.upgrade() else { return };
        let environment_id = entry.environment_id.clone();
        tokio::spawn(async move {
            let registry = ConnectionRegistry { inner };
            if let Err(error) = registry.edit(|document| document.put_entry(&environment_id, &next)).await {
                tracing::warn!(%error, "could not save learned routes");
            }
        });
    })
}
