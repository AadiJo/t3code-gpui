// Checks the Electron import against made-up fixtures encrypted the way
// Electron safeStorage does it: the Windows scheme (AES-256-GCM) and the
// macOS scheme (AES-128-CBC with the PBKDF2 key). Reads each fixture,
// imports it into a fresh catalog, and confirms the fixture files did not
// change. The real key sources (DPAPI, Keychain) are not exercised here.
//
// Usage: import-check <out-dir>
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use t3code_client_runtime::authorization::RemoteAuthorization;
use t3code_client_runtime::connection::credential_store::CredentialStore;
use t3code_client_runtime::connection::{ConnectionContext, ConnectionRegistry};
use t3code_client_runtime::platform::electron_import::{
    ElectronPaths, SafeStorageKey, encrypt_safe_storage, read_electron_catalog, read_electron_clerk_client_jwt,
};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore, SecretStore};
use t3code_client_runtime::relay::DpopKey;
use t3code_client_runtime::rpc::RpcSessionOptions;

/// A made-up desktop catalog with one of each case. Addresses are from the
/// documentation ranges; tokens are placeholders.
const CATALOG: &str = r#"{
  "schemaVersion": 1,
  "targets": [
    { "_tag": "BearerConnectionTarget", "environmentId": "env-a", "label": "Desk", "connectionId": "bearer:env-a:http://192.0.2.10:3773" },
    { "_tag": "BearerConnectionTarget", "environmentId": "env-a", "label": "Desk", "connectionId": "learned:env-a:http://198.51.100.10:3773@bearer:env-a:http://192.0.2.10:3773" },
    { "_tag": "RelayConnectionTarget", "environmentId": "env-a", "label": "Desk" },
    { "_tag": "BearerConnectionTarget", "environmentId": "env-b", "label": "Laptop", "connectionId": "bearer:env-b:https://laptop.example.net" },
    { "_tag": "RelayConnectionTarget", "environmentId": "env-c", "label": "Cloud box" },
    { "_tag": "BearerConnectionTarget", "environmentId": "env-c", "label": "Cloud box", "connectionId": "learned:env-c:https://cloud.example.net" },
    { "_tag": "SshConnectionTarget", "environmentId": "env-d", "label": "Devbox", "connectionId": "ssh:env-d:[\"devbox\",\"devbox.example.net\",null,null]" },
    { "_tag": "BearerConnectionTarget", "environmentId": "env-e", "label": "Old PC", "connectionId": "bearer:env-e:http://192.0.2.20:3773" }
  ],
  "profiles": [
    { "_tag": "BearerConnectionProfile", "connectionId": "bearer:env-a:http://192.0.2.10:3773", "environmentId": "env-a", "label": "Desk", "httpBaseUrl": "http://192.0.2.10:3773/", "wsBaseUrl": "ws://192.0.2.10:3773/" },
    { "_tag": "BearerConnectionProfile", "connectionId": "learned:env-a:http://198.51.100.10:3773@bearer:env-a:http://192.0.2.10:3773", "environmentId": "env-a", "label": "Desk", "httpBaseUrl": "http://198.51.100.10:3773/", "wsBaseUrl": "ws://198.51.100.10:3773/", "learned": true },
    { "_tag": "BearerConnectionProfile", "connectionId": "bearer:env-b:https://laptop.example.net", "environmentId": "env-b", "label": "Laptop", "httpBaseUrl": "https://laptop.example.net/", "wsBaseUrl": "wss://laptop.example.net/" },
    { "_tag": "BearerConnectionProfile", "connectionId": "learned:env-c:https://cloud.example.net", "environmentId": "env-c", "label": "Cloud box", "httpBaseUrl": "https://cloud.example.net/", "wsBaseUrl": "wss://cloud.example.net/", "learned": true, "authorization": "t3-connect", "network": "tailscale" },
    { "_tag": "SshConnectionProfile", "connectionId": "ssh:env-d:[\"devbox\",\"devbox.example.net\",null,null]", "environmentId": "env-d", "label": "Devbox", "target": { "alias": "devbox", "hostname": "devbox.example.net", "username": null, "port": null } },
    { "_tag": "BearerConnectionProfile", "connectionId": "bearer:env-e:http://192.0.2.20:3773", "environmentId": "env-e", "label": "Old PC", "httpBaseUrl": "http://192.0.2.20:3773/", "wsBaseUrl": "ws://192.0.2.20:3773/" }
  ],
  "credentials": [
    { "connectionId": "bearer:env-a:http://192.0.2.10:3773", "credential": { "_tag": "BearerConnectionCredential", "token": "fixture-bearer-a" } },
    { "connectionId": "bearer:env-b:https://laptop.example.net", "credential": { "_tag": "DpopConnectionCredential", "accessToken": "fixture-dpop-b", "expiresAtEpochMs": 0 } },
    { "connectionId": "bearer:env-e:http://192.0.2.20:3773", "credential": { "_tag": "BearerConnectionCredential", "token": "fixture-bearer-e" } },
    { "connectionId": "local:wsl:Ubuntu", "credential": { "_tag": "BearerConnectionCredential", "token": "fixture-orphan" } }
  ],
  "remoteDpopTokens": [
    { "environmentId": "env-c", "accountId": "user_fixture", "label": "Cloud box",
      "endpoint": { "httpBaseUrl": "https://tunnel.example.net/", "wsBaseUrl": "wss://tunnel.example.net/", "providerKind": "cloudflare_tunnel" },
      "accessToken": "fixture-relay-token", "expiresAtEpochMs": 0, "dpopThumbprint": "fixture-jkt" }
  ],
  "disabledEnvironmentIds": ["env-e"]
}"#;

