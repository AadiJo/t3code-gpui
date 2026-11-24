//! Reads an existing T3 Code desktop install into this app's data directory at first launch,
//! with no button: the files that need no key (client and desktop settings, the notification
//! inbox, the web app's localStorage: theme, custom themes, appearance mode, theme halves, UI
//! state, drafts) at once, and the encrypted connection catalog and T3 Connect session once
//! the app has explained the Keychain prompt they cause. Nothing in the install is written:
//! localStorage is read from a copy, since Electron holds a lock on the original.
//!
//! Later launches re-read `client-settings.json` when its modification time changed.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use t3code_client_runtime::platform::electron_import::ElectronPaths;

/// What this app's data directory records about the import.
pub const STATE_FILE: &str = "install-import.json";
/// The web app's localStorage keys that are client documents here.
const DRAFTS_KEY: &str = "t3code:composer-drafts:v1";
const UI_STATE_KEY: &str = "t3code:ui-state:v1";
const CUSTOM_THEMES_KEY: &str = "t3code:themes:v1";
/// The sidebar's saved width and open state (`SidebarProvider` storage keys).
const SIDEBAR_WIDTH_KEY: &str = "chat_thread_sidebar_width";
const SIDEBAR_OPEN_KEY: &str = "chat_thread_sidebar_open";
/// The sidebar shelves' open states.
const SHELF_KEYS: [&str; 3] = ["t3code:sidebar:working-expanded", "t3code:sidebar:snoozed-expanded", "t3code:sidebar:settled-expanded"];

/// The desktop install to read. `T3CODE_IMPORT_HOME` points at a stand-in install for checks:
/// `<dir>/userdata` is the T3 home state directory, `<dir>/t3code-v2` Chromium's `userData`.
#[derive(Clone, Debug)]
pub struct Install {
    pub paths: ElectronPaths,
}

impl Install {
    pub fn detect() -> Option<Self> {
        if let Some(home) = std::env::var_os("T3CODE_IMPORT_HOME") {
            let home = PathBuf::from(home);
            return Some(Self { paths: ElectronPaths { state_dir: home.join("userdata"), user_data_dir: home.join("t3code-v2") } });
        }
        ElectronPaths::detect().map(|paths| Self { paths })
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.paths.state_dir.join(name)
    }

    /// Chromium's localStorage database.
    pub fn local_storage_dir(&self) -> PathBuf {
        self.paths.user_data_dir.join("Local Storage").join("leveldb")
    }

    /// Whether the encrypted connection catalog exists (the Keychain step has work to do).
    pub fn has_catalog(&self) -> bool {
        self.paths.catalog_path().is_file()
    }
}

/// What has been imported, kept in [`STATE_FILE`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportState {
    /// The unencrypted files and localStorage were copied.
    pub files_imported: bool,
    /// `client-settings.json`'s modification time when last applied, in ms.
    pub client_settings_modified_ms: Option<u128>,
    /// The encrypted catalog and Clerk session were read (or there was nothing to read).
    pub connections_imported: bool,
    /// The one-line "shared T3 Connect session" notice was shown.
    pub clerk_notice_shown: bool,
}

impl ImportState {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(STATE_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) {
        _ = std::fs::create_dir_all(data_dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            _ = std::fs::write(data_dir.join(STATE_FILE), bytes);
        }
    }
}

/// What one run of [`import_files`] did, for the log and the checks.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesReport {
    /// localStorage keys copied into `local-storage.json`.
    pub local_storage_keys: Vec<String>,
    pub drafts: bool,
    pub ui_state: bool,
    /// The sidebar's width, open state, shelves and project expansion reached the window's UI state.
    pub layout: bool,
    pub desktop_settings: bool,
    pub notifications: bool,
    /// The install's client settings to apply over this app's (first launch, or the file
    /// changed since the last launch).
    pub client_settings_path: Option<PathBuf>,
    pub errors: Vec<String>,
}

/// One localStorage entry.
#[derive(Clone, Debug)]
pub struct StorageEntry {
    pub origin: String,
    pub key: String,
    pub value: String,
}

/// A Chromium localStorage string: a 0x00 prefix is UTF-16LE, 0x01 is Latin-1.
fn decode_storage_string(bytes: &[u8]) -> Option<String> {
    let (&format, rest) = bytes.split_first()?;
    match format {
        0 => {
            let units: Vec<u16> = rest.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
            String::from_utf16(&units).ok()
        }
        1 => Some(rest.iter().map(|&byte| byte as char).collect()),
        _ => None,
    }
}

