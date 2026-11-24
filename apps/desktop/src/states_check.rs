// Produces every connection state the Environments panel and the status dot show, for real,
// against an isolated T3 Code server, by breaking the right thing, and screenshots each one.
// The app root and connection runtime run in a headless window. The stand-in's direct-route
// proxy (`STANDIN_CONTROL`, address `T3_DIRECT_URL`) stands for one direct address of the
// server: it can go silent, refuse, or point at a second server. Phases share a data dir:
//
//   states   pair (connecting, connected), switch off and on, reconnecting with its
//            countdown (address refuses), descriptor mismatch (address now reaches server B on
//            `T3_OTHER_PORT`), credential rejected (`T3_REVOKE_CMD` revokes this device's
//            session on the host); `STATES_CLIENT_VERSION` newer than the host's shows the
//            version mismatch
//   routing  the host listens on every interface and advertises LAN and Tailscale addresses:
//            learned routes and their kinds, the cleartext rule, the 500 ms head start past a
//            silent preferred route, promotion back to it, and a route order the user changed
//   offline  run inside a network namespace with no route: the app reports itself offline
//   wake     the open socket stalls (as one the host dropped while a laptop slept) while the
//            host still answers new requests; the probe a wake sends replaces it within a
//            second, instead of waiting out the socket's 15 s silence timer
//
// `T3_PAIR_CMD` prints a fresh pairing credential.
// Usage: states-check <states|routing|offline|wake> <out-dir> <data-dir>
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_client_runtime::connection::ConnectionPhase;
use t3code_client_runtime::connection::model::{ConnectionErrorKind, NetworkStatus};
use t3code_client_runtime::connection::onboarding::parse_pairing_url;
use t3code_client_runtime::connection::routes::{RouteKind, is_learned, route_id, route_kind, usable_routes};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::connection::presentation::{diagnose, row_status, status_text, transport_label, version_mismatch};
use t3code_web::connection::shell::EnvironmentShell;
use t3code_web::connection::{ConnectionRuntime, ConnectionsState, EnvironmentState, RuntimeConfig};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
    failures: usize,
    handle: AnyWindowHandle,
    shell: Entity<EnvironmentShell>,
    control: String,
}

impl Check {
    fn note(&mut self, text: impl AsRef<str>) {
        let line = format!("{:>8.3}s  {}", self.started.elapsed().as_secs_f64(), text.as_ref());
        println!("{line}");
        writeln!(self.log, "{line}").ok();
    }

    fn expect(&mut self, ok: bool, what: impl AsRef<str>) {
        if ok {
            self.note(format!("ok: {}", what.as_ref()));
        } else {
            self.failures += 1;
            self.note(format!("FAIL: {}", what.as_ref()));
        }
    }

    fn frame(&mut self) -> anyhow::Result<()> {
        self.cx.update_window(self.handle, |_, w, cx| w.render_frame(cx))?;
        self.cx.run_until_parked();
        self.cx.update_window(self.handle, |_, w, cx| w.render_frame(cx))?;
        Ok(())
    }

