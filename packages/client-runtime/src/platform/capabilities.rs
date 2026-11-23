use std::path::PathBuf;

/// Directory name under the OS data dir. Deliberately not the Electron app's
/// `t3code-v2` profile or `~/.t3`, which belong to the desktop app and server.
const DATA_DIR_NAME: &str = "t3code-gpui";

/// The app's own data directory, for example `~/.local/share/t3code-gpui`.
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(DATA_DIR_NAME)
}

/// How this client introduces itself: the `client*` query parameters on the
/// socket and the `client_*` fields of a token exchange.
#[derive(Clone, Debug)]
pub struct ClientPresentation {
    /// Shown on the host's list of paired devices.
    pub label: String,
    pub app_version: String,
    /// One of the server's display OS names, such as `linux`, `macos`, `windows`.
    pub os: &'static str,
}

impl Default for ClientPresentation {
    fn default() -> Self {
        Self {
            label: "T3 Code Native".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            os: match std::env::consts::OS {
                "macos" => "macos",
                "windows" => "windows",
                "linux" => "linux",
                _ => "unknown",
            },
        }
    }
}
