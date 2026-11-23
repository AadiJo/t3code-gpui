//! Persists the catalog document as JSON in the app's own data directory.

use std::path::PathBuf;

use super::catalog::ConnectionCatalogDocument;

/// The catalog file at `<data_dir>/connection-catalog.json`. Writes are
/// atomic (temp file, then rename).
pub struct CatalogStore {
    path: PathBuf,
}

impl CatalogStore {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self { path: data_dir.join("connection-catalog.json") }
    }

    /// Reads the catalog; a missing file is an empty catalog. An unreadable
    /// file is set aside as `.unreadable-<ms>` rather than lost.
    pub async fn load(&self) -> std::io::Result<ConnectionCatalogDocument> {
        match tokio::fs::read(&self.path).await {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(document) => Ok(document),
                Err(error) => {
                    let millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let aside = self.path.with_extension(format!("json.unreadable-{millis}"));
                    tracing::warn!(%error, path = %aside.display(), "catalog unreadable, setting it aside");
                    tokio::fs::rename(&self.path, &aside).await?;
                    Ok(ConnectionCatalogDocument::default())
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConnectionCatalogDocument::default()),
            Err(error) => Err(error),
        }
    }

    pub async fn save(&self, document: &ConnectionCatalogDocument) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(document).expect("catalog serializes");
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &self.path).await
    }
}
