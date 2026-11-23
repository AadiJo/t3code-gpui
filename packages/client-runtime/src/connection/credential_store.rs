//! Route credentials in the secret store, keyed by connection id.

use std::sync::Arc;

use super::catalog::ConnectionCredential;
use crate::platform::{SecretStore, SecretStoreError};

/// Reads and writes [`ConnectionCredential`]s. Each secret store call runs
/// on the blocking pool, since keychains block.
#[derive(Clone)]
pub struct CredentialStore {
    secrets: Arc<dyn SecretStore>,
}

fn key(connection_id: &str) -> String {
    format!("credential:{connection_id}")
}

impl CredentialStore {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }

    pub async fn get(&self, connection_id: &str) -> Result<Option<ConnectionCredential>, SecretStoreError> {
        let secrets = self.secrets.clone();
        let key = key(connection_id);
        let bytes = tokio::task::spawn_blocking(move || secrets.get(&key))
            .await
            .expect("secret store task")?;
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| SecretStoreError::Corrupt(error.to_string())))
            .transpose()
    }

    pub async fn put(&self, connection_id: &str, credential: &ConnectionCredential) -> Result<(), SecretStoreError> {
        let secrets = self.secrets.clone();
        let key = key(connection_id);
        let bytes = serde_json::to_vec(credential).expect("credential serializes");
        tokio::task::spawn_blocking(move || secrets.set(&key, &bytes))
            .await
            .expect("secret store task")
    }

    pub async fn delete(&self, connection_id: &str) -> Result<(), SecretStoreError> {
        let secrets = self.secrets.clone();
        let key = key(connection_id);
        tokio::task::spawn_blocking(move || secrets.delete(&key))
            .await
            .expect("secret store task")
    }
}
