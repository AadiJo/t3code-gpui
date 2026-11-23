//! One-shot, read-only import of the Electron desktop app's saved
//! environments (spec 02, sections 1.3 to 1.5). Nothing here writes to the
//! Electron files: the renderer caches the catalog and overwrites it whole.
//!
//! Electron's `safeStorage` values are Chromium `OSCrypt` output, `v10` plus
//! a payload. Windows: AES-256-GCM with a DPAPI-wrapped key from `Local
//! State`. macOS: AES-128-CBC with a key derived from a Keychain password.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;

use crate::connection::catalog::{
    ConnectionCatalogDocument, ConnectionCredential, ConnectionProfile, ConnectionRoute, TargetRoute,
};
use crate::connection::routes::{credential_connection_id, is_learned, route_id};
use crate::environment::EnvironmentId;

/// Every safeStorage value starts with this version tag.
const OSCRYPT_V10: &[u8] = b"v10";

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("no desktop app data was found at {0}")]
    NotFound(PathBuf),
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("the desktop app's encryption key is unavailable: {0}")]
    KeyUnavailable(String),
    #[error("this platform keeps the desktop app's key somewhere this app cannot read")]
    UnsupportedPlatform,
    #[error("the desktop app's data did not decrypt; it may use another key or format")]
    Decrypt,
    #[error("the desktop app's catalog did not parse: {0}")]
    Parse(String),
}

/// The key that decrypts the Electron app's safeStorage values.
#[derive(Clone)]
pub enum SafeStorageKey {
    /// Windows: the 32-byte key from `Local State`, after DPAPI.
    WindowsGcm([u8; 32]),
    /// macOS: the 16-byte key derived from the Keychain password.
    MacCbc([u8; 16]),
}

impl SafeStorageKey {
    /// macOS derives the key from the Keychain password with Chromium's fixed parameters.
    pub fn from_mac_password(password: &[u8]) -> Self {
        let mut key = [0u8; 16];
        ring::pbkdf2::derive(
            ring::pbkdf2::PBKDF2_HMAC_SHA1,
            std::num::NonZeroU32::new(1003).expect("non-zero"),
            b"saltysalt",
            password,
            &mut key,
        );
        Self::MacCbc(key)
    }
}

/// Where the Electron app keeps its files.
#[derive(Clone, Debug)]
pub struct ElectronPaths {
    /// The T3 home `stateDir`: `connection-catalog.json` and `clerk/clerk-tokens.json`.
    pub state_dir: PathBuf,
    /// Chromium's `userData`: `Local State` on Windows.
    pub user_data_dir: PathBuf,
}

impl ElectronPaths {
    /// The production desktop app's paths for this user (`$T3CODE_HOME` respected).
    pub fn detect() -> Option<Self> {
        let state_dir = match std::env::var_os("T3CODE_HOME") {
            Some(home) => PathBuf::from(home).join("userdata"),
            None => dirs::home_dir()?.join(".t3").join("userdata"),
        };
        let user_data_dir = if cfg!(target_os = "macos") {
            dirs::home_dir()?.join("Library/Application Support/t3code-v2")
        } else {
            dirs::config_dir()?.join("t3code-v2")
        };
        Some(Self { state_dir, user_data_dir })
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.state_dir.join("connection-catalog.json")
    }

    pub fn clerk_tokens_path(&self) -> PathBuf {
        self.state_dir.join("clerk").join("clerk-tokens.json")
    }
}

/// Reads the OS-held key. Windows unwraps `os_crypt.encrypted_key` with
/// DPAPI; macOS reads the `T3 Code Safe Storage` Keychain item, which shows
/// the user a Keychain prompt. Unverified against real installs.
pub fn load_platform_key(paths: &ElectronPaths) -> Result<SafeStorageKey, ImportError> {
    #[cfg(windows)]
    {
        windows_key(paths)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = paths;
        mac_key()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = paths;
        Err(ImportError::UnsupportedPlatform)
    }
}

#[cfg(windows)]
fn windows_key(paths: &ElectronPaths) -> Result<SafeStorageKey, ImportError> {
    #[derive(Deserialize)]
    struct LocalState {
        os_crypt: OsCrypt,
    }
    #[derive(Deserialize)]
    struct OsCrypt {
        encrypted_key: String,
    }
    let path = paths.user_data_dir.join("Local State");
    let bytes = read(&path)?;
    let state: LocalState = serde_json::from_slice(&bytes).map_err(|error| ImportError::Parse(error.to_string()))?;
    let wrapped = STANDARD.decode(state.os_crypt.encrypted_key).map_err(|_| ImportError::Decrypt)?;
    let wrapped = wrapped.strip_prefix(b"DPAPI").ok_or(ImportError::Decrypt)?;
    let key = dpapi_unprotect(wrapped)?;
    let key: [u8; 32] = key.try_into().map_err(|_| ImportError::Decrypt)?;
    Ok(SafeStorageKey::WindowsGcm(key))
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>, ImportError> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: `input` borrows `data` for the call; Windows allocates `output`,
    // which is copied out and freed with LocalFree.
    unsafe {
        let ok = CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut output,
        );
        if ok == 0 {
            return Err(ImportError::KeyUnavailable(std::io::Error::last_os_error().to_string()));
        }
        let key = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Ok(key)
    }
}

