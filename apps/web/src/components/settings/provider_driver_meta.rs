//! Every provider driver this build knows: its label, badge, environment
//! fields and settings form. Generated from the TypeScript provider client
//! registry (`providerClients.ts` with `deriveProviderSettingsFields`) by
//! `gen_driver_meta.py`; regenerate rather than edit by hand.

use gpui_kit::SharedString;

use crate::i18n::t;

/// How a settings-form field is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldControl {
    Text,
    Password,
    Select,
    Switch,
    Textarea,
}

/// Whether clearing a field removes the key or stores an empty string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearWhenEmpty {
    Omit,
    Persist,
}

/// One option of a select field.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldOption {
    pub value: &'static str,
    pub label: SharedString,
}

/// One field of a driver's settings form (`ProviderSettingsFieldModel`).
#[derive(Clone, Debug, PartialEq)]
pub struct FormField {
    pub key: &'static str,
    pub control: FieldControl,
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub placeholder: Option<SharedString>,
    pub clear_when_empty: ClearWhenEmpty,
    pub secret: bool,
    pub accepts_secret_reference: bool,
    pub default_bool: Option<bool>,
    pub options: Vec<FieldOption>,
}

/// A dedicated environment variable a driver asks for (`ProviderEnvironmentField`).
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentField {
    pub name: &'static str,
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub placeholder: Option<SharedString>,
    pub sensitive: bool,
}

/// A driver's client definition.
#[derive(Clone, Debug, PartialEq)]
pub struct DriverMeta {
    pub driver: &'static str,
    pub label: &'static str,
    pub badge_label: Option<SharedString>,
    pub has_default_instance: bool,
    pub model_families: &'static [&'static str],
    pub environment_fields: Vec<EnvironmentField>,
    pub fields: Vec<FormField>,
}

/// The drivers in registry order, which is the Providers list and the add dialog's order.
pub const DRIVERS: &[&str] = &["codex", "claudeAgent", "cursor", "grok", "fx", "opencode", "antigravity", "kiro", "muse", "pi", "acpRegistry"];