    /// Screenshots the window and records what the row says.
    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame()?;
        let image = self.cx.capture_screenshot(self.handle)?;
        image.save(self.out.join(name))?;
        let rows = self.state(|s| s.environments.iter().map(describe_row).collect::<Vec<_>>());
        self.note(format!("screenshot {name}"));
        for row in rows {
            self.note(format!("    {row}"));
        }
        Ok(())
    }

    fn state<R>(&mut self, read: impl FnOnce(&ConnectionsState) -> R) -> R {
        self.cx.update(|cx| read(ConnectionRuntime::state(cx).read(cx)))
    }

    fn env<R>(&mut self, read: impl FnOnce(&EnvironmentState) -> R) -> R {
        self.state(|s| read(&s.environments[0]))
    }

    fn wait(&mut self, what: &str, timeout: Duration, ready: impl Fn(&ConnectionsState) -> bool) -> anyhow::Result<Duration> {
        let start = Instant::now();
        let deadline = start + timeout;
        let mut last = String::new();
        loop {
            self.frame()?;
            let summary = self.state(summarize);
            if summary != last {
                self.note(format!("state: {summary}"));
                last = summary;
            }
            if self.state(&ready) {
                let took = start.elapsed();
                self.note(format!("ok: {what} ({:.2}s)", took.as_secs_f64()));
                return Ok(took);
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn control(&mut self, method: &str, path: &str) -> anyhow::Result<String> {
        let mut stream = std::net::TcpStream::connect(&self.control)?;
        write!(stream, "{method} {path} HTTP/1.0\r\nHost: control\r\nContent-Length: 0\r\n\r\n")?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer)?;
        self.note(format!("control {method} {path}"));
        Ok(answer.split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default())
    }

    fn command(&mut self, env_var: &str, args: &str) -> anyhow::Result<String> {
        let command = std::env::var(env_var).with_context(|| env_var.to_owned())?;
        let output = std::process::Command::new("sh").arg("-c").arg(format!("{command} {args}")).output()?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn open_environments(&mut self) -> anyhow::Result<()> {
        let shell = self.shell.clone();
        self.cx.update_window(self.handle, |_, window, cx| {
            shell.update(cx, |shell, cx| shell.open_settings(SettingsPath::Connections, window, cx));
        })?;
        self.frame()
    }

    fn open_row(&mut self, routes: bool, details: bool) -> anyhow::Result<()> {
        let id = self.env(|e| e.id().to_string());
        let shell = self.shell.clone();
        self.cx.update_window(self.handle, |_, _, cx| {
            let settings = shell.read(cx).settings().cloned().context("settings are open")?;
            let page = settings.read(cx).environments().clone();
            page.update(cx, |page, cx| page.toggle_for_scene(&id, routes, details, cx));
            anyhow::Ok(())
        })??;
        self.frame()
    }

    /// Pairs the direct address with a fresh credential from the host.
    fn pair(&mut self, base: &str) -> anyhow::Result<()> {
        let credential = self.command("T3_PAIR_CMD", "")?;
        let request = parse_pairing_url(&format!("{base}#token={credential}")).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.cx.update(|cx| ConnectionRuntime::global(cx).pair(request, None, cx).detach());
        self.note(format!("pairing {base}"));
        Ok(())
    }

    fn set_enabled(&mut self, enabled: bool) -> anyhow::Result<()> {
        let id = self.env(|e| e.id().clone());
        self.cx.update(|cx| ConnectionRuntime::global(cx).set_enabled(id, enabled, cx).detach());
        self.note(format!("switch {}", if enabled { "on" } else { "off" }));
        Ok(())
    }

    fn reorder(&mut self, order: Vec<String>) -> anyhow::Result<()> {
        let id = self.env(|e| e.id().clone());
        self.note(format!("route order {order:?}"));
        self.cx.update(|cx| ConnectionRuntime::global(cx).reorder_routes(id, order, cx).detach());
        Ok(())
    }

    fn retry(&mut self) {
        let id = self.env(|e| e.id().clone());
        self.cx.update(|cx| ConnectionRuntime::global(cx).retry(&id));
    }
}

fn phase_text(env: &EnvironmentState) -> String {
    match &env.status {
        None if !env.entry.enabled => "off".to_owned(),
        None => "starting".to_owned(),
        Some(status) => format!(
            "{:?}{}{}",
            status.phase,
            status.active_route_id.as_deref().map(|r| format!(" via {r}")).unwrap_or_default(),
            status.last_failure.as_ref().map(|f| format!(" ({})", f.detail)).unwrap_or_default()
        ),
    }
}

fn summarize(state: &ConnectionsState) -> String {
    state.environments.iter().map(|env| format!("{} {}", env.entry.label, phase_text(env))).collect::<Vec<_>>().join("; ")
}

/// The row as a user reads it: subtitle, tooltip, diagnosis headline.
fn describe_row(env: &EnvironmentState) -> String {
    let (status, tone) = row_status(env);
    let diagnosis = diagnose(env);
    let subtitle = if diagnosis.is_some() {
        transport_label(env).to_string()
    } else {
        format!("{} · {status}", transport_label(env))
    };
    let headline = diagnosis.map(|d| format!(" | headline: {} | steps: {}", d.headline, d.steps.join(" / "))).unwrap_or_default();
    format!("row: {subtitle} ({tone:?}) | tooltip: {}{headline}", status_text(env))
}

fn connected(state: &ConnectionsState) -> bool {
    state.environments.first().and_then(|e| e.status.as_ref()).is_some_and(|s| s.phase == ConnectionPhase::Connected)
}

fn active_route(state: &ConnectionsState) -> Option<String> {
    state.environments.first()?.status.as_ref().filter(|s| s.phase == ConnectionPhase::Connected)?.active_route_id.clone()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: states-check <states|routing|offline|wake> <out-dir> <data-dir>";
    let phase = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    let data_dir = PathBuf::from(args.next().context(usage)?);
    std::fs::create_dir_all(&out)?;
    let control = std::env::var("STANDIN_CONTROL").unwrap_or_else(|_| "127.0.0.1:39446".into());
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let mut presentation = ClientPresentation { label: "t3code-gpui states check".into(), ..Default::default() };
    if let Ok(version) = std::env::var("STATES_CLIENT_VERSION") {
        presentation.app_version = version;
    }
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data_dir.join("secrets.json")))),
        data_dir,
        cloud: None,
        presentation,
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
        let opened = WindowRoot::open_window(options, app, cx).map(|(handle, root)| (handle, root.shell));
        ThemeSettings::apply_env_overrides(cx);
        opened
    })?;
    let mut check = Check { cx, out: out.clone(), started: Instant::now(), log: String::new(), failures: 0, handle, shell, control };
    check.note(format!("phase {phase}"));
    let result = match phase.as_str() {
        "states" => run_states(&mut check),
        "routing" => run_routing(&mut check),
        "offline" => run_offline(&mut check),
        "wake" => run_wake(&mut check),
        other => bail!("unknown phase {other}"),
    };
    if let Err(error) = &result {
        check.note(format!("FAILED: {error:#}"));
        check.shot(&format!("{phase}-failure.png")).ok();
    }
    check.cx.update(ConnectionRuntime::shutdown);
    check.note(format!("runtime shut down; {} failed expectations", check.failures));
    std::fs::write(out.join(format!("{phase}-transcript.txt")), &check.log)?;
    result?;
    if check.failures > 0 {
        bail!("{} expectations failed", check.failures);
    }
    Ok(())
}

