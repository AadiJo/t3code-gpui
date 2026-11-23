//! Decodes the server's own encoded example values (`wire-fixtures.json`)
//! into the server settings panels' snapshot and renders each page over
//! them, without changing anything. Writes `wire-decode.md` (what decoded)
//! and a screenshot per page.
//!
//!   cargo run -p t3code-web --features test-support --example server_settings_wire_check -- <wire-fixtures.json> <out dir>

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use gpui_kit::{test::TestWindowExt as _, AppContext as _, HeadlessAppContext, WindowBounds, WindowOptions, px, size};
use serde::de::DeserializeOwned;
use serde_json::Value;
use t3code_contracts::{
    HostResourcesSnapshot, IssueTrackerConnectionList, Project, ProviderAuthState, ScheduledTask, ScheduledTaskListResult,
    ServerConfigStreamEvent, ServerProcessDiagnosticsResult, ServerProviderUpdatedPayload, SourceControlDiscoveryResult,
    StorageCleanupReport,
};
use t3code_web::components::settings::server_settings_data::{FixtureServerSettings, ServerPanelsModel};
use t3code_web::components::settings::server_settings_pages::ServerSettingsPages;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::provider_instances::ServerProvider;
use t3code_web::routes::settings::SettingsRoute;
use t3code_web::scenes::settings_server::fixture_snapshot;
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

