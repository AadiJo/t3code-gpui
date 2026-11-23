//! Drives the server settings panels headlessly against the fixture source:
//! the Add provider wizard, every provider sign-in flow (device code, pasted
//! callback, credentials, terminal), the scheduled task editor's four
//! schedule kinds and its validation, the device host editor, the GitHub
//! device-code sign-in and the add-to-machine folder browser. Each step is
//! checked against the RPCs the fixture saw and screenshotted; the transcript
//! goes to `transcript.txt`.
//!
//!   cargo run -p t3code-web --features test-support --example server_settings_check -- <out dir>

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use gpui_kit::component::input::InputState;
use gpui_kit::{Focusable as _, SharedString, AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, WindowBounds, WindowOptions, px, size, test::TestWindowExt as _};
use serde_json::json;
use t3code_contracts::ProviderAuthState;
use t3code_web::components::add_project_to_machine_dialog::AddProjectToMachineDialog;
use t3code_web::components::settings::integrations_settings::DevicesSettingsPanel;
use t3code_web::components::settings::projects_settings::ProjectsSettingsPanel;
use t3code_web::components::settings::provider_settings_panel::ProviderSettingsPanel;
use t3code_web::components::settings::scheduled_tasks_settings::ScheduledTasksSettingsPanel;
use t3code_web::components::settings::server_settings_data::FixtureServerSettings;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::routes::settings::SettingsRoute;
use t3code_web::scenes::settings_server::{ServerSettingsScene, Variant, build_scene};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

struct Harness {
    cx: HeadlessAppContext,
    handle: AnyWindowHandle,
    scene: Entity<ServerSettingsScene>,
    source: Arc<FixtureServerSettings>,
    out: PathBuf,
    transcript: String,
    failures: usize,
    seen_calls: usize,
}

impl Harness {
    fn open(out: &Path) -> anyhow::Result<Self> {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(|cx| {
            t3code_web::init(cx);
            ThemeSettings::set_mode(ThemeMode::Dark, cx);
        });
        let mut scene = None;
        let (handle, _) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(1440.), px(1000.)), cx)),
                show: false,
                focus: true,
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                let s = build_scene(Variant::Providers, window, cx);
                scene = Some(s.clone());
                s
            })
        })?;
        let scene = scene.context("scene built")?;
        let source = cx.update(|cx| scene.read(cx).source.clone());
        let mut h = Self { cx, handle, scene, source, out: out.to_owned(), transcript: String::new(), failures: 0, seen_calls: 0 };
        h.frame();
        Ok(h)
    }

    fn frame(&mut self) {
        self.cx.run_until_parked();
        self.cx.update_window(self.handle, |_, window, cx| window.render_frame(cx)).ok();
    }

    fn note(&mut self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
        self.transcript.push_str(line.as_ref());
        self.transcript.push('\n');
    }

    fn click(&mut self, id: &str) {
        self.frame();
        let id = id.to_owned();
        let found = self.cx.update_window(self.handle, |_, window, cx| {
            if window.try_find(SharedString::from(id.clone())).is_some() {
                window.click(SharedString::from(id.clone()), cx);
                true
            } else {
                false
            }
        });
        self.frame();
        match found {
            Ok(true) => self.note(format!("click {id}")),
            _ => self.fail(format!("click {id}: element not found")),
        }
    }

    fn type_into(&mut self, field: Option<Entity<InputState>>, text: &str, label: &str) {
        let Some(field) = field else { return self.fail(format!("type {label}: no field")) };
        let text = text.to_owned();
        self.cx
            .update_window(self.handle, |_, window, cx| {
                field.update(cx, |state, cx| state.set_value("", window, cx));
                let focus = field.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                window.input(&text, cx);
            })
            .ok();
        self.frame();
        self.note(format!("type {label} = {text:?}"));
    }

    fn fail(&mut self, line: String) {
        self.failures += 1;
        self.note(format!("FAIL {line}"));
    }

    fn expect(&mut self, ok: bool, what: &str) {
        if ok { self.note(format!("ok   {what}")) } else { self.fail(what.to_owned()) }
    }

    /// The RPCs sent since the last call, as a transcript line.
    fn calls(&mut self) -> Vec<String> {
        let all = self.source.calls.lock().unwrap().clone();
        let new = all[self.seen_calls..].to_vec();
        self.seen_calls = all.len();
        for call in &new {
            self.note(format!("rpc  {call}"));
        }
        new
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        let path = self.out.join(format!("{name}.png"));
        self.cx.capture_screenshot(self.handle)?.save(&path)?;
        self.note(format!("shot {}", path.display()));
        Ok(())
    }

    fn navigate(&mut self, path: SettingsPath) {
        let settings: Entity<SettingsRoute> = self.cx.update(|cx| self.scene.read(cx).settings.clone());
        self.cx.update(|cx| settings.update(cx, |s, cx| s.navigate(path, cx)));
        self.frame();
        self.note(format!("navigate {}", path.slug()));
    }

    fn page<T: 'static>(&mut self, path: SettingsPath) -> Option<Entity<T>> {
        self.cx.update(|cx| self.scene.read(cx).pages.read(cx).typed::<T>(path))
    }

    fn script_auth(&mut self, instance: &str, states: Vec<serde_json::Value>) {
        let states: Vec<ProviderAuthState> = states.into_iter().map(|s| serde_json::from_value(s).expect("auth state")).collect();
        self.source.auth_script.lock().unwrap().insert(instance.to_owned(), states);
    }

    fn auth_phase(&mut self, instance: &str) -> String {
        let panel = self.page::<ProviderSettingsPanel>(SettingsPath::Providers).expect("providers page");
        self.cx.update(|cx| {
            panel.read(cx).model().read(cx).snapshot().provider_auth.get(instance).map(|a| a.phase.as_str().to_owned()).unwrap_or_default()
        })
    }
}