fn run_states(check: &mut Check) -> anyhow::Result<()> {
    let direct = std::env::var("T3_DIRECT_URL").context("T3_DIRECT_URL")?;
    let other_port = std::env::var("T3_OTHER_PORT").context("T3_OTHER_PORT")?;
    let port = std::env::var("T3_SERVER_PORT").unwrap_or_else(|_| "39410".into());
    check.control("POST", &format!("/direct/up?port={port}"))?;
    check.open_environments()?;
    check.pair(&direct)?;
    check.wait("the paired environment appears", Duration::from_secs(20), |s| !s.environments.is_empty())?;
    check.wait("connected", Duration::from_secs(20), connected)?;
    check.wait("latency measured", Duration::from_secs(40), |s| s.environments[0].latency.is_some())?;
    check.open_row(true, false)?;
    check.shot("s02-connected-loopback.png")?;
    let client = check.cx.update(|cx| ConnectionRuntime::global(cx).auth().presentation().app_version.clone());
    let server = check.env(|e| e.server_version.clone());
    let mismatch = version_mismatch(server.as_deref(), &client);
    check.note(format!("client {client}, host {server:?}, mismatch {mismatch:?}"));
    if std::env::var("STATES_CLIENT_VERSION").is_ok() {
        check.expect(mismatch.is_some(), "a host older than the app shows the version mismatch");
        check.shot("s03-version-mismatch.png")?;
        return Ok(());
    }

    check.set_enabled(false)?;
    check.wait("switched off", Duration::from_secs(10), |s| s.environments[0].status.is_none() && !s.environments[0].entry.enabled)?;
    check.shot("s04-disabled.png")?;
    // Switched on while the address is silent: the first attempt is still connecting.
    check.control("POST", "/direct/silent")?;
    check.set_enabled(true)?;
    check.wait("connecting", Duration::from_secs(10), |s| {
        s.environments[0].status.as_ref().is_some_and(|st| st.phase == ConnectionPhase::Connecting)
    })?;
    check.shot("s01-connecting.png")?;
    check.control("POST", "/direct/up")?;
    check.retry();
    check.wait("connected again", Duration::from_secs(30), connected)?;

    // The address starts refusing: the socket drops and retries back off with a countdown.
    check.control("POST", "/direct/down")?;
    check.wait("reconnecting with a retry scheduled", Duration::from_secs(30), |s| {
        s.environments[0].status.as_ref().is_some_and(|st| st.phase == ConnectionPhase::Backoff && st.retry_at.is_some() && st.attempt >= 2)
    })?;
    check.open_row(false, true)?;
    check.shot("s05-reconnecting-countdown.png")?;
    check.control("POST", "/direct/up")?;
    check.retry();
    check.wait("reconnected once the address answers", Duration::from_secs(20), connected)?;

    // The saved address now reaches another machine: the token is never sent there.
    check.control("POST", &format!("/direct/up?port={other_port}"))?;
    check.wait("descriptor mismatch", Duration::from_secs(30), |s| {
        s.environments[0].status.as_ref().and_then(|st| st.last_failure.as_ref()).is_some_and(|f| f.detail.contains("does not match"))
    })?;
    check.shot("s06-descriptor-mismatch.png")?;
    check.control("POST", &format!("/direct/up?port={port}"))?;
    check.retry();
    check.wait("reconnected to the right host", Duration::from_secs(20), connected)?;

    // The host revokes this device's session.
    let revoked = check.command("T3_REVOKE_CMD", "")?;
    check.note(format!("revoke: {revoked}"));
    check.wait("credential rejected", Duration::from_secs(30), |s| {
        s.environments[0].status.as_ref().is_some_and(|st| {
            st.phase == ConnectionPhase::Blocked
                && st.last_failure.as_ref().is_some_and(|f| f.kind == ConnectionErrorKind::Blocked(t3code_client_runtime::connection::model::BlockedReason::Authentication))
        })
    })?;
    check.shot("s07-credential-rejected.png")?;
    let headline = check.env(|e| diagnose(e).map(|d| d.headline.to_string()).unwrap_or_default());
    check.expect(headline.contains("did not accept this device"), format!("headline: {headline}"));
    Ok(())
}