fn digest(path: &Path) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}

/// Writes the fixture Electron files for `key` under `dir`.
fn write_fixture(dir: &Path, key: &SafeStorageKey) -> anyhow::Result<ElectronPaths> {
    let paths = ElectronPaths { state_dir: dir.join("userdata"), user_data_dir: dir.join("t3code-v2") };
    std::fs::create_dir_all(paths.state_dir.join("clerk"))?;
    std::fs::create_dir_all(&paths.user_data_dir)?;
    let encrypted = encrypt_safe_storage(CATALOG.as_bytes(), key, [7; 12]);
    let envelope = serde_json::json!({ "version": 1, "encryptedCatalog": STANDARD.encode(encrypted) });
    std::fs::write(paths.catalog_path(), serde_json::to_vec_pretty(&envelope)?)?;
    // electron-store keeps the JWT JSON-encoded.
    let jwt = encrypt_safe_storage(br#""fixture.client.jwt""#, key, [9; 12]);
    let tokens = serde_json::json!({ "__clerk_client_jwt": format!("enc:{}", STANDARD.encode(jwt)) });
    std::fs::write(paths.clerk_tokens_path(), serde_json::to_vec_pretty(&tokens)?)?;
    Ok(paths)
}

async fn check(name: &str, key: SafeStorageKey, wrong: SafeStorageKey, out: &Path, log: &mut String) -> anyhow::Result<()> {
    let dir = out.join(name);
    _ = std::fs::remove_dir_all(&dir);
    let paths = write_fixture(&dir.join("electron"), &key)?;
    let before = (digest(&paths.catalog_path())?, digest(&paths.clerk_tokens_path())?);
    writeln!(log, "== {name}")?;
    ensure!(read_electron_catalog(&paths, &wrong).is_err(), "a wrong key must not decrypt");
    writeln!(log, "wrong key: refused")?;
    let import = read_electron_catalog(&paths, &key)?;
    for env in &import.environments {
        let routes: Vec<_> = env
            .routes
            .iter()
            .map(|r| t3code_client_runtime::connection::routes::route_id(&r.target).to_owned())
            .collect();
        writeln!(log, "keep {} ({}) enabled={} routes={routes:?}", env.label, env.environment_id, env.enabled)?;
    }
    for (id, _) in &import.credentials {
        writeln!(log, "credential {id}")?;
    }
    for skipped in &import.skipped {
        writeln!(log, "needs re-pairing: {} {} {:?}", skipped.environment_label, skipped.address, skipped.reason)?;
    }
    let jwt = read_electron_clerk_client_jwt(&paths, &key)?;
    writeln!(log, "clerk client jwt: {jwt:?}")?;
    ensure!(jwt.as_deref() == Some("fixture.client.jwt"), "clerk jwt decrypts");
    ensure!(import.environments.len() == 3, "env-a, env-c and env-e carry over");
    ensure!(import.credentials.len() == 2, "only the two bearer credentials with targets carry over");
    ensure!(import.skipped.len() == 2, "the DPoP route and the SSH route need pairing again");

    // Import into a fresh catalog through the registry, as the app does.
    let data = dir.join("client-data");
    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(data.join("secrets.json")));
    let key = DpopKey::load_or_create(secrets.as_ref())?;
    let auth = Arc::new(RemoteAuthorization::new(ClientPresentation::default(), key, CredentialStore::new(secrets.clone()), None));
    let context = Arc::new(ConnectionContext { auth, session_options: RpcSessionOptions::default(), frame_tap: None });
    let registry = ConnectionRegistry::open(context, &data).await?;
    let added = registry.import(&import).await?;
    let again = registry.import(&import).await?;
    writeln!(log, "registry import added {added} routes; importing again added {again}")?;
    ensure!(again == 0, "a second import adds nothing");
    let entries = registry.entries().borrow().clone();
    for entry in entries.iter() {
        writeln!(log, "catalog: {} enabled={} routes={}", entry.label, entry.enabled, entry.routes.len())?;
    }
    let stored = std::fs::read_to_string(data.join("secrets.json"))?;
    let stored: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&stored)?;
    writeln!(log, "secret keys: {:?}", stored.keys().collect::<Vec<_>>())?;
    drop(registry);
    let after = (digest(&paths.catalog_path())?, digest(&paths.clerk_tokens_path())?);
    ensure!(before == after, "the Electron fixture files must not change");
    writeln!(log, "electron files unchanged (sha256 {} / {})", &after.0[..12], &after.1[..12])?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).context("usage: import-check <out-dir>")?);
    std::fs::create_dir_all(&out)?;
    let mut log = String::new();
    let result = async {
        check("windows-gcm", SafeStorageKey::WindowsGcm([3; 32]), SafeStorageKey::WindowsGcm([4; 32]), &out, &mut log).await?;
        check(
            "macos-cbc",
            SafeStorageKey::from_mac_password(b"fixture keychain password"),
            SafeStorageKey::from_mac_password(b"another password"),
            &out,
            &mut log,
        )
        .await
    }
    .await;
    if let Err(error) = &result {
        writeln!(log, "FAILED: {error:#}")?;
    }
    print!("{log}");
    std::fs::write(out.join("import-check.txt"), &log)?;
    result
}