fn auth(phase: &str, interaction: serde_json::Value) -> serde_json::Value {
    json!({ "instanceId": "claudeAgent", "phase": phase, "flowId": "flow-2", "authorizationUrl": null, "expiresAt": null, "message": null, "interaction": interaction })
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).context("usage: server_settings_check <out dir>")?);
    std::fs::create_dir_all(&out)?;
    let mut h = Harness::open(&out)?;
    let providers = h.page::<ProviderSettingsPanel>(SettingsPath::Providers).context("providers page")?;

    // Add provider: Grok, label "Team", no config, create.
    h.note("== add provider wizard");
    h.click("providers-add");
    h.shot("check-01-add-step-provider")?;
    h.click("add-driver-grok");
    h.click("add-next");
    let label = h.cx.update(|cx| providers.read(cx).add_dialog().map(|d| d.read(cx).identity_fields().0.clone()));
    h.type_into(label, "Team", "label");
    let resolved = h.cx.update(|cx| providers.read(cx).add_dialog().map(|d| d.read(cx).resolved_instance_id(cx)));
    h.expect(resolved.as_deref() == Some("grok_team"), &format!("instance id derived from label: {resolved:?}"));
    h.shot("check-02-add-step-identity")?;
    h.click("add-next");
    h.shot("check-03-add-step-config")?;
    h.click("add-submit");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "server.updateSettings"), "create sent with server.updateSettings");
    let created = h.cx.update(|cx| providers.read(cx).model().read(cx).snapshot().settings.provider_instances.clone().unwrap_or_default().contains_key("grok_team"));
    h.expect(created, "grok_team stored in providerInstances");
    h.shot("check-04-add-created")?;

    // Toggle an instance off from the list.
    h.note("== enable switch");
    h.click("provider-enable-codex_work");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "server.updateSettings"), "disable sent with server.updateSettings");
    let enabled = h.cx.update(|cx| providers.read(cx).model().read(cx).snapshot().providers.iter().find(|p| p.instance_id == "codex_work").map(|p| p.enabled));
    h.expect(enabled == Some(false), "codex_work now disabled");

    // Device code: copy, then cancel.
    h.note("== device-code sign-in");
    h.click("provider-row-claudeAgent");
    h.shot("check-05-device-code")?;
    h.click("auth-copy-code-claudeAgent");
    let clip = h.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    h.expect(clip.as_deref() == Some("QFXK-7RMB"), &format!("device code copied: {clip:?}"));
    h.click("auth-cancel-claudeAgent");
    h.calls();
    let phase = h.auth_phase("claudeAgent");
    h.expect(phase == "cancelled", &format!("cancelled ({phase})"));

    // Browser with a pasted callback URL.
    h.note("== browser sign-in with pasted callback");
    h.script_auth("claudeAgent", vec![auth("waiting", json!({ "type": "browser", "id": "b-1", "url": "https://claude.ai/oauth/authorize?x=1", "requiresConsent": false, "acceptsCallback": true }))]);
    h.click("auth-start-claudeAgent");
    h.shot("check-06-browser-callback")?;
    let field = h.cx.update(|cx| providers.read(cx).field("claudeAgent:auth:callback"));
    h.type_into(field, "http://localhost:1455/auth/callback?code=abc&state=xyz", "callback");
    h.click("auth-callback-claudeAgent");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "provider.auth.complete claudeAgent"), "callback sent with provider.auth.complete");
    let phase = h.auth_phase("claudeAgent");
    h.expect(phase == "succeeded", &format!("succeeded ({phase})"));

    // Credentials.
    h.note("== credentials sign-in");
    h.script_auth("claudeAgent", vec![
        auth("waiting", json!({ "type": "credentials", "id": "c-1", "fields": [{ "name": "apiKey", "label": "API key", "secret": true }] })),
        auth("failed", serde_json::Value::Null),
    ]);
    h.click("auth-start-claudeAgent");
    let field = h.cx.update(|cx| providers.read(cx).field("claudeAgent:auth:cred:apiKey"));
    h.type_into(field, "sk-test", "apiKey");
    h.shot("check-07-credentials")?;
    h.click("auth-credentials-claudeAgent");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "provider.auth.respond claudeAgent"), "credentials sent with provider.auth.respond");

    // Terminal login.
    h.note("== terminal sign-in");
    h.script_auth("claudeAgent", vec![auth("waiting", json!({ "type": "terminal", "id": "t-1", "output": "Welcome to Claude Code\nPaste the code from your browser:\n" }))]);
    h.click("auth-start-claudeAgent");
    let field = h.cx.update(|cx| providers.read(cx).field("claudeAgent:auth:terminal"));
    h.type_into(field, "ABC123", "terminal line");
    h.shot("check-08-terminal")?;
    h.click("auth-terminal-send-claudeAgent");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "provider.auth.respond claudeAgent"), "terminal line sent with provider.auth.respond");

    // Scheduled task editor: each kind, then validation.
    h.note("== scheduled task editor");
    h.navigate(SettingsPath::ScheduledTasks);
    h.click("task-new");
    let tasks = h.page::<ScheduledTasksSettingsPanel>(SettingsPath::ScheduledTasks).context("tasks page")?;
    for (id, name) in [("schedule-kind-Fixed", "fixed"), ("schedule-kind-Once", "once"), ("schedule-kind-Interval", "interval"), ("schedule-kind-Webhook", "webhook")] {
        h.click(id);
        h.shot(&format!("check-09-task-{name}"))?;
    }
    h.click("filter-add");
    h.click("task-save");
    let errors = h.cx.update(|cx| tasks.read(cx).editor().map(|e| e.read(cx).draft.title.is_empty()));
    h.expect(errors == Some(true), "save with an empty title stays open");
    h.shot("check-10-task-validation")?;
    h.click("task-cancel");
    h.click("task-enabled-task-hook");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "scheduledTasks.setEnabled task-hook true"), "enable switch sends scheduledTasks.setEnabled");

    // Device host editor.
    h.note("== device host editor");
    h.navigate(SettingsPath::Devices);
    h.click("device-host-add");
    h.shot("check-11-device-host")?;
    let devices = h.page::<DevicesSettingsPanel>(SettingsPath::Devices).context("devices page")?;
    let _ = devices;
    h.click("device-cancel");

    // GitHub device-code sign-in.
    h.note("== source control sign-in");
    h.navigate(SettingsPath::SourceControl);
    h.click("sc-sign-in-github.com");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "sourceControl.signInStart github.com"), "sourceControl.signInStart sent");
    h.expect(calls.iter().any(|c| c == "sourceControl.signInWait github.com"), "sourceControl.signInWait sent");
    h.shot("check-12-github-device-code")?;
    h.click("sc-cancel-github.com");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "sourceControl.signInCancel github.com"), "sourceControl.signInCancel sent");

    // Add to another machine: folder browser.
    h.note("== add project to machine");
    h.navigate(SettingsPath::Projects);
    h.click("project-add-to-machine");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c.starts_with("filesystem.browse")), "filesystem.browse on open");
    h.click("folder-~/t3code");
    let calls = h.calls();
    h.expect(calls.iter().any(|c| c == "filesystem.browse ~/t3code/"), "picking a folder browses into it");
    let projects = h.page::<ProjectsSettingsPanel>(SettingsPath::Projects).context("projects page")?;
    let open: Option<Entity<AddProjectToMachineDialog>> = h.cx.update(|cx| projects.read(cx).dialog().cloned());
    h.expect(open.is_some(), "dialog open");
    h.shot("check-13-add-to-machine")?;

    std::fs::write(out.join("transcript.txt"), &h.transcript)?;
    if h.failures > 0 {
        bail!("{} checks failed", h.failures);
    }
    println!("all checks passed");
    Ok(())
}