/// The definition for `driver`, or `None` for a driver this build lacks.
pub fn driver_meta(driver: &str) -> Option<DriverMeta> {
    Some(match driver {
        "codex" => DriverMeta {
            driver: "codex",
            label: "Codex",
            badge_label: None,
            has_default_instance: true,
            model_families: &["GPT-", "GPT "],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the Codex binary used by this instance.")), placeholder: Some(SharedString::new_static("codex")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "homePath", control: FieldControl::Text, label: t!("CODEX_HOME path"), description: Some(t!("Custom Codex home and config directory.")), placeholder: Some(SharedString::new_static("~/.codex")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "shadowHomePath", control: FieldControl::Text, label: t!("Shadow home path"), description: Some(t!("Account-specific Codex home. Keeps auth.json separate while sharing state from CODEX_HOME.")), placeholder: Some(SharedString::new_static("~/.codex-t3/personal")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "launchArgs", control: FieldControl::Text, label: t!("Launch arguments"), description: Some(t!("Additional CLI arguments passed to codex app-server on session start.")), placeholder: None, clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "claudeAgent" => DriverMeta {
            driver: "claudeAgent",
            label: "Claude",
            badge_label: None,
            has_default_instance: true,
            model_families: &["Claude "],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the Claude binary used by this instance.")), placeholder: Some(SharedString::new_static("claude")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "homePath", control: FieldControl::Text, label: t!("CLAUDE_CONFIG_DIR path"), description: Some(t!("Custom Claude home and config directory. Keeps .claude.json and .claude separate.")), placeholder: Some(SharedString::new_static("~/.claude")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "autoCompactWindow", control: FieldControl::Text, label: t!("Auto-compact after"), description: Some(t!("Compact after 100,000 to 1,000,000 tokens. Leave empty to use Claude's default.")), placeholder: Some(t!("e.g. 300000")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "idleCompaction", control: FieldControl::Select, label: t!("Compact idle threads"), description: Some(t!("When to compact an idle thread over 200,000 tokens: after you return, just before its prompt cache expires, or never. Overrides the default in Providers > Advanced.")), placeholder: None, clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![FieldOption { value: "default", label: t!("Use default") }, FieldOption { value: "onReturn", label: t!("When you return") }, FieldOption { value: "beforeCacheExpires", label: t!("Before the cache expires (beta)") }, FieldOption { value: "off", label: t!("Never", context = "option, never compact idle threads") }] },
        FormField { key: "launchArgs", control: FieldControl::Text, label: t!("Launch arguments"), description: Some(t!("Additional CLI arguments passed on session start.")), placeholder: Some(t!("e.g. --chrome")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "cursor" => DriverMeta {
            driver: "cursor",
            label: "Cursor",
            badge_label: None,
            has_default_instance: true,
            model_families: &["Grok "],
            environment_fields: vec![            EnvironmentField { name: "CURSOR_API_KEY", label: SharedString::new_static("Cursor API key"), description: Some(SharedString::new_static("Optional. Overrides browser sign-in for this provider.")), placeholder: Some(SharedString::new_static("Paste API key")), sensitive: true }],
            fields: vec![

            ],
        },
        "grok" => DriverMeta {
            driver: "grok",
            label: "Grok",
            badge_label: None,
            has_default_instance: true,
            model_families: &["Grok "],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the Grok CLI binary.")), placeholder: Some(SharedString::new_static("grok")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "fx" => DriverMeta {
            driver: "fx",
            label: "fx",
            badge_label: None,
            has_default_instance: true,
            model_families: &[],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the fx binary. All fx instances on this machine share its fx login.")), placeholder: Some(SharedString::new_static("fx")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "opencode" => DriverMeta {
            driver: "opencode",
            label: "OpenCode",
            badge_label: None,
            has_default_instance: true,
            model_families: &[],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the OpenCode binary.")), placeholder: Some(SharedString::new_static("opencode")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "serverUrl", control: FieldControl::Text, label: t!("Server URL"), description: Some(t!("Leave blank to let T3 Code spawn the server when needed.")), placeholder: Some(SharedString::new_static("http://127.0.0.1:4096")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "serverPassword", control: FieldControl::Password, label: t!("Server password"), description: Some(t!("Stored separately on the host.")), placeholder: Some(t!("Optional", context = "placeholder of an optional settings field")), clear_when_empty: ClearWhenEmpty::Omit, secret: true, accepts_secret_reference: true, default_bool: None, options: vec![] }
            ],
        },
        "antigravity" => DriverMeta {
            driver: "antigravity",
            label: "Antigravity",
            badge_label: None,
            has_default_instance: true,
            model_families: &["Gemini "],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "authMethod", control: FieldControl::Select, label: t!("Sign-in method"), description: Some(t!("Google accounts use your subscription; API keys and Agent Platform bill usage.")), placeholder: None, clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![FieldOption { value: "oauth-personal", label: t!("Google account") }, FieldOption { value: "oauth-business", label: SharedString::new_static("Gemini Enterprise") }, FieldOption { value: "gemini-api-key", label: t!("Gemini API key") }, FieldOption { value: "agent-platform", label: SharedString::new_static("Agent Platform (Vertex AI)") }] },
        FormField { key: "apiKey", control: FieldControl::Password, label: t!("API key"), description: Some(t!("Gemini or Vertex AI express key. Stored separately on the host.")), placeholder: Some(t!("Optional", context = "placeholder of an optional settings field")), clear_when_empty: ClearWhenEmpty::Omit, secret: true, accepts_secret_reference: true, default_bool: None, options: vec![] },
        FormField { key: "gcpProject", control: FieldControl::Text, label: t!("GCP project"), description: Some(t!("Required for Gemini Enterprise. Agent Platform uses it when no API key is set.")), placeholder: Some(SharedString::new_static("my-project-id")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "gcpLocation", control: FieldControl::Text, label: t!("GCP location"), description: Some(t!("Region for Gemini Enterprise or Agent Platform.")), placeholder: Some(SharedString::new_static("us-central1")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Custom ACP executable. Leave empty to select automatically.")), placeholder: Some(t!("Automatic", context = "placeholder, chosen automatically when empty")), clear_when_empty: ClearWhenEmpty::Persist, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "kiro" => DriverMeta {
            driver: "kiro",
            label: "Kiro",
            badge_label: Some(SharedString::new_static("Early Access")),
            has_default_instance: true,
            model_families: &[],
            environment_fields: vec![            EnvironmentField { name: "KIRO_API_KEY", label: SharedString::new_static("Kiro API key"), description: Some(SharedString::new_static("Optional. Signs in without `kiro-cli login` (Kiro Pro and above).")), placeholder: Some(SharedString::new_static("Paste API key")), sensitive: true }],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the Kiro CLI binary.")), placeholder: Some(SharedString::new_static("kiro-cli")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "muse" => DriverMeta {
            driver: "muse",
            label: "Muse Code",
            badge_label: None,
            has_default_instance: true,
            model_families: &["Muse "],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the Muse Code binary on this environment. Sign in with muse login on that host.")), placeholder: Some(SharedString::new_static("muse")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "pi" => DriverMeta {
            driver: "pi",
            label: "Pi",
            badge_label: None,
            has_default_instance: true,
            model_families: &[],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "binaryPath", control: FieldControl::Text, label: t!("Binary path"), description: Some(t!("Path to the Pi coding agent binary.")), placeholder: Some(SharedString::new_static("pi")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "launchArgs", control: FieldControl::Text, label: t!("Launch arguments"), description: Some(t!("Additional CLI arguments passed to pi --mode rpc on session start.")), placeholder: None, clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        "acpRegistry" => DriverMeta {
            driver: "acpRegistry",
            label: "ACP Registry",
            badge_label: None,
            has_default_instance: false,
            model_families: &[],
            environment_fields: vec![],
            fields: vec![
        FormField { key: "source", control: FieldControl::Select, label: t!("ACP source"), description: None, placeholder: None, clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![FieldOption { value: "registry", label: SharedString::new_static("ACP Registry") }, FieldOption { value: "local", label: t!("Local command") }] },
        FormField { key: "agentId", control: FieldControl::Text, label: t!("Registry agent ID"), description: Some(t!("Agent identifier from the official ACP Registry, for example 'devin'.")), placeholder: Some(SharedString::new_static("devin")), clear_when_empty: ClearWhenEmpty::Persist, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "commandPath", control: FieldControl::Text, label: t!("Executable override"), description: Some(t!("Executable on this environment. For registry agents, this overrides the distribution executable while keeping its arguments and environment.")), placeholder: Some(t!("Registry default")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] },
        FormField { key: "authMethodId", control: FieldControl::Text, label: t!("Authentication method"), description: Some(t!("Optional ACP authentication method ID. By default, the first agent-managed method is selected.")), placeholder: Some(SharedString::new_static("auto")), clear_when_empty: ClearWhenEmpty::Omit, secret: false, accepts_secret_reference: false, default_bool: None, options: vec![] }
            ],
        },
        _ => return None,
    })
}

/// A local ACP command shows only the source and its executable (`deriveProviderSettingsFields` with `source: "local"`).
pub fn local_acp_fields() -> Vec<FormField> {
    let Some(meta) = driver_meta("acpRegistry") else { return Vec::new() };
    meta.fields
        .into_iter()
        .filter(|f| f.key == "source" || f.key == "commandPath")
        .map(|mut f| {
            if f.key == "commandPath" {
                f.label = t!("Executable");
                f.description = Some(t!("Executable name or path on this environment."));
                f.placeholder = Some(t!("e.g. dsh"));
            }
            f
        })
        .collect()
}