/// Every localStorage entry in a Chromium `Local Storage/leveldb` directory. Reads a copy in a
/// temporary directory: the original stays untouched and its lock is never taken.
pub fn read_local_storage(dir: &Path) -> anyhow::Result<Vec<StorageEntry>> {
    let copy = std::env::temp_dir().join(format!("t3code-local-storage-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&copy)?;
    let result = (|| {
        for file in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let file = file?;
            if file.file_name() == "LOCK" || !file.file_type()?.is_file() {
                continue;
            }
            std::fs::copy(file.path(), copy.join(file.file_name()))?;
        }
        let options = rusty_leveldb::Options { create_if_missing: false, ..Default::default() };
        let mut db = rusty_leveldb::DB::open(&copy, options).map_err(|error| anyhow::anyhow!("opening localStorage: {error}"))?;
        let mut iter = db.new_iter().map_err(|error| anyhow::anyhow!("reading localStorage: {error}"))?;
        let mut entries = Vec::new();
        while let Some((key, value)) = rusty_leveldb::LdbIterator::next(&mut iter) {
            let Some(rest) = key.strip_prefix(b"_") else { continue };
            let Some(split) = rest.iter().position(|&byte| byte == 0) else { continue };
            let origin = String::from_utf8_lossy(&rest[..split]).into_owned();
            let (Some(key), Some(value)) = (decode_storage_string(&rest[split + 1..]), decode_storage_string(&value)) else { continue };
            entries.push(StorageEntry { origin, key, value });
        }
        Ok(entries)
    })();
    _ = std::fs::remove_dir_all(&copy);
    result
}

/// The app's own origin: the one holding the most `t3code` keys.
pub fn app_entries(entries: Vec<StorageEntry>) -> Vec<StorageEntry> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for entry in entries.iter().filter(|e| e.key.starts_with("t3code")) {
        *counts.entry(entry.origin.as_str()).or_default() += 1;
    }
    let Some(origin) = counts.into_iter().max_by_key(|(_, count)| *count).map(|(origin, _)| origin.to_owned()) else {
        return Vec::new();
    };
    entries.into_iter().filter(|e| e.origin == origin).collect()
}

/// A localStorage string as the JSON value this app stores: JSON when it parses, else the text.
fn storage_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

/// A zustand `persist` document's state (`{ state, version }`), or the value itself.
fn persisted_state(raw: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(raw).ok()?;
    match value {
        Value::Object(mut object) if object.contains_key("state") => object.remove("state"),
        other => Some(other),
    }
}

/// Writes a client document envelope the runtime's `ClientStore` reads, unless one exists.
fn write_client_document<T: Serialize + serde::de::DeserializeOwned>(data_dir: &Path, file: &str, state: Value) -> anyhow::Result<bool> {
    let path = data_dir.join("client-state").join(file);
    if path.exists() {
        return Ok(false);
    }
    let document: T = serde_json::from_value(state)?;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let envelope = serde_json::json!({ "schemaVersion": 1, "document": serde_json::to_value(&document)? });
    std::fs::write(&path, serde_json::to_vec(&envelope)?)?;
    Ok(true)
}

/// The web app stores a custom theme as `{ id, label, appearance, colors, variants }`
/// (`parseStoredTheme`); this app's theme store keeps the theme file form it imports
/// (`version: 1`, `name`). Managed and CSS-only fields have no counterpart and are dropped.
fn custom_themes_as_files(value: Value) -> Value {
    let Value::Array(themes) = value else { return value };
    Value::Array(
        themes
            .into_iter()
            .filter_map(|theme| {
                let Value::Object(theme) = theme else { return None };
                let mut file = Map::new();
                file.insert("version".into(), Value::from(1));
                file.insert("id".into(), theme.get("id")?.clone());
                file.insert("name".into(), theme.get("label")?.clone());
                file.insert("appearance".into(), theme.get("appearance")?.clone());
                file.insert("colors".into(), theme.get("colors")?.clone());
                if let Some(variants) = theme.get("variants") {
                    file.insert("variants".into(), variants.clone());
                }
                Some(Value::Object(file))
            })
            .collect(),
    )
}

/// The web app keeps local visit times as ISO strings; the runtime's document keeps epoch ms.
fn ui_state_document(mut state: Value) -> Value {
    if let Some(Value::Object(visits)) = state.get_mut("threadLastVisitedAtById") {
        visits.retain(|_, value| match value {
            Value::String(iso) => match t3code_client_runtime::state::models::parse_ms(iso) {
                Some(ms) => {
                    *value = Value::from(ms);
                    true
                }
                None => false,
            },
            Value::Number(_) => true,
            _ => false,
        });
    }
    state
}

