//! The server-side settings pages over a fixture environment: Providers with
//! instances in several states and a Claude sign-in waiting on a device
//! code, the Add provider dialog, Threads, Source control & issues with a
//! GitHub sign-in in progress, and the scheduled task editor.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use serde_json::json;
use t3code_client_runtime::relay::CloudAccountState;
use t3code_contracts::{ProviderAuthState, SourceControlProviderKind, SourceControlSignInInput};

use crate::AppRoot;
use crate::components::settings::provider_settings_panel::ProviderSettingsPanel;
use crate::components::settings::scheduled_tasks_settings::ScheduledTasksSettingsPanel;
use crate::components::settings::server_settings_data::{FixtureServerSettings, ServerPanelsModel, ServerPanelsSnapshot};
use crate::components::settings::server_settings_pages::ServerSettingsPages;
use crate::components::settings::settings_search::SettingsPath;
use crate::components::settings::source_control_settings::SourceControlSettingsPanel;
use crate::components::ui::dialog::Modal;
use crate::provider_instances::ServerProvider;
use crate::routes::settings::SettingsRoute;

fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
    serde_json::from_value(value).expect("fixture decodes")
}

fn model(slug: &str, name: &str, default: bool) -> serde_json::Value {
    json!({ "slug": slug, "name": name, "isCustom": false, "isDefault": default, "capabilities": null })
}

/// Five instances: Codex signed in with an update, Claude waiting on sign-in,
/// a second Codex account with an accent, Cursor missing its CLI, OpenCode off.
pub fn fixture_providers() -> Vec<Arc<ServerProvider>> {
    let checked = "2026-11-23T09:58:00.000Z";
    let list = vec![
        json!({
            "instanceId": "codex", "driver": "codex", "enabled": true, "installed": true, "version": "0.121.0",
            "status": "ready", "checkedAt": checked,
            "auth": { "status": "authenticated", "email": "dev@example.com", "label": "ChatGPT Pro" },
            "setup": { "canAuthenticate": true, "canInstall": true },
            "models": [model("gpt-6-astra", "GPT-6 Astra", true), model("gpt-6-luna", "GPT-6 Luna", false), model("gpt-5.6-terra", "GPT-5.6 Terra", false)],
            "versionAdvisory": { "status": "behind_latest", "currentVersion": "0.121.0", "latestVersion": "0.124.0",
                "updateCommand": "npm i -g @openai/codex@0.124.0", "canUpdate": true, "checkedAt": checked, "message": null }
        }),
        json!({
            "instanceId": "claudeAgent", "driver": "claudeAgent", "enabled": true, "installed": true, "version": "2.4.1",
            "status": "warning", "checkedAt": checked, "message": "Sign in to use Claude.",
            "auth": { "status": "unauthenticated" },
            "setup": { "canAuthenticate": true, "canInstall": true },
            "models": [model("claude-opus-5-5", "Claude Opus 5.5", true), model("claude-sonnet-5-5", "Claude Sonnet 5.5", false)]
        }),
        json!({
            "instanceId": "codex_work", "driver": "codex", "displayName": "Work", "accentColor": "#2563eb",
            "enabled": true, "installed": true, "version": "0.121.0", "status": "ready", "checkedAt": checked,
            "auth": { "status": "authenticated", "email": "work@example.com", "label": "ChatGPT Business" },
            "setup": { "canAuthenticate": true, "canInstall": true },
            "models": [model("gpt-6-astra", "GPT-6 Astra", true)]
        }),
        json!({
            "instanceId": "cursor", "driver": "cursor", "enabled": true, "installed": false, "version": null,
            "status": "error", "checkedAt": checked, "message": "cursor-agent was not found on PATH.",
            "auth": { "status": "unknown" }, "models": []
        }),
        json!({
            "instanceId": "opencode", "driver": "opencode", "enabled": false, "installed": true, "version": "1.4.2",
            "status": "disabled", "checkedAt": checked, "auth": { "status": "unknown" }, "models": []
        }),
    ];
    list.into_iter().map(|p| Arc::new(decode(p))).collect()
}