fn run_routing(check: &mut Check) -> anyhow::Result<()> {
    let direct = std::env::var("T3_DIRECT_URL").context("T3_DIRECT_URL")?;
    let port = std::env::var("T3_SERVER_PORT").unwrap_or_else(|_| "39410".into());
    check.control("POST", &format!("/direct/up?port={port}"))?;
    check.open_environments()?;
    check.pair(&direct)?;
    check.wait("connected over the paired loopback address", Duration::from_secs(30), connected)?;
    let paired = check.env(|e| route_id(&e.entry.routes[0].target).to_owned());
    check.wait("learned the host's Tailscale addresses", Duration::from_secs(30), |s| {
        s.environments[0].entry.routes.iter().any(|r| is_learned(r) && route_kind(r) == RouteKind::Tailnet)
    })?;
    let kinds: Vec<String> = check.env(|e| {
        e.entry.routes.iter().map(|r| format!("{} {:?} learned={}", route_id(&r.target), route_kind(r), is_learned(r))).collect()
    });
    for kind in &kinds {
        check.note(format!("route {kind}"));
    }
    let learned_lan = check.env(|e| e.entry.routes.iter().any(|r| is_learned(r) && route_kind(r) == RouteKind::Lan));
    check.expect(
        !learned_lan,
        "the host's plain-HTTP LAN addresses are not learned for a loopback-paired token (it never goes over cleartext)",
    );
    let usable: Vec<String> = check.env(|e| usable_routes(&e.entry).iter().map(|r| route_id(&r.target).to_owned()).collect());
    let tailnet_usable = check.env(|e| {
        e.entry.routes.iter().filter(|r| route_kind(r) == RouteKind::Tailnet).all(|r| usable.contains(&route_id(&r.target).to_owned()))
    });
    check.expect(tailnet_usable, "the Tailscale addresses may carry it");
    let tailnet_ids: Vec<String> = check.env(|e| {
        e.entry.routes.iter().filter(|r| route_kind(r) == RouteKind::Tailnet).map(|r| route_id(&r.target).to_owned()).collect()
    });
    check.open_row(true, false)?;
    check.shot("r01-learned-routes.png")?;

    // The preferred address goes silent: after the 500 ms head start the next route connects.
    check.control("POST", "/direct/silent")?;
    let took = check.wait("connected through Tailscale while loopback is silent", Duration::from_secs(30), |s| {
        active_route(s).is_some_and(|r| tailnet_ids.contains(&r))
    })?;
    check.expect(took < Duration::from_secs(5), format!("fell over in {:.2}s", took.as_secs_f64()));
    check.shot("r02-connected-tailscale.png")?;
    // It answers again: the minute's better-route check moves back to it.
    check.control("POST", "/direct/up")?;
    let took = check.wait("promoted back to the preferred loopback route", Duration::from_secs(80), |s| {
        active_route(s).as_deref() == Some(paired.as_str())
    })?;
    check.expect(took <= Duration::from_secs(70), format!("promotion took {:.1}s (check interval 60 s)", took.as_secs_f64()));
    check.shot("r03-promoted-back.png")?;

    // The user puts Tailscale first: the next connect follows that order.
    let mut order: Vec<String> = check.env(|e| e.entry.routes.iter().map(|r| route_id(&r.target).to_owned()).collect());
    order.sort_by_key(|id| !tailnet_ids.contains(id));
    check.reorder(order)?;
    let took = check.wait("the better-route check moves to the Tailscale routes now first", Duration::from_secs(80), |s| {
        active_route(s).is_some_and(|r| tailnet_ids.contains(&r))
    })?;
    check.note(format!("followed the new order after {:.1}s", took.as_secs_f64()));
    check.shot("r04-user-order.png")?;

    // Pairing the host's LAN address too (Add route): a token paired over plain HTTP on the
    // LAN may use that LAN address; put it first and it connects there.
    if let Ok(lan) = std::env::var("T3_LAN_URL") {
        let id = check.env(|e| e.id().clone());
        let credential = check.command("T3_PAIR_CMD", "")?;
        let request = parse_pairing_url(&format!("{lan}#token={credential}")).map_err(|e| anyhow::anyhow!("{e}"))?;
        check.cx.update(|cx| ConnectionRuntime::global(cx).pair(request, Some(id), cx).detach());
        check.wait("the LAN route was added", Duration::from_secs(20), |s| {
            s.environments[0].entry.routes.iter().any(|r| !is_learned(r) && route_kind(r) == RouteKind::Lan)
        })?;
        let lan_id = check.env(|e| {
            e.entry.routes.iter().find(|r| !is_learned(r) && route_kind(r) == RouteKind::Lan).map(|r| route_id(&r.target).to_owned())
        });
        let mut order: Vec<String> = check.env(|e| e.entry.routes.iter().map(|r| route_id(&r.target).to_owned()).collect());
        if let Some(lan_id) = &lan_id {
            order.retain(|id| id != lan_id);
            order.insert(0, lan_id.clone());
        }
        check.reorder(order)?;
        check.retry();
        check.wait("connected over the LAN", Duration::from_secs(90), |s| active_route(s) == lan_id)?;
        check.shot("r05-connected-lan.png")?;
    }
    Ok(())
}

