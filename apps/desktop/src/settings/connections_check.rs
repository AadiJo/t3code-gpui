// End-to-end check of the environment screens against a real T3 Code
// server, in a headless window running the real app root and connection
// runtime. Run `pair` once, then `restart` as a new process on the same
// data directory.
//
// Usage: connections-check <out-dir> <data-dir> pair     (needs T3_HOST, T3_CODE)
//        connections-check <out-dir> <data-dir> restart
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_client_runtime::connection::ConnectionPhase;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_client_runtime::relay::CloudConfig;
use t3code_web::app_root::{AppRootOptions, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::connection::shell::EnvironmentShell;
use t3code_web::connection::{ConnectionRuntime, ConnectionsState, RuntimeConfig};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
    handle: AnyWindowHandle,
    shell: Entity<EnvironmentShell>,
}

impl Check {
    fn note(&mut self, text: impl AsRef<str>) {
        let line = format!("{:>8.3}s  {}", self.started.elapsed().as_secs_f64(), text.as_ref());
        println!("{line}");
        writeln!(self.log, "{line}").ok();
    }

    fn frame(&mut self) -> anyhow::Result<()> {
        self.cx.update_window(self.handle, |_, w, cx| w.render_frame(cx))?;
        self.cx.run_until_parked();
        self.cx.update_window(self.handle, |_, w, cx| w.render_frame(cx))?;
        Ok(())
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame()?;
        let image = self.cx.capture_screenshot(self.handle)?;
        image.save(self.out.join(name))?;
        self.note(format!("screenshot {name}"));
        Ok(())
    }

    fn click(&mut self, id: &'static str) -> anyhow::Result<()> {
        self.frame()?;
        self.cx.update_window(self.handle, |_, w, cx| w.click(id, cx))?;
        self.frame()?;
        self.note(format!("click {id}"));
        Ok(())
    }

    /// A left click at a window point, for chrome this check does not own.
    fn click_point(&mut self, what: &str, at: Point<Pixels>) -> anyhow::Result<()> {
        use gpui_kit::test::TestEventExt as _;
        self.frame()?;
        self.cx.update_window(self.handle, |_, w, cx| {
            w.pointer_move(at, None, cx);
            w.pointer_down(at, MouseButton::Left, cx);
            w.pointer_up(at, MouseButton::Left, cx);
        })?;
        self.frame()?;
        self.note(format!("click {what}"));
        Ok(())
    }

    fn click_owned(&mut self, id: String) -> anyhow::Result<()> {
        self.frame()?;
        let element = SharedString::from(id.clone());
        self.cx.update_window(self.handle, |_, w, cx| w.click(element, cx))?;
        self.frame()?;
        self.note(format!("click {id}"));
        Ok(())
    }

    fn type_into(&mut self, id: &'static str, text: &str, shown: &str) -> anyhow::Result<()> {
        // Below the field's label, inside the input.
        self.frame()?;
        let bounds = self.cx.update_window(self.handle, |_, w, _| w.find(id).bounds())?;
        self.note(format!("{id} bounds {bounds:?}"));
        self.cx.update_window(self.handle, |_, w, cx| w.click_at(id, point(px(24.), px(38.)), cx))?;
        self.frame()?;
        let shell = self.shell.clone();
        let focused = self.cx.update_window(self.handle, |_, w, cx| {
            let settings = shell.read(cx).settings().cloned();
            let page = settings.map(|s| s.read(cx).environments().clone());
            let dialog = page.and_then(|p| p.read(cx).add_dialog().cloned());
            dialog.map(|d| {
                let (host, code) = d.read(cx).fields();
                (host.read(cx).focus_handle(cx).is_focused(w), code.read(cx).focus_handle(cx).is_focused(w))
            })
        })?;
        self.note(format!("focus (host, code): {focused:?}"));
        self.cx.update_window(self.handle, |_, w, cx| w.input(text, cx))?;
        self.frame()?;
        self.note(format!("typed {shown} into {id}"));
        Ok(())
    }

    fn state<R>(&mut self, read: impl FnOnce(&ConnectionsState) -> R) -> R {
        self.cx.update(|cx| read(ConnectionRuntime::state(cx).read(cx)))
    }

