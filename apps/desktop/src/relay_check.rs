// End-to-end check of T3 Connect: the real app root and connection runtime in a headless
// window, against a stand-in relay and Clerk Frontend API in front of a real T3 Code server
// (the stand-in, its tunnel and its control port are outside this repo). Each phase is its own
// process on its own data directory:
//
//   connect   email-code sign-in, relay discovery, connect through the relay (session token,
//             relay DPoP token, connect, bootstrap exchange at the tunnel's /oauth/token, ticket,
//             upgrade with `dpop`), renewal a minute before a shortened expiry with the socket
//             kept, a dropped tunnel socket, a session Clerk rejects (falls back to email-code
//             sign-in and the relay route goes), sign-in again, and sign-out
//   approval  a host that requires device approval: approval-pending with the code and its
//             4 s retry, then approval through the host's CLI (`T3_APPROVE_CMD <code>`)
//
// Env: T3CODE_RELAY_URL, T3CODE_CLERK_PUBLISHABLE_KEY (the stand-in), SSL_CERT_FILE (its CA),
// STANDIN_CONTROL (host:port), STANDIN_EMAIL.
// Usage: relay-check <connect|approval> <out-dir> <data-dir>
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use serde::Deserialize;
use t3code_client_runtime::connection::ConnectionPhase;
use t3code_client_runtime::connection::model::{ConnectionErrorKind, TransientReason};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_client_runtime::relay::{CloudAccountState, CloudConfig};
use t3code_web::app_root::{AppRootOptions, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::connection::shell::EnvironmentShell;
use t3code_web::connection::{ConnectionRuntime, ConnectionsState, RelayAvailability, RuntimeConfig};
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

/// The stand-in's counters, from its control port.
#[derive(Deserialize, Debug, Default)]
struct StandinState {
    relay: RelayCounts,
    tunnel: TunnelCounts,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct RelayCounts {
    mints: u32,
    token_exchanges: u32,
    connects: u32,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct TunnelCounts {
    upgrades: u32,
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

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame()?;
        let image = self.cx.capture_screenshot(self.handle)?;
        image.save(self.out.join(name))?;
        self.note(format!("screenshot {name}"));
        Ok(())
    }

    fn click(&mut self, id: impl Into<SharedString>) -> anyhow::Result<()> {
        let id = id.into();
        self.frame()?;
        let target = id.clone();
        self.cx.update_window(self.handle, |_, w, cx| w.click(target, cx))?;
        self.frame()?;
        self.note(format!("click {id}"));
        Ok(())
    }

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

    fn type_text(&mut self, text: &str, shown: &str) -> anyhow::Result<()> {
        self.cx.update_window(self.handle, |_, w, cx| w.input(text, cx))?;
        self.frame()?;
        self.note(format!("typed {shown}"));
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

    /// Pumps the app for `duration`, logging status changes.
    fn idle(&mut self, duration: Duration) -> anyhow::Result<()> {
        let until = Instant::now() + duration;
        let mut last = self.state(summarize);
        while Instant::now() < until {
            self.frame()?;
            let summary = self.state(summarize);
            if summary != last {
                self.note(format!("state: {summary}"));
                last = summary;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    /// One plain-HTTP request to the stand-in's control port.
    fn control(&mut self, method: &str, path: &str) -> anyhow::Result<String> {
        let mut stream = std::net::TcpStream::connect(&self.control)?;
        write!(stream, "{method} {path} HTTP/1.0\r\nHost: control\r\nContent-Length: 0\r\n\r\n")?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer)?;
        let body = answer.split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default();
        Ok(body)
    }

    fn standin(&mut self) -> anyhow::Result<StandinState> {
        Ok(serde_json::from_str(&self.control("GET", "/state")?)?)
    }

    /// Opens an environment row's routes or diagnosis details.
    fn open_row(&mut self, id: &str, routes: bool, details: bool) -> anyhow::Result<()> {
        let shell = self.shell.clone();
        self.cx.update_window(self.handle, |_, _, cx| {
            let settings = shell.read(cx).settings().cloned().context("settings are open")?;
            let page = settings.read(cx).environments().clone();
            page.update(cx, |page, cx| page.toggle_for_scene(id, routes, details, cx));
            anyhow::Ok(())
        })??;
        self.frame()
    }

    /// Focuses the sign-in dialog's email or code field.
    fn focus_sign_in_field(&mut self, code: bool) -> anyhow::Result<()> {
        let shell = self.shell.clone();
        self.cx.update_window(self.handle, |_, window, cx| {
            let settings = shell.read(cx).settings().cloned().context("settings are open")?;
            let page = settings.read(cx).environments().clone();
            let dialog = page.read(cx).sign_in_dialog().cloned().context("sign-in dialog is open")?;
            let field = if code { dialog.read(cx).code().clone() } else { dialog.read(cx).email().clone() };
            field.update(cx, |state, cx| state.focus(window, cx));
            anyhow::Ok(())
        })??;
        self.frame()
    }

    /// Email-code sign-in from the empty window's "Sign in" (settings closed
    /// first), reading the code the stand-in "emailed".
    fn sign_in(&mut self, email: &str, shot_prefix: &str) -> anyhow::Result<()> {
        self.frame()?;
        if self.cx.update_window(self.handle, |_, w, _| w.try_find("settings-close").is_some())? {
            self.click("settings-close")?;
            self.idle(Duration::from_millis(400))?;
        }
        self.click("empty-sign-in")?;
        self.idle(Duration::from_millis(400))?;
        self.shot(&format!("{shot_prefix}-sign-in-email.png"))?;
        self.focus_sign_in_field(false)?;
        self.type_text(email, email)?;
        #[derive(Deserialize)]
        struct Code {
            code: Option<String>,
        }
        let earlier: Code = serde_json::from_str(&self.control("GET", "/clerk/last-code")?)?;
        self.click("t3-connect-submit")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let code = loop {
            self.frame()?;
            let latest: Code = serde_json::from_str(&self.control("GET", "/clerk/last-code")?)?;
            if latest.code.is_some() && latest.code != earlier.code {
                break latest.code.unwrap_or_default();
            }
            if Instant::now() > deadline {
                bail!("no email code arrived");
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        self.idle(Duration::from_millis(300))?;
        self.shot(&format!("{shot_prefix}-sign-in-code.png"))?;
        self.focus_sign_in_field(true)?;
        self.type_text(&code, "<emailed code>")?;
        self.click("t3-connect-submit")?;
        self.wait("signed in", Duration::from_secs(15), |s| {
            matches!(s.cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedIn(_)))
        })
    }

    fn wait_discovered(&mut self) -> anyhow::Result<String> {
        self.wait("account environment listed and online", Duration::from_secs(20), |s| {
            s.cloud.as_ref().is_some_and(|c| {
                c.discovery.environments.iter().any(|e| e.availability == RelayAvailability::Online)
            })
        })?;
        Ok(self.state(|s| s.cloud.as_ref().unwrap().discovery.environments[0].record.environment_id.to_string()))
    }
}

fn summarize(state: &ConnectionsState) -> String {
    let account = match state.cloud.as_ref().map(|c| &c.account) {
        Some(CloudAccountState::SignedIn(account)) => format!("signed in as {}", account.email.as_deref().unwrap_or("?")),
        Some(CloudAccountState::SignedOut) => "signed out".into(),
        Some(CloudAccountState::Loading) => "account loading".into(),
        None => "no cloud".into(),
    };
    let listed = state.cloud.as_ref().map_or(0, |c| c.discovery.environments.len());
    let environments = state
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
            format!("{} {}", env.entry.label, phase)
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("{account}; {listed} listed; [{environments}]")
}

fn connected(state: &ConnectionsState) -> bool {
    !state.environments.is_empty()
        && state.environments.iter().all(|env| env.status.as_ref().is_some_and(|s| s.phase == ConnectionPhase::Connected))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: relay-check <connect|approval> <out-dir> <data-dir>";
    let phase = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    let data_dir = PathBuf::from(args.next().context(usage)?);
    std::fs::create_dir_all(&out)?;
    let control = std::env::var("STANDIN_CONTROL").context("STANDIN_CONTROL")?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data_dir.join("secrets.json")))),
        data_dir,
        cloud: Some(CloudConfig::from_env()),
        presentation: ClientPresentation { label: "t3code-gpui relay check".into(), ..Default::default() },
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
    let mut check =
        Check { cx, out: out.clone(), started: Instant::now(), log: String::new(), failures: 0, handle, shell, control };
    check.note(format!("phase {phase}"));
    let result = match phase.as_str() {
        "connect" => run_connect(&mut check),
        "approval" => run_approval(&mut check),
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

fn run_connect(check: &mut Check) -> anyhow::Result<()> {
    let email = std::env::var("STANDIN_EMAIL").context("STANDIN_EMAIL")?;
    check.wait("signed out with no stored session", Duration::from_secs(10), |s| {
        matches!(s.cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedOut))
    })?;
    check.shot("c01-signed-out.png")?;
    check.sign_in(&email, "c02")?;
    let environment_id = check.wait_discovered()?;
    check.shot("c03-discovered.png")?;

    // A token the host says lasts 75 s renews at 15 s, a minute before expiry.
    check.control("POST", "/tunnel/expires-in?seconds=75")?;
    let before = check.standin()?;
    check.click(format!("relay-add-{environment_id}"))?;
    check.wait("connected through T3 Connect", Duration::from_secs(30), connected)?;
    let connected_at = Instant::now();
    check.wait("latency measured", Duration::from_secs(40), |s| s.environments.iter().all(|e| e.latency.is_some()))?;
    let after = check.standin()?;
    check.note(format!("stand-in: {after:?}"));
    check.expect(after.relay.mints == before.relay.mints + 1, "one bootstrap credential minted");
    check.expect(after.tunnel.upgrades == before.tunnel.upgrades + 1, "one socket upgrade through the tunnel");
    let route = check.state(|s| s.environments[0].status.as_ref().and_then(|st| st.active_route_id.clone()));
    check.expect(route.as_deref() == Some("relay"), "active route is the relay");
    check.open_row(&environment_id, true, false)?;
    check.shot("c04-connected-relay.png")?;

    // Renewal: a second mint around 15 s after the first, on the same socket.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        check.idle(Duration::from_millis(500))?;
        let now = check.standin()?;
        if now.relay.mints >= after.relay.mints + 1 {
            let at = connected_at.elapsed().as_secs_f64();
            check.note(format!("renewed {at:.1}s after connecting: {now:?}"));
            check.expect((8.0..=25.0).contains(&at), "renewal ran about a minute before the 75 s expiry");
            check.expect(now.tunnel.upgrades == after.tunnel.upgrades, "renewal kept the socket (no new upgrade)");
            break;
        }
        if Instant::now() > deadline {
            bail!("no renewal within 40 s");
        }
    }
    check.control("POST", "/tunnel/expires-in")?;
    check.idle(Duration::from_secs(2))?;
    let still = check.state(connected);
    check.expect(still, "still connected after renewal");

    // A dropped tunnel socket reconnects with the cached token: no new mint.
    let before_drop = check.standin()?;
    check.control("POST", "/tunnel/drop")?;
    check.wait("reconnected after the tunnel dropped the socket", Duration::from_secs(20), |s| {
        connected(s) && s.environments[0].status.as_ref().is_some_and(|st| st.generation > 1)
    })?;
    let after_drop = check.standin()?;
    check.note(format!("stand-in after drop: {after_drop:?}"));
    check.expect(after_drop.tunnel.upgrades > before_drop.tunnel.upgrades, "a new upgrade after the drop");

    // Clerk rejects the session: the next token request signs out, the relay
    // route is removed with the account, and email-code sign-in is offered.
    check.control("POST", "/clerk/reject-client")?;
    check.cx.update(|cx| ConnectionRuntime::global(cx).refresh_relay(cx));
    check.wait("signed out after Clerk rejected the session", Duration::from_secs(15), |s| {
        matches!(s.cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedOut))
    })?;
    check.wait("the account's relay route went with it", Duration::from_secs(10), |s| s.environments.is_empty())?;
    check.shot("c05-session-rejected.png")?;
    check.control("POST", "/clerk/accept-client")?;
    check.sign_in(&email, "c06")?;
    let environment_id = check.wait_discovered()?;
    check.click(format!("relay-add-{environment_id}"))?;
    check.wait("connected again after signing in again", Duration::from_secs(30), connected)?;
    check.shot("c07-connected-again.png")?;

    // Sign-out removes the relay route and forgets the account.
    let task = check.cx.update(|cx| ConnectionRuntime::global(cx).sign_out(cx));
    check.cx.run_until_parked();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check.state(|s| matches!(s.cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedOut))) {
        check.frame()?;
        if Instant::now() > deadline {
            bail!("sign-out did not finish");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(task);
    check.wait("signed out, no relay environments", Duration::from_secs(10), |s| {
        s.environments.is_empty() && s.cloud.as_ref().is_some_and(|c| c.discovery.environments.is_empty())
    })?;
    check.shot("c08-signed-out.png")?;
    Ok(())
}

fn run_approval(check: &mut Check) -> anyhow::Result<()> {
    let email = std::env::var("STANDIN_EMAIL").context("STANDIN_EMAIL")?;
    let approve = std::env::var("T3_APPROVE_CMD").context("T3_APPROVE_CMD")?;
    check.wait("signed out with no stored session", Duration::from_secs(10), |s| {
        matches!(s.cloud.as_ref().map(|c| &c.account), Some(CloudAccountState::SignedOut))
    })?;
    check.sign_in(&email, "a01")?;
    let environment_id = check.wait_discovered()?;
    let code = check.state(|s| s.verification_code.to_string());
    check.click(format!("relay-add-{environment_id}"))?;
    check.wait("approval pending", Duration::from_secs(30), |s| {
        s.environments.first().and_then(|e| e.status.as_ref()).and_then(|st| st.last_failure.as_ref()).is_some_and(|f| {
            f.kind == ConnectionErrorKind::Transient(TransientReason::ApprovalPending)
        })
    })?;
    let first = check.standin()?;
    check.shot("a02-approval-pending.png")?;
    check.open_row(&environment_id, false, true)?;
    check.shot("a03-approval-pending-details.png")?;
    let shown = check.state(|s| {
        s.environments[0].status.as_ref().and_then(|st| st.last_failure.as_ref()).map(|f| f.detail.clone()).unwrap_or_default()
    });
    check.expect(shown.contains(&code), format!("the pending state names this device's code {code}: {shown}"));
    // Retries every 4 s: count relay connects over 13 s.
    check.idle(Duration::from_secs(13))?;
    let later = check.standin()?;
    let retries = later.relay.connects - first.relay.connects;
    check.note(format!("{retries} mint retries in 13 s"));
    check.expect((2..=4).contains(&retries), "the whole mint retries about every 4 s");
    let output = std::process::Command::new("sh").arg("-c").arg(format!("{approve} {code}")).output()?;
    check.note(format!("approve: {}", String::from_utf8_lossy(&output.stdout).trim()));
    check.wait("connected after approval", Duration::from_secs(20), connected)?;
    check.shot("a04-approved-connected.png")?;
    Ok(())
}