#[cfg(target_os = "macos")]
fn mac_key() -> Result<SafeStorageKey, ImportError> {
    let mut last = None;
    for (service, account) in [("T3 Code Safe Storage", "T3 Code Key"), ("T3 Code Safe Storage", "T3 Code")] {
        match keyring::Entry::new(service, account).and_then(|entry| entry.get_secret()) {
            Ok(password) => return Ok(SafeStorageKey::from_mac_password(&password)),
            Err(error) => last = Some(error.to_string()),
        }
    }
    Err(ImportError::KeyUnavailable(last.unwrap_or_default()))
}

/// Decrypts one safeStorage value (`v10` + payload).
pub fn decrypt_safe_storage(value: &[u8], key: &SafeStorageKey) -> Result<Vec<u8>, ImportError> {
    let payload = value.strip_prefix(OSCRYPT_V10).ok_or(ImportError::Decrypt)?;
    match key {
        SafeStorageKey::WindowsGcm(key) => {
            use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
            if payload.len() < 12 + 16 {
                return Err(ImportError::Decrypt);
            }
            let (nonce, sealed) = payload.split_at(12);
            let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| ImportError::Decrypt)?);
            let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| ImportError::Decrypt)?;
            let mut buffer = sealed.to_vec();
            let plain = key.open_in_place(nonce, Aad::empty(), &mut buffer).map_err(|_| ImportError::Decrypt)?;
            Ok(plain.to_vec())
        }
        SafeStorageKey::MacCbc(key) => {
            use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
            let iv = [b' '; 16];
            cbc::Decryptor::<aes::Aes128>::new(key.into(), &iv.into())
                .decrypt_padded_vec_mut::<Pkcs7>(payload)
                .map_err(|_| ImportError::Decrypt)
        }
    }
}

/// Encrypts like safeStorage. Only for building test fixtures.
pub fn encrypt_safe_storage(plain: &[u8], key: &SafeStorageKey, nonce: [u8; 12]) -> Vec<u8> {
    let mut out = OSCRYPT_V10.to_vec();
    match key {
        SafeStorageKey::WindowsGcm(key) => {
            use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
            let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("32-byte key"));
            let mut buffer = plain.to_vec();
            key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buffer)
                .expect("seal");
            out.extend_from_slice(&nonce);
            out.extend_from_slice(&buffer);
        }
        SafeStorageKey::MacCbc(key) => {
            use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
            let iv = [b' '; 16];
            out.extend(cbc::Encryptor::<aes::Aes128>::new(key.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plain));
        }
    }
    out
}