fn run_offline(check: &mut Check) -> anyhow::Result<()> {
    check.open_environments()?;
    check.wait("the app noticed it has no network", Duration::from_secs(20), |s| {
        s.environments.first().and_then(|e| e.status.as_ref()).is_some_and(|st| st.network == NetworkStatus::Offline)
    })?;
    check.wait("offline state", Duration::from_secs(10), |s| {
        s.environments[0].status.as_ref().is_some_and(|st| st.phase == ConnectionPhase::Offline)
    })?;
    check.open_row(false, true)?;
    check.shot("s08-offline.png")?;
    Ok(())
}

fn run_wake(check: &mut Check) -> anyhow::Result<()> {
    let direct = std::env::var("T3_DIRECT_URL").context("T3_DIRECT_URL")?;
    let port = std::env::var("T3_SERVER_PORT").unwrap_or_else(|_| "39410".into());
    check.control("POST", &format!("/direct/up?port={port}"))?;
    check.pair(&direct)?;
    check.wait("connected", Duration::from_secs(30), connected)?;
    check.wait("latency measured", Duration::from_secs(40), |s| s.environments[0].latency.is_some())?;
    let generation = check.env(|e| e.status.as_ref().map_or(0, |s| s.generation));
    check.control("POST", "/direct/stall")?;
    // What a wake (or a network change) sends every supervisor.
    check.cx.update(|cx| ConnectionRuntime::global(cx).registry().broadcast(t3code_client_runtime::connection::SupervisorSignal::Wakeup));
    let took = check.wait("a new socket replaced the stalled one", Duration::from_secs(20), |s| {
        s.environments[0].status.as_ref().is_some_and(|st| st.phase == ConnectionPhase::Connected && st.generation > generation)
    })?;
    check.expect(took < Duration::from_secs(3), format!("replaced {:.2}s after the wake (socket timer: 15 s)", took.as_secs_f64()));
    check.control("POST", "/direct/up")?;
    Ok(())
}