/// Writes the window's own UI state (`ui-state.json`, `layout.json` in `ui_dir`) from the
/// web's: `t3code:ui-state:v1` with the shelves' keys, and the sidebar's width and open state.
/// Each file is written only when this app has none yet.
fn write_window_ui_state(ui_dir: &Path, entries: &[StorageEntry]) -> anyhow::Result<bool> {
    use t3code_web::ui_state_store::{LayoutState, UiState, files};
    let value = |key: &str| entries.iter().find(|e| e.key == key).map(|e| storage_value(&e.value));
    let mut wrote = false;
    let ui_path = ui_dir.join(files::UI_STATE);
    if !ui_path.exists() {
        let mut state = match entries.iter().find(|e| e.key == UI_STATE_KEY).and_then(|e| persisted_state(&e.value)) {
            Some(Value::Object(state)) => state,
            _ => Map::new(),
        };
        let fields = ["sidebarWorkingExpanded", "sidebarSnoozedExpanded", "sidebarSettledExpanded"];
        for (key, field) in SHELF_KEYS.into_iter().zip(fields) {
            if let Some(open @ Value::Bool(_)) = value(key) {
                state.insert(field.to_owned(), open);
            }
        }
        if !state.is_empty() {
            let state: UiState = serde_json::from_value(Value::Object(state))?;
            std::fs::create_dir_all(ui_dir)?;
            std::fs::write(&ui_path, serde_json::to_vec(&state)?)?;
            wrote = true;
        }
    }
    let layout_path = ui_dir.join(files::LAYOUT);
    let width = value(SIDEBAR_WIDTH_KEY).and_then(|v| v.as_f64());
    let open = value(SIDEBAR_OPEN_KEY).and_then(|v| v.as_bool());
    if !layout_path.exists() && (width.is_some() || open.is_some()) {
        let mut layout = LayoutState::default();
        if let Some(width) = width {
            layout.sidebar_width = width as f32;
        }
        if let Some(open) = open {
            layout.sidebar_open = open;
        }
        std::fs::create_dir_all(ui_dir)?;
        std::fs::write(&layout_path, serde_json::to_vec(&layout)?)?;
        wrote = true;
    }
    Ok(wrote)
}

fn modified_ms(path: &Path) -> Option<u128> {
    std::fs::metadata(path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis())
}

/// Copies `source` to `target` unless `target` exists.
fn copy_if_absent(source: &Path, target: &Path) -> anyhow::Result<bool> {
    if target.exists() || !source.is_file() {
        return Ok(false);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, target)?;
    Ok(true)
}

/// The unencrypted half of the import. `data_dir` is the runtime's data directory (settings,
/// localStorage, client documents); `ui_dir` holds the inbox. Runs before the settings stores
/// load, so they read what it wrote. Only fills what this app does not have yet.
pub fn import_files(install: &Install, data_dir: &Path, ui_dir: Option<&Path>, state: &mut ImportState) -> FilesReport {
    let mut report = FilesReport::default();
    let client_settings = install.state_file("client-settings.json");
    let modified = modified_ms(&client_settings);
    if modified.is_some() && (!state.files_imported || state.client_settings_modified_ms != modified) {
        report.client_settings_path = Some(client_settings);
        state.client_settings_modified_ms = modified;
    }
    if state.files_imported {
        return report;
    }
    let mut errors = Vec::new();
    let mut note = |result: anyhow::Result<bool>, what: &str| match result {
        Ok(done) => done,
        Err(error) => {
            errors.push(format!("{what}: {error:#}"));
            false
        }
    };
    report.desktop_settings = note(
        copy_if_absent(&install.state_file("desktop-settings.json"), &data_dir.join("desktop-settings.json")),
        "desktop settings",
    );
    if let Some(ui_dir) = ui_dir {
        report.notifications =
            note(copy_if_absent(&install.state_file("notifications.json"), &ui_dir.join("notifications.json")), "notifications");
    }
    let storage_dir = install.local_storage_dir();
    if storage_dir.is_dir() {
        match read_local_storage(&storage_dir) {
            Ok(entries) => {
                let mut local: Map<String, Value> = std::fs::read(data_dir.join("local-storage.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default();
                let entries = app_entries(entries);
                if let Some(ui_dir) = ui_dir {
                    report.layout = note(write_window_ui_state(ui_dir, &entries), "window ui state");
                }
                for entry in entries {
                    match entry.key.as_str() {
                        DRAFTS_KEY => {
                            let written = persisted_state(&entry.value)
                                .map(|state| {
                                    write_client_document::<t3code_client_runtime::state::drafts::Drafts>(data_dir, "composer-drafts.json", state)
                                })
                                .unwrap_or(Ok(false));
                            report.drafts = note(written, "drafts");
                        }
                        UI_STATE_KEY => {
                            let written = persisted_state(&entry.value)
                                .map(ui_state_document)
                                .map(|state| {
                                    write_client_document::<t3code_client_runtime::state::ui_state::UiState>(data_dir, "ui-state.json", state)
                                })
                                .unwrap_or(Ok(false));
                            report.ui_state = note(written, "ui state");
                        }
                        key if key.starts_with("t3code") && !local.contains_key(key) => {
                            let value = storage_value(&entry.value);
                            let value = if key == CUSTOM_THEMES_KEY { custom_themes_as_files(value) } else { value };
                            local.insert(entry.key.clone(), value);
                            report.local_storage_keys.push(entry.key);
                        }
                        _ => {}
                    }
                }
                let written = std::fs::create_dir_all(data_dir)
                    .and_then(|()| std::fs::write(data_dir.join("local-storage.json"), serde_json::to_vec_pretty(&local).unwrap_or_default()))
                    .map(|()| true)
                    .map_err(anyhow::Error::from);
                note(written, "local storage");
            }
            Err(error) => {
                note(Err(error), "local storage");
            }
        }
    }
    report.errors = errors;
    state.files_imported = true;
    report
}