fn read(path: &Path) -> Result<Vec<u8>, ImportError> {
    std::fs::read(path).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => ImportError::NotFound(path.to_owned()),
        _ => ImportError::Read { path: path.to_owned(), source },
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEnvelope {
    encrypted_catalog: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ElectronCredentials {
    #[serde(default)]
    credentials: Vec<ElectronCredentialRecord>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ElectronCredentialRecord {
    connection_id: String,
    credential: serde_json::Value,
}

/// A route that could not carry over and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedRoute {
    pub environment_label: String,
    /// The route's address, or its connection id when it has none.
    pub address: String,
    pub reason: SkipReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Paired with the desktop's DPoP key, which never leaves Chromium. Pair again.
    DeviceBoundCredential,
    /// No credential was saved for it. Pair again.
    MissingCredential,
    /// SSH routes need the desktop's SSH launcher, which this app does not have.
    Ssh,
}

/// One environment to add to this app's catalog.
#[derive(Clone, Debug)]
pub struct ImportedEnvironment {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub routes: Vec<ConnectionRoute>,
    pub enabled: bool,
}

/// What the desktop catalog holds that this app can use, and what it cannot.
#[derive(Clone, Debug, Default)]
pub struct ElectronImport {
    pub environments: Vec<ImportedEnvironment>,
    /// Plain bearer credentials, by connection id.
    pub credentials: Vec<(String, ConnectionCredential)>,
    pub skipped: Vec<SkippedRoute>,
}

/// Reads and decrypts `connection-catalog.json` and keeps what is portable:
/// relay targets, bearer routes with bearer credentials, learned routes that
/// borrow one of those or use T3 Connect. Drops DPoP credentials, relay
/// tokens, SSH routes and credentials with no target.
pub fn read_electron_catalog(paths: &ElectronPaths, key: &SafeStorageKey) -> Result<ElectronImport, ImportError> {
    let bytes = read(&paths.catalog_path())?;
    let envelope: CatalogEnvelope = serde_json::from_slice(&bytes).map_err(|error| ImportError::Parse(error.to_string()))?;
    let encrypted = STANDARD.decode(envelope.encrypted_catalog.trim()).map_err(|_| ImportError::Decrypt)?;
    let plain = decrypt_safe_storage(&encrypted, key)?;
    parse_catalog(&plain)
}

fn parse_catalog(plain: &[u8]) -> Result<ElectronImport, ImportError> {
    let document: ConnectionCatalogDocument =
        serde_json::from_slice(plain).map_err(|error| ImportError::Parse(error.to_string()))?;
    let stored: ElectronCredentials =
        serde_json::from_slice(plain).map_err(|error| ImportError::Parse(error.to_string()))?;
    let credential = |connection_id: &str| -> Option<Result<ConnectionCredential, ()>> {
        let record = stored.credentials.iter().find(|record| record.connection_id == connection_id)?;
        Some(serde_json::from_value(record.credential.clone()).map_err(|_| ()))
    };
    let mut import = ElectronImport::default();
    for entry in document.entries() {
        let mut routes = Vec::new();
        for route in entry.routes.iter() {
            let id = route_id(&route.target);
            let address = route
                .bearer_profile()
                .map(|profile| profile.http_base_url.to_string())
                .unwrap_or_else(|| id.to_owned());
            let skip = |reason| SkippedRoute { environment_label: entry.label.clone(), address: address.clone(), reason };
            match &route.target.route {
                TargetRoute::Relay => routes.push(route.clone()),
                TargetRoute::Ssh { .. } => import.skipped.push(skip(SkipReason::Ssh)),
                TargetRoute::Bearer { .. } => {
                    let Some(ConnectionProfile::Bearer(profile)) = &route.profile else {
                        import.skipped.push(skip(SkipReason::MissingCredential));
                        continue;
                    };
                    if profile.authorization.is_some() {
                        routes.push(route.clone());
                        continue;
                    }
                    let owner = if is_learned(route) { credential_connection_id(id) } else { id };
                    match credential(owner) {
                        Some(Ok(bearer @ ConnectionCredential::Bearer { .. })) => {
                            if !import.credentials.iter().any(|(existing, _)| existing == owner) {
                                import.credentials.push((owner.to_owned(), bearer));
                            }
                            routes.push(route.clone());
                        }
                        Some(Ok(ConnectionCredential::Dpop { .. })) => {
                            import.skipped.push(skip(SkipReason::DeviceBoundCredential))
                        }
                        Some(Err(())) | None => import.skipped.push(skip(SkipReason::MissingCredential)),
                    }
                }
            }
        }
        // A learned route can outlive the bearer route it borrowed from.
        let kept: Vec<String> = routes.iter().map(|route| route_id(&route.target).to_owned()).collect();
        routes.retain(|route| {
            let id = route_id(&route.target);
            !is_learned(route)
                || route.bearer_profile().is_some_and(|profile| profile.authorization.is_some())
                || credential_connection_id(id) == id
                || kept.iter().any(|kept| kept == credential_connection_id(id))
        });
        if !routes.is_empty() {
            import.environments.push(ImportedEnvironment {
                environment_id: entry.environment_id.clone(),
                label: entry.label.clone(),
                routes,
                enabled: entry.enabled,
            });
        }
    }
    import.credentials.retain(|(owner, _)| {
        import.environments.iter().flat_map(|env| env.routes.iter()).any(|route| {
            let id = route_id(&route.target);
            id == owner || credential_connection_id(id) == owner
        })
    });
    Ok(import)
}

#[derive(Deserialize)]
struct ClerkTokens {
    #[serde(rename = "__clerk_client_jwt")]
    client_jwt: Option<String>,
}

/// The desktop's Clerk client JWT from `clerk/clerk-tokens.json`, or `None`
/// when it has no sign-in. Values are `enc:` + base64 ciphertext, or `raw:`.
pub fn read_electron_clerk_client_jwt(paths: &ElectronPaths, key: &SafeStorageKey) -> Result<Option<String>, ImportError> {
    let bytes = match read(&paths.clerk_tokens_path()) {
        Ok(bytes) => bytes,
        Err(ImportError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let tokens: ClerkTokens = serde_json::from_slice(&bytes).map_err(|error| ImportError::Parse(error.to_string()))?;
    let Some(value) = tokens.client_jwt.filter(|value| !value.is_empty()) else { return Ok(None) };
    let plain = if let Some(encoded) = value.strip_prefix("enc:") {
        let encrypted = STANDARD.decode(encoded).map_err(|_| ImportError::Decrypt)?;
        decrypt_safe_storage(&encrypted, key)?
    } else if let Some(raw) = value.strip_prefix("raw:") {
        raw.as_bytes().to_vec()
    } else {
        return Err(ImportError::Decrypt);
    };
    let jwt = String::from_utf8(plain).map_err(|_| ImportError::Decrypt)?;
    // electron-store keeps strings JSON-encoded.
    Ok(Some(serde_json::from_str::<String>(&jwt).unwrap_or(jwt)))
}