/// Decodes every sample under `key`; returns the values and a table row.
fn decode_all<T: DeserializeOwned>(fixtures: &Value, key: &str, table: &mut Vec<String>) -> Vec<T> {
    let samples = fixtures.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for (ix, sample) in samples.iter().enumerate() {
        match serde_json::from_value::<T>(sample.clone()) {
            Ok(v) => ok.push(v),
            Err(e) => errors.push(format!("#{ix}: {e}")),
        }
    }
    table.push(format!("| {key} | {} | {} | {} |", samples.len(), ok.len(), errors.first().cloned().unwrap_or_default()));
    ok
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let fixtures_path = PathBuf::from(args.next().context("fixtures path")?);
    let out = PathBuf::from(args.next().context("out dir")?);
    std::fs::create_dir_all(&out)?;
    let fixtures: Value = serde_json::from_str(&std::fs::read_to_string(&fixtures_path)?)?;
    let mut table = vec!["| fixture | samples | decoded | first error |".to_owned(), "|---|---:|---:|---|".to_owned()];

    let mut providers: Vec<Arc<ServerProvider>> = Vec::new();
    for payload in decode_all::<ServerProviderUpdatedPayload>(&fixtures, "ServerProviderUpdatedPayload", &mut table) {
        providers.extend(payload.providers.into_iter().map(Arc::new));
    }
    for event in decode_all::<ServerConfigStreamEvent>(&fixtures, "ServerConfigStreamEvent", &mut table) {
        let value = serde_json::to_value(&event)?;
        if let Some(list) = value.pointer("/payload/providers").and_then(|v| v.as_array()) {
            for p in list {
                if let Ok(p) = serde_json::from_value::<ServerProvider>(p.clone()) {
                    providers.push(Arc::new(p));
                }
            }
        }
    }
    let auth = decode_all::<ProviderAuthState>(&fixtures, "ProviderAuthState", &mut table);
    let lists = decode_all::<ScheduledTaskListResult>(&fixtures, "ScheduledTaskListResult", &mut table);
    let mut tasks: Vec<ScheduledTask> = Vec::new();
    let mut task_failures = 0;
    for list in &lists {
        for item in &list.tasks {
            match serde_json::to_value(item).and_then(serde_json::from_value) {
                Ok(task) => tasks.push(task),
                Err(_) => task_failures += 1,
            }
        }
    }
    table.push(format!("| ScheduledTaskListResult.tasks → ScheduledTask | {} | {} | |", tasks.len() + task_failures, tasks.len()));
    let discovery = decode_all::<SourceControlDiscoveryResult>(&fixtures, "SourceControlDiscoveryResult", &mut table);
    let trackers = decode_all::<IssueTrackerConnectionList>(&fixtures, "IssueTrackerConnectionList", &mut table);
    let reports = decode_all::<StorageCleanupReport>(&fixtures, "StorageCleanupReport", &mut table);
    let processes = decode_all::<ServerProcessDiagnosticsResult>(&fixtures, "ServerProcessDiagnosticsResult", &mut table);
    let hosts = decode_all::<HostResourcesSnapshot>(&fixtures, "HostResourcesSnapshot", &mut table);
    let projects = decode_all::<Project>(&fixtures, "Project", &mut table);
    for key in ["ServerSettings", "ServerConfig"] {
        if fixtures.get(key).is_none() {
            table.push(format!("| {key} | 0 | 0 | not in wire-fixtures.json; the panels use the scene's settings |"));
        }
    }

    // One snapshot built from the decoded values. Settings stay the scene's, since no sample exists.
    let mut snapshot = fixture_snapshot();
    snapshot.environment_label = "Wire fixtures".into();
    snapshot.providers = providers.clone().into();
    snapshot.provider_auth = Arc::new(auth.iter().map(|a| (a.instance_id.to_string(), a.clone())).collect::<HashMap<_, _>>());
    snapshot.scheduled_tasks = tasks.clone().into();
    snapshot.source_control = discovery.into_iter().max_by_key(|d| d.source_control_providers.len()).map(Arc::new);
    snapshot.issue_trackers = trackers.into_iter().max_by_key(|t| t.trackers.len()).map(Arc::new);
    snapshot.storage_report = reports.into_iter().next().map(Arc::new);
    snapshot.process_diagnostics = processes.into_iter().max_by_key(|p| p.processes.len()).map(Arc::new);
    snapshot.host_resources = hosts.into_iter().next().map(Arc::new);
    snapshot.projects = projects.into_iter().filter(|p| p.deleted_at.is_none()).take(8).collect::<Vec<_>>().into();
    table.push(format!("| rendered | providers {} · auth states {} · tasks {} | | |", snapshot.providers.len(), snapshot.provider_auth.len(), snapshot.scheduled_tasks.len()));

    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
    });
    let source = Arc::new(FixtureServerSettings::new(snapshot));
    let mut route = None;
    let (handle, _) = cx.update(|cx| {
        let options = WindowOptions { window_bounds: Some(WindowBounds::centered(None, size(px(1440.), px(1000.)), cx)), show: false, focus: true, ..Default::default() };
        gpui_kit::open_window(options, cx, |window, cx| {
            let state = cx.new(|_| t3code_web::connection::ConnectionsState {
                environments: Vec::new(),
                cloud: None,
                verification_code: String::new().into(),
                electron_import_available: false,
            });
            let settings = cx.new(|cx| SettingsRoute::with_state(SettingsPath::Providers, state, window, cx));
            let model = cx.new(|_| ServerPanelsModel::new(source.clone()));
            let pages = cx.new(|_| ServerSettingsPages::new(model));
            settings.update(cx, |s, cx| s.set_server_pages(pages, cx));
            route = Some(settings.clone());
            settings
        })
    })?;
    let route = route.context("route")?;
    let mut shots = Vec::new();
    for path in [SettingsPath::Providers, SettingsPath::SourceControl, SettingsPath::ScheduledTasks, SettingsPath::Projects, SettingsPath::Maintenance] {
        cx.update(|cx| route.update(cx, |r, cx| r.navigate(path, cx)));
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx)).ok();
        let file = out.join(format!("wire-{}.png", path.slug()));
        cx.capture_screenshot(handle)?.save(&file)?;
        shots.push(format!("- {}: `{}`", path.slug(), file.display()));
    }
    let report = format!(
        "Wire fixtures decoded into the server settings panels (`{}`).\n\n{}\n\nRendered pages:\n{}\n",
        fixtures_path.display(),
        table.join("\n"),
        shots.join("\n")
    );
    std::fs::write(out.join("wire-decode.md"), &report)?;
    println!("{report}");
    Ok(())
}
