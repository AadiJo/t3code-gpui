//! Saved environments: targets, route profiles, credentials, and the
//! on-disk catalog document. JSON shapes follow the TypeScript catalog
//! (`_tag` variants, camelCase), so an imported desktop catalog decodes.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use crate::environment::EnvironmentId;

/// How one route reaches the environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum TargetRoute {
    /// A direct address with a profile and a stored or borrowed credential.
    #[serde(rename = "BearerConnectionTarget")]
    Bearer {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
    /// T3 Connect: authorized through the relay, no stored credential.
    #[serde(rename = "RelayConnectionTarget")]
    Relay,
    /// Launched and forwarded over SSH. Not connectable from this client yet.
    #[serde(rename = "SshConnectionTarget")]
    Ssh {
        #[serde(rename = "connectionId")]
        connection_id: String,
    },
}

/// One saved route's identity, in priority order inside its environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTarget {
    pub environment_id: EnvironmentId,
    pub label: String,
    #[serde(flatten)]
    pub route: TargetRoute,
}

/// Set on a learned route whose token comes from T3 Connect, not a stored credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileAuthorization {
    #[serde(rename = "t3-connect")]
    T3Connect,
}

/// Set when the server confirmed the address is on its Tailscale interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileNetwork {
    #[serde(rename = "tailscale")]
    Tailscale,
}

/// A direct route's address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BearerProfile {
    pub connection_id: String,
    pub environment_id: EnvironmentId,
    pub label: String,
    pub http_base_url: Url,
    pub ws_base_url: Url,
    /// Learned from the server's `directEndpoints`, not added by the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub learned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<ProfileAuthorization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<ProfileNetwork>,
}

/// An SSH route's host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshTarget {
    pub alias: String,
    pub hostname: String,
    pub username: Option<String>,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshProfile {
    pub connection_id: String,
    pub environment_id: EnvironmentId,
    pub label: String,
    pub target: SshTarget,
}

/// Address details for a route, keyed by its connection id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ConnectionProfile {
    #[serde(rename = "BearerConnectionProfile")]
    Bearer(BearerProfile),
    #[serde(rename = "SshConnectionProfile")]
    Ssh(SshProfile),
}

impl ConnectionProfile {
    pub fn connection_id(&self) -> &str {
        match self {
            Self::Bearer(profile) => &profile.connection_id,
            Self::Ssh(profile) => &profile.connection_id,
        }
    }
}

/// A route: its target and, for direct and SSH routes, its profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionRoute {
    pub target: ConnectionTarget,
    pub profile: Option<ConnectionProfile>,
}

impl ConnectionRoute {
    pub fn bearer_profile(&self) -> Option<&BearerProfile> {
        match &self.profile {
            Some(ConnectionProfile::Bearer(profile)) => Some(profile),
            _ => None,
        }
    }
}

/// One saved environment and its routes in priority order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub environment_id: EnvironmentId,
    pub label: String,
    /// Never empty. Order is priority; the user can reorder.
    pub routes: Arc<[ConnectionRoute]>,
    pub enabled: bool,
}

/// A stored secret for a direct route. Lives in the secret store, never in
/// the catalog file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum ConnectionCredential {
    /// A plain session token, not bound to a key.
    #[serde(rename = "BearerConnectionCredential")]
    Bearer { token: String },
    /// A token bound to this device's DPoP key, renewed with the host.
    #[serde(rename = "DpopConnectionCredential")]
    Dpop {
        #[serde(rename = "accessToken")]
        access_token: String,
        #[serde(rename = "expiresAtEpochMs")]
        expires_at_epoch_ms: i64,
    },
}

/// The catalog file. Ordered targets (contiguous per environment), their
/// profiles, and the environments switched off. Credentials are not here.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCatalogDocument {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub targets: Vec<ConnectionTarget>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub profiles: Vec<ConnectionProfile>,
    #[serde(default)]
    pub disabled_environment_ids: Vec<EnvironmentId>,
}

fn schema_version() -> u32 {
    1
}

/// Drops elements that do not decode (a target kind this client does not
/// know) instead of failing the whole catalog.
fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(item) => Some(item),
            Err(error) => {
                tracing::warn!(%error, "skipping a catalog item this client cannot read");
                None
            }
        })
        .collect())
}

impl ConnectionCatalogDocument {
    /// Groups targets into entries, in first-seen order.
    pub fn entries(&self) -> Vec<CatalogEntry> {
        let mut entries: Vec<(EnvironmentId, String, Vec<ConnectionRoute>)> = Vec::new();
        for target in &self.targets {
            let profile = match &target.route {
                TargetRoute::Bearer { connection_id } | TargetRoute::Ssh { connection_id } => self
                    .profiles
                    .iter()
                    .find(|profile| profile.connection_id() == connection_id)
                    .cloned(),
                TargetRoute::Relay => None,
            };
            let route = ConnectionRoute { target: target.clone(), profile };
            match entries.iter_mut().find(|(id, ..)| *id == target.environment_id) {
                Some((_, _, routes)) => routes.push(route),
                None => entries.push((target.environment_id.clone(), target.label.clone(), vec![route])),
            }
        }
        entries
            .into_iter()
            .map(|(environment_id, label, routes)| CatalogEntry {
                enabled: !self.disabled_environment_ids.contains(&environment_id),
                environment_id,
                label,
                routes: routes.into(),
            })
            .collect()
    }

    /// Replaces an environment's routes, keeping its position, or appends it.
    pub fn put_entry(&mut self, environment_id: &EnvironmentId, routes: &[ConnectionRoute]) {
        let position = self
            .targets
            .iter()
            .position(|target| target.environment_id == *environment_id)
            .unwrap_or(self.targets.len());
        self.remove_entry_routes(environment_id);
        let position = position.min(self.targets.len());
        self.targets
            .splice(position..position, routes.iter().map(|route| route.target.clone()));
        for route in routes {
            if let Some(profile) = &route.profile {
                self.profiles.retain(|existing| existing.connection_id() != profile.connection_id());
                self.profiles.push(profile.clone());
            }
        }
    }

    /// Removes an environment entirely. Returns the connection ids whose
    /// credentials the caller should delete.
    pub fn remove_entry(&mut self, environment_id: &EnvironmentId) -> Vec<String> {
        let removed = self.remove_entry_routes(environment_id);
        self.disabled_environment_ids.retain(|id| id != environment_id);
        removed
    }

    fn remove_entry_routes(&mut self, environment_id: &EnvironmentId) -> Vec<String> {
        let ids: Vec<String> = self
            .targets
            .iter()
            .filter(|target| target.environment_id == *environment_id)
            .filter_map(|target| match &target.route {
                TargetRoute::Bearer { connection_id } | TargetRoute::Ssh { connection_id } => {
                    Some(connection_id.clone())
                }
                TargetRoute::Relay => None,
            })
            .collect();
        self.targets.retain(|target| target.environment_id != *environment_id);
        self.profiles.retain(|profile| !ids.iter().any(|id| id == profile.connection_id()));
        ids
    }

    pub fn set_enabled(&mut self, environment_id: &EnvironmentId, enabled: bool) {
        self.disabled_environment_ids.retain(|id| id != environment_id);
        if !enabled {
            self.disabled_environment_ids.push(environment_id.clone());
        }
    }
}