    /// Pumps the app until `ready` holds, logging each new status line.
    fn wait(&mut self, what: &str, timeout: Duration, ready: impl Fn(&ConnectionsState) -> bool) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        loop {
            self.frame()?;
            let summary = self.state(summarize);
            if summary != last {
                self.note(format!("state: {summary}"));
                last = summary;
            }
            if self.state(&ready) {
                self.note(format!("ok: {what}"));
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn summarize(state: &ConnectionsState) -> String {
    if state.environments.is_empty() {
        return "no environments".into();
    }
    state
        .environments
        .iter()
        .map(|env| {
            let phase = match &env.status {
                None if !env.entry.enabled => "off".to_owned(),
                None => "starting".to_owned(),
                Some(status) => format!(
                    "{:?}{}{}",
                    status.phase,
                    status.active_route_id.as_deref().map(|r| format!(" via {r}")).unwrap_or_default(),
                    status.last_failure.as_ref().map(|f| format!(" ({})", f.detail)).unwrap_or_default()
                ),
            };
            format!(
                "{} [{}] {}{}",
                env.entry.label,
                env.entry.environment_id,
                phase,
                env.latency.map(|l| format!(" {}ms", l.as_millis())).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn connected(state: &ConnectionsState) -> bool {
    !state.environments.is_empty()
        && state.environments.iter().all(|env| env.status.as_ref().is_some_and(|s| s.phase == ConnectionPhase::Connected))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: connections-check <out-dir> <data-dir> <pair|restart>";
    let out = PathBuf::from(args.next().context(usage)?);
    let data_dir = PathBuf::from(args.next().context(usage)?);
    let phase = args.next().context(usage)?;
    std::fs::create_dir_all(&out)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data_dir.join("secrets.json")))),
        data_dir,
        cloud: Some(CloudConfig::production()),
        presentation: ClientPresentation { label: "t3code-gpui connections check".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
        ConnectionRuntime::init(config, cx)
    })?;
    let (handle, shell) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(1280.), px(860.)), cx)),
            show: false,
            focus: true,
            ..component::TitleBar::window_options()
        };
        let app = AppRootOptions::without_fixtures(TitlebarPlatform::current());
        WindowRoot::open_window(options, app, cx).map(|(handle, root)| (handle, root.shell))
    })?;
    let mut check = Check { cx, out: out.clone(), started: Instant::now(), log: String::new(), handle, shell };
    check.note(format!("phase {phase}"));
    let result = match phase.as_str() {
        "pair" => run_pair(&mut check),
        "restart" => run_restart(&mut check),
        other => bail!("unknown phase {other}"),
    };
    if let Err(error) = &result {
        check.note(format!("FAILED: {error:#}"));
        check.shot(&format!("{phase}-failure.png")).ok();
    }
    check.cx.update(ConnectionRuntime::shutdown);
    check.note("runtime shut down");
    std::fs::write(out.join(format!("{phase}-transcript.txt")), &check.log)?;
    result
}

fn run_pair(check: &mut Check) -> anyhow::Result<()> {
    let host = std::env::var("T3_HOST").context("T3_HOST")?;
    let code = std::env::var("T3_CODE").context("T3_CODE")?;
    check.wait("empty catalog", Duration::from_secs(5), |s| s.environments.is_empty())?;
    check.shot("01-empty-state.png")?;
    check.click("empty-add-pairing")?;
    let open = check.cx.update(|cx| check.shell.read(cx).settings().is_some());
    check.note(format!("settings open: {open}"));
    check.shot("02-settings-environments.png")?;
    check.click("add-environment")?;
    check.shot("03-add-dialog.png")?;
    check.type_into("add-environment-host", &host, &host)?;
    check.type_into("add-environment-code", &code, "<pairing code>")?;
    check.shot("04-add-dialog-filled.png")?;
    check.click("add-environment-submit")?;
    check.wait("paired and connected", Duration::from_secs(30), connected)?;
    check.wait("latency measured", Duration::from_secs(15), |s| s.environments.iter().all(|e| e.latency.is_some()))?;
    check.shot("05-connected.png")?;
    let id = check.state(|s| s.environments[0].entry.environment_id.to_string());
    check.click_owned(format!("env-routes-toggle-{id}"))?;
    check.shot("06-connected-routes.png")?;
    Ok(())
}

fn run_restart(check: &mut Check) -> anyhow::Result<()> {
    check.wait("reconnected from the saved catalog", Duration::from_secs(30), connected)?;
    check.shot("07-restart-main.png")?;
    // The sidebar's settings gear, bottom left of the 1280x860 window.
    check.click_point("sidebar settings button", point(px(24.), px(836.)))?;
    check.click("settings-nav-connections")?;
    check.wait("latency measured", Duration::from_secs(15), |s| s.environments.iter().all(|e| e.latency.is_some()))?;
    check.shot("08-restart-connected.png")?;
    let id = check.state(|s| s.environments[0].entry.environment_id.to_string());
    check.click_owned(format!("env-switch-{id}"))?;
    check.wait("switched off", Duration::from_secs(10), |s| {
        s.environments.iter().all(|e| !e.entry.enabled && e.status.is_none())
    })?;
    check.shot("09-disabled.png")?;
    check.click_owned(format!("env-switch-{id}"))?;
    check.wait("switched back on and connected", Duration::from_secs(30), connected)?;
    check.click_owned(format!("env-more-{id}"))?;
    check.shot("10-more-menu.png")?;
    // Entries: Routes, Rename, separator, Remove from this device.
    check.frame()?;
    check.cx.update_window(check.handle, |_, w, cx| w.click(("menu-item", 3usize), cx))?;
    check.note("click menu item: Remove from this device…");
    check.shot("11-remove-confirm.png")?;
    check.click("confirm-remove-environment")?;
    check.wait("removed", Duration::from_secs(10), |s| s.environments.is_empty())?;
    check.shot("12-removed.png")?;
    check.click("settings-close")?;
    check.shot("13-empty-state-again.png")?;
    Ok(())
}
