use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Keychain service name for every secret this app stores.
const KEYCHAIN_SERVICE: &str = "t3code-gpui";

#[derive(Debug, thiserror::Error)]
pub enum SecretStoreError {
    #[error("secret store unavailable: {0}")]
    Unavailable(String),
    #[error("secret store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("secret store file is corrupt: {0}")]
    Corrupt(String),
}

/// Byte secrets keyed by a short name: credentials, the DPoP key. Calls block,
/// so async callers run them on `spawn_blocking`.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretStoreError>;
    fn delete(&self, key: &str) -> Result<(), SecretStoreError>;
}

/// The OS keystore: Keychain, Credential Manager, or Secret Service.
pub struct KeychainSecretStore;

impl KeychainSecretStore {
    /// Fails when the platform has no usable keystore, for example Linux
    /// without a Secret Service on the session bus.
    pub fn open() -> Result<Self, SecretStoreError> {
        keyring::Entry::store_status()
            .as_ref()
            .map_err(|error| SecretStoreError::Unavailable(error.to_string()))?;
        // A store can load and still have no running daemon, so probe once.
        let probe = keyring::Entry::new(KEYCHAIN_SERVICE, "__probe").map_err(unavailable)?;
        match probe.get_secret() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(Self),
            Err(error) => Err(unavailable(error)),
        }
    }

    fn entry(key: &str) -> Result<keyring::Entry, SecretStoreError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(unavailable)
    }
}

fn unavailable(error: keyring::Error) -> SecretStoreError {
    SecretStoreError::Unavailable(error.to_string())
}

impl SecretStore for KeychainSecretStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match Self::entry(key)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(unavailable(error)),
        }
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        Self::entry(key)?.set_secret(value).map_err(unavailable)
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(unavailable(error)),
        }
    }
}

/// Fallback for machines without a keystore: one JSON map of base64 values in
/// a file only this user can read (mode 0600, directory 0700).
pub struct FileSecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path, lock: Mutex::new(()) }
    }

    fn read(&self) -> Result<BTreeMap<String, String>, SecretStoreError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| SecretStoreError::Corrupt(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }

    fn write(&self, map: &BTreeMap<String, String>) -> Result<(), SecretStoreError> {
        let dir = self.path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(dir)?;
        restrict_dir(dir)?;
        let tmp = self.path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec(map).expect("string map serializes"))?;
        file.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

impl SecretStore for FileSecretStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        let _guard = self.lock.lock().expect("secret file lock");
        self.read()?
            .get(key)
            .map(|value| STANDARD.decode(value).map_err(|error| SecretStoreError::Corrupt(error.to_string())))
            .transpose()
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        let _guard = self.lock.lock().expect("secret file lock");
        let mut map = self.read()?;
        map.insert(key.to_owned(), STANDARD.encode(value));
        self.write(&map)
    }

    fn delete(&self, key: &str) -> Result<(), SecretStoreError> {
        let _guard = self.lock.lock().expect("secret file lock");
        let mut map = self.read()?;
        if map.remove(key).is_some() {
            self.write(&map)?;
        }
        Ok(())
    }
}

/// The OS keystore when it works, else the 0600 file at `<data_dir>/secrets.json`.
pub fn open_secret_store(data_dir: &std::path::Path) -> Arc<dyn SecretStore> {
    match KeychainSecretStore::open() {
        Ok(store) => Arc::new(store),
        Err(error) => {
            tracing::warn!(%error, "OS keystore unavailable, storing secrets in a 0600 file");
            Arc::new(FileSecretStore::new(data_dir.join("secrets.json")))
        }
    }
}