/// The fixture environment, as the panels read it.
pub fn fixture_snapshot() -> ServerPanelsSnapshot {
    let settings = decode(json!({
        "providerInstances": {
            "codex_work": { "driver": "codex", "displayName": "Work", "accentColor": "#2563eb", "enabled": true,
                "config": { "homePath": "~/.codex-work" },
                "environment": [{ "name": "OPENAI_BASE_URL", "value": "https://proxy.example.com/v1", "sensitive": false }] },
            "cursor": { "driver": "cursor", "enabled": true },
            "opencode": { "driver": "opencode", "enabled": false }
        },
        "defaultModelSelection": { "instanceId": "codex", "model": "gpt-6-astra" },
        "textGenerationModelSelection": { "instanceId": "codex", "model": "gpt-6-luna" },
        "autoResumeLimitedThreads": true,
        "sidebarAutoSettleOnMerge": true,
        "sidebarAutoSettleAfterDays": 3,
        "addProjectBaseDirectory": "~/Code",
        "responseStreamingMode": "paragraph",
        "defaultRuntimeMode": "full-access",
        "pullRequestMergeMethod": "squash",
        "reviewGuidesEnabled": true,
        "favorites": [{ "provider": "codex", "model": "gpt-6-astra" }],
        "providerModelPreferences": { "codex": { "hiddenModels": ["gpt-5.6-terra"] } },
        "usageLimitSources": {},
        "textGenerators": {}
    }));
    let projects = vec![
        decode(json!({ "id": "proj-web", "title": "t3code", "workspaceRoot": "~/Code/t3code", "defaultModelSelection": null,
            "scripts": [{ "id": "dev", "name": "Dev server", "command": "pnpm dev", "icon": "play", "runOnWorktreeCreate": false }],
            "pinnedAt": "2026-10-01T00:00:00Z", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-11-01T00:00:00Z", "deletedAt": null })),
        decode(json!({ "id": "proj-site", "title": "website", "workspaceRoot": "~/Code/website", "defaultModelSelection": null,
            "scripts": [], "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-11-01T00:00:00Z", "deletedAt": null })),
    ];
    let task = |id: &str, title: &str, schedule: serde_json::Value, enabled: bool| {
        decode(json!({
            "id": id, "title": title, "prompt": "Check for outdated dependencies and open a PR.", "enabled": enabled,
            "schedule": schedule, "projectId": "proj-web", "threadId": null,
            "workspaceStrategy": { "type": "worktree", "baseRef": "main", "startFromOrigin": true },
            "modelSelection": { "instanceId": "codex", "model": "gpt-6-astra" }, "runtimeMode": "full-access",
            "interactionMode": "default", "createdBy": "user", "creationSource": "web",
            "createdAt": "2026-11-01T00:00:00Z", "updatedAt": "2026-11-01T00:00:00Z",
            "nextRunAt": "2026-11-24T17:00:00.000Z", "lastRunAt": null, "lastRunStatus": "never", "lastRunError": null, "runCount": 0
        }))
    };
    let tasks = vec![
        task("task-deps", "Nightly dependency check", json!({ "type": "fixed_time", "timeOfDay": "09:00", "weekdays": [1, 2, 3, 4, 5] }), true),
        task("task-triage", "Triage new issues", json!({ "type": "interval", "everyMs": 3_600_000 }), true),
        task("task-hook", "Review opened PRs", json!({ "type": "webhook", "signature": { "header": "x-hub-signature-256", "encoding": "hex", "prefix": "sha256=" },
            "dedupe": { "replayKey": "{{headers.x-github-delivery}}" }, "filters": [{ "path": "body.action", "op": "in", "value": ["opened", "reopened"] }] }), false),
    ];
    let claude_auth: ProviderAuthState = decode(json!({
        "instanceId": "claudeAgent", "phase": "waiting", "flowId": "flow-1", "authorizationUrl": null, "expiresAt": null,
        "message": null, "interaction": { "type": "deviceCode", "id": "i-1", "url": "https://claude.ai/oauth/device", "userCode": "QFXK-7RMB" }
    }));
    let codex_auth: ProviderAuthState = decode(json!({ "instanceId": "codex", "phase": "succeeded", "flowId": null, "authorizationUrl": null, "expiresAt": null, "message": null }));
    let none = json!({ "_tag": "None" });
    let some = |v: &str| json!({ "_tag": "Some", "value": v });
    let host = |kind: &str, label: &str, exe: Option<&str>, status: &str, version: serde_json::Value, hint: &str, auth: &str, app: bool| json!({
        "kind": kind, "label": label, "executable": exe, "status": status, "version": version, "installHint": hint,
        "detail": { "_tag": "None" }, "appSignIn": app,
        "auth": { "status": auth, "account": { "_tag": "None" }, "host": { "_tag": "Some", "value": "github.com" }, "detail": { "_tag": "None" } }
    });
    let source_control = decode(json!({
        "versionControlSystems": [
            { "kind": "git", "implemented": true, "label": "Git", "executable": "git", "status": "available", "version": some("git version 2.43.0"), "installHint": "Install Git.", "detail": none },
            { "kind": "jj", "implemented": false, "label": "Jujutsu", "executable": "jj", "status": "missing", "version": none, "installHint": "Install Jujutsu.", "detail": none }
        ],
        "sourceControlProviders": [
            host("github", "GitHub", Some("gh"), "available", some("gh version 2.62.0"), "Install the GitHub CLI.", "unauthenticated", true),
            host("gitlab", "GitLab", Some("glab"), "missing", none.clone(), "Install the GitLab command-line tool (`glab`).", "unknown", false),
            host("bitbucket", "Bitbucket", None, "available", none.clone(), "Add a Bitbucket token in Settings → Source control & issues.", "unauthenticated", false)
        ]
    }));
    let trackers = decode(json!({ "trackers": [
        { "tracker": "linear", "displayName": "Linear", "host": "linear.app", "scopeNoun": "team",
          "credential": { "label": "Personal API key", "helpUrl": "https://linear.app/settings/api" }, "connected": false, "account": null, "detail": null, "scopes": [] }
    ], "bindings": {} }));
    ServerPanelsSnapshot {
        environment_id: "env-studio".into(),
        environment_label: "Studio Mac".into(),
        read_only: false,
        settings: Arc::new(settings),
        providers: fixture_providers().into(),
        managed_policy: None,
        supports_project_provider_overrides: true,
        projects: projects.into(),
        scheduled_tasks: tasks.into(),
        scheduled_task_time_zone: Some("America/Los_Angeles".into()),
        provider_auth: Arc::new(HashMap::from([("claudeAgent".to_owned(), claude_auth), ("codex".to_owned(), codex_auth)])),
        provider_install: Arc::new(HashMap::new()),
        source_control: Some(Arc::new(source_control)),
        issue_trackers: Some(Arc::new(trackers)),
        storage_report: None,
        database_storage: None,
        trace_diagnostics: None,
        process_diagnostics: None,
        host_resources: None,
    }
}

/// A fixture model over [`fixture_snapshot`], with the source for scripted checks.
pub fn fixture_model(cx: &mut App) -> (Entity<ServerPanelsModel>, Arc<FixtureServerSettings>) {
    let source = Arc::new(FixtureServerSettings::new(fixture_snapshot()));
    let model = cx.new(|_| ServerPanelsModel::new(source.clone()));
    (model, source)
}

/// The app with the settings modal open on one of these pages.
pub struct ServerSettingsScene {
    pub app: Entity<AppRoot>,
    pub settings: Entity<SettingsRoute>,
    pub pages: Entity<ServerSettingsPages>,
    pub source: Arc<FixtureServerSettings>,
    pub path: SettingsPath,
}

impl Render for ServerSettingsScene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `T3_SCENE_OFFSET=<px>` draws the page alone, scrolled down by that
        // much, in a column `T3_SCENE_PAGE_WIDTH` wide (default 768), to
        // inspect what lies below the dialog's fold.
        let env = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok());
        if let Some(offset) = env("T3_SCENE_OFFSET") {
            use crate::ActiveAppTheme as _;
            let width = env("T3_SCENE_PAGE_WIDTH").unwrap_or(768.);
            let path = self.path;
            let view = self.pages.update(cx, |pages, cx| pages.page(path, window, cx));
            let theme = cx.app_theme();
            return div()
                .size_full()
                .overflow_hidden()
                .bg(theme.colors.background())
                .text_color(theme.colors.foreground())
                .child(div().w(gpui_kit::px(width)).mx_auto().mt(gpui_kit::px(-offset)).children(view))
                .into_any_element();
        }
        div().size_full().child(self.app.clone()).child(Modal::dialog("settings-route", self.settings.clone(), |_, _| {})).into_any_element()
    }
}

