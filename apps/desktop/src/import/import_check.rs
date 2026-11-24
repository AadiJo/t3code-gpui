// Checks the Electron import against made-up fixtures encrypted the way
// Electron safeStorage does it: the Windows scheme (AES-256-GCM) and the
// macOS scheme (AES-128-CBC with the PBKDF2 key). Reads each fixture,
// imports it into a fresh catalog, and confirms the fixture files did not
// change. The real key sources (DPAPI, Keychain) are not exercised here.
//
// With an owner directory (`mac-client-settings.json`, `mac-desktop-settings.json`,
// `mac-local-storage/leveldb` copied from a real install), it also builds a whole stand-in
// install around them and runs the first-launch file import the app runs, checking that the
// theme, custom themes, drafts and UI state decode and that no install file changed. Only key
// names and counts are printed.
//
// Usage: import-check <out-dir> [owner-dir]
// Only the read half is used here.
#[allow(dead_code)]
#[path = "install.rs"]
mod install;

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
    ensure!(import.environments.len() == 4, "env-a, env-c and env-e carry over, and env-b without its device-bound credential");
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

/// Every file under `dir` with its digest, to prove an import left the install alone.
fn tree_digest(dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                files.push((entry.path().strip_prefix(dir)?.display().to_string(), digest(&entry.path())?));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// A stand-in Mac install: the fixture catalog and Clerk tokens under the macOS scheme, the
/// owner's settings files and localStorage copy. Runs the app's file import into a fresh data
/// directory twice (the second run must change nothing) and checks what came over.
fn check_owner_install(owner: &Path, out: &Path, log: &mut String) -> anyhow::Result<()> {
    let dir = out.join("owner-install");
    _ = std::fs::remove_dir_all(&dir);
    let key = SafeStorageKey::from_mac_password(b"fixture keychain password");
    let paths = write_fixture(&dir.join("install"), &key)?;
    std::fs::copy(owner.join("mac-client-settings.json"), paths.state_dir.join("client-settings.json"))?;
    std::fs::copy(owner.join("mac-desktop-settings.json"), paths.state_dir.join("desktop-settings.json"))?;
    copy_tree(&owner.join("mac-local-storage/leveldb"), &paths.user_data_dir.join("Local Storage/leveldb"))?;
    // Electron holds this lock while it runs; the import must not need it.
    std::fs::write(paths.user_data_dir.join("Local Storage/leveldb/LOCK"), b"")?;
    let before = tree_digest(&dir.join("install"))?;
    writeln!(log, "== owner install ({} files)", before.len())?;

    let entries = install::read_local_storage(&paths.user_data_dir.join("Local Storage/leveldb"))?;
    let mut origins: Vec<_> = entries.iter().map(|e| e.origin.clone()).collect();
    origins.sort();
    origins.dedup();
    writeln!(log, "localStorage: {} entries in {} origins", entries.len(), origins.len())?;
    let app = install::app_entries(entries);
    // Key names only, cut at the first id-like part, so the log carries no user data.
    let mut keys: Vec<_> = app.iter().map(|e| e.key.split([':', '.']).take(3).collect::<Vec<_>>().join(":")).collect();
    keys.sort();
    keys.dedup();
    writeln!(log, "app origin keys:\n  {}", keys.join("\n  "))?;

    let data = dir.join("data");
    let ui = dir.join("ui");
    let target = install::Install { paths: paths.clone() };
    let mut state = install::ImportState::default();
    let report = install::import_files(&target, &data, Some(&ui), &mut state);
    writeln!(
        log,
        "import: {} localStorage keys, drafts {}, ui state {}, desktop settings {}, notifications {}, client settings to apply {}, errors {:?}",
        report.local_storage_keys.len(),
        report.drafts,
        report.ui_state,
        report.desktop_settings,
        report.notifications,
        report.client_settings_path.is_some(),
        report.errors
    )?;
    ensure!(report.errors.is_empty(), "the file import reports no errors");
    ensure!(report.client_settings_path.is_some(), "first launch applies the install's client settings");
    let again = install::import_files(&target, &data, Some(&ui), &mut state);
    ensure!(again.client_settings_path.is_none() && again.local_storage_keys.is_empty(), "a second launch with nothing changed imports nothing");
    writeln!(log, "second launch: nothing to import")?;

    // The sidebar's width, open state and shelves reach the window's UI state store.
    let store = t3code_web::ui_state_store::UiStateStore::load(t3code_web::ui_state_store::StateDir(Some(ui.clone())));
    let (layout, ui_state) = (&store.layout, &store.ui);
    writeln!(
        log,
        "window ui state: sidebar {}px open {}, shelves working {} snoozed {} settled {}, {} project expansions, {} thread visits",
        layout.sidebar_width,
        layout.sidebar_open,
        ui_state.sidebar_working_expanded,
        ui_state.sidebar_snoozed_expanded,
        ui_state.sidebar_settled_expanded,
        ui_state.project_expanded_by_id.len(),
        ui_state.thread_last_visited_at_by_id.len()
    )?;
    if keys.iter().any(|k| k == "chat_thread_sidebar_width") {
        ensure!(report.layout && layout.sidebar_width != t3code_web::ui_state_store::LayoutState::default().sidebar_width, "the saved sidebar width reaches the layout");
    }

    let local: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&std::fs::read(data.join("local-storage.json"))?)?;
    let theme = local.get("t3code:theme").and_then(|v| v.as_str()).map(str::to_owned);
    writeln!(log, "theme preference present: {}", theme.is_some())?;
    let custom = local.get("t3code:themes:v1").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut parsed = 0;
    for theme in &custom {
        match t3code_web::theme_palette::ThemeDocument::parse(theme.to_string().as_bytes()) {
            Ok(_) => parsed += 1,
            Err(error) => writeln!(log, "custom theme does not parse: {error}")?,
        }
    }
    writeln!(log, "custom themes: {} stored, {parsed} parse", custom.len())?;
    ensure!(parsed == custom.len(), "every custom theme parses");
    if let Some(theme) = &theme {
        let known = custom.iter().any(|t| t.get("id").and_then(|id| id.as_str()) == Some(theme.as_str()));
        writeln!(log, "selected theme is a custom theme: {known}")?;
    }
    for key in ["t3code:theme-appearance-mode", "t3code:theme-halves:v1"] {
        writeln!(log, "{key}: {}", local.contains_key(key))?;
    }

    let after = tree_digest(&dir.join("install"))?;
    ensure!(before == after, "no install file changed");
    writeln!(log, "install files unchanged ({} files)", after.len())?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).context("usage: import-check <out-dir> [owner-dir]")?);
    let owner = std::env::args().nth(2).map(std::path::PathBuf::from);
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
        .await?;
        if let Some(owner) = &owner {
            check_owner_install(owner, &out, &mut log)?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = &result {
        writeln!(log, "FAILED: {error:#}")?;
    }
    print!("{log}");
    std::fs::write(out.join("import-check.txt"), &log)?;
    result
}