/// What a scene opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Providers,
    ProvidersAdd,
    ProvidersClaude,
    Threads,
    SourceControl,
    TaskEditor,
    Page(SettingsPath),
}

/// Builds the scene entity (also used by the scripted check).
pub fn build_scene(variant: Variant, window: &mut Window, cx: &mut App) -> Entity<ServerSettingsScene> {
    let (model, source) = fixture_model(cx);
    let path = match variant {
        Variant::Providers | Variant::ProvidersAdd | Variant::ProvidersClaude => SettingsPath::Providers,
        Variant::Threads => SettingsPath::Threads,
        Variant::SourceControl => SettingsPath::SourceControl,
        Variant::TaskEditor => SettingsPath::ScheduledTasks,
        Variant::Page(path) => path,
    };
    let connections = cx.new(|_| crate::connection::ConnectionsState {
        environments: crate::scenes::connections::fixture_environments(),
        cloud: Some(crate::connection::CloudState { account: CloudAccountState::SignedOut, discovery: Default::default() }),
        verification_code: "7KQ4-M2XR".into(),
        electron_import_available: false,
    });
    let app = cx.new(|cx| AppRoot::new(window, cx));
    let settings = cx.new(|cx| SettingsRoute::with_state(path, connections, window, cx));
    let pages = cx.new(|_| ServerSettingsPages::new(model));
    settings.update(cx, |s, cx| s.set_server_pages(pages.clone(), cx));
    pages.update(cx, |p, cx| {
        p.page(path, window, cx);
    });
    match variant {
        Variant::ProvidersAdd => {
            if let Some(panel) = pages.read(cx).typed::<ProviderSettingsPanel>(SettingsPath::Providers) {
                panel.update(cx, |panel, cx| panel.open_add_dialog(window, cx));
            }
        }
        Variant::ProvidersClaude => {
            if let Some(panel) = pages.read(cx).typed::<ProviderSettingsPanel>(SettingsPath::Providers) {
                panel.update(cx, |panel, cx| panel.select("claudeAgent", cx));
            }
        }
        Variant::SourceControl => {
            if let Some(panel) = pages.read(cx).typed::<SourceControlSettingsPanel>(SettingsPath::SourceControl) {
                panel.update(cx, |panel, cx| {
                    panel.start_sign_in(SourceControlSignInInput { provider: SourceControlProviderKind::new("github"), host: "github.com".into() }, cx)
                });
            }
        }
        Variant::TaskEditor => {
            if let Some(panel) = pages.read(cx).typed::<ScheduledTasksSettingsPanel>(SettingsPath::ScheduledTasks) {
                panel.update(cx, |panel, cx| panel.open_editor(Some("task-hook"), cx));
            }
        }
        _ => {}
    }
    cx.new(|_| ServerSettingsScene { app, settings, pages, source, path })
}

fn scene(variant: Variant, light: bool, window: &mut Window, cx: &mut App) -> AnyView {
    if light {
        crate::theme_palette::set_app_theme(crate::AppTheme::new(&crate::theme_palette::ThemeDocument::graphite(), crate::theme_palette::Appearance::Light), cx);
    }
    build_scene(variant, window, cx).into()
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Providers, false, window, cx)
}
pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Providers, true, window, cx)
}
pub fn build_claude(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::ProvidersClaude, false, window, cx)
}
pub fn build_claude_light(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::ProvidersClaude, true, window, cx)
}
pub fn build_add(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::ProvidersAdd, false, window, cx)
}
pub fn build_add_light(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::ProvidersAdd, true, window, cx)
}
pub fn build_threads(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Threads, false, window, cx)
}
pub fn build_threads_light(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Threads, true, window, cx)
}
pub fn build_source_control(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::SourceControl, false, window, cx)
}
pub fn build_source_control_light(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::SourceControl, true, window, cx)
}
pub fn build_task_editor(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::TaskEditor, false, window, cx)
}
pub fn build_task_editor_light(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::TaskEditor, true, window, cx)
}
pub fn build_tasks(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::ScheduledTasks), false, window, cx)
}
pub fn build_worktrees(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::Worktrees), false, window, cx)
}
pub fn build_maintenance(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::Maintenance), false, window, cx)
}
pub fn build_devices(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::Devices), false, window, cx)
}
pub fn build_projects(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::Projects), false, window, cx)
}
pub fn build_browser(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::Browser), false, window, cx)
}
pub fn build_access(window: &mut Window, cx: &mut App) -> AnyView {
    scene(Variant::Page(SettingsPath::Access), false, window, cx)
}
