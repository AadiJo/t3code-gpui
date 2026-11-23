//! End-to-end check of the streamed browser preview against a running T3 Code
//! server: pairs, opens the preview panel in a headless window, loads a page,
//! takes control, clicks a link, types into a field, answers an alert, and
//! records frame decode time, input-to-frame latency and memory.
//!
//! `preview-check <out-dir> <data-dir>` with `T3_HOST` (http origin),
//! `T3_CODE` (pairing credential) and `T3_PAGE` (a URL the server's browser reaches).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, MouseButton, WindowBounds, WindowOptions, component, px, size};
use t3code_client_runtime::connection::onboarding::PairingRequest;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_client_runtime::preview::server_browser_stream::Controller;
use t3code_contracts::ThreadId;
use t3code_web::browser::server_browser_surface::ServerBrowserSurface;
use t3code_web::components::preview::preview_source::LivePreviewSource;
use t3code_web::components::preview::preview_view::PreviewView;
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
    handle: AnyWindowHandle,
    rss_samples: Vec<u64>,
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
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

    /// Pumps the app until `ready` holds.
    fn wait(&mut self, what: &str, timeout: Duration, mut ready: impl FnMut(&mut Self) -> bool) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.frame()?;
            if ready(self) {
                self.note(format!("ok: {what}"));
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {what}");
            }
            self.rss_samples.push(rss_kib());
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: preview-check <out-dir> <data-dir>";
    let out = PathBuf::from(args.next().context(usage)?);
    let data_dir = PathBuf::from(args.next().context(usage)?);
    let host = std::env::var("T3_HOST").context("T3_HOST")?;
    let code = std::env::var("T3_CODE").context("T3_CODE")?;
    let page = std::env::var("T3_PAGE").context("T3_PAGE")?;
    std::fs::create_dir_all(&out)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data_dir.join("secrets.json")))),
        data_dir,
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui preview check".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
        ConnectionRuntime::init(config, cx)
    })?;
    let handle = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(900.), px(700.)), cx)),
            show: false,
            focus: true,
            ..component::TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| gpui_kit::Empty))
    })?;
    let mut check = Check { cx, out: out.clone(), started: Instant::now(), log: String::new(), handle: handle.0, rss_samples: Vec::new() };
    let result = run(&mut check, &host, &code, &page);
    if let Err(error) = &result {
        check.note(format!("FAILED: {error:#}"));
        check.shot("preview-failure.png").ok();
    }
    check.cx.update(ConnectionRuntime::shutdown);
    std::fs::write(out.join("preview-transcript.txt"), &check.log)?;
    result
}

fn run(check: &mut Check, host: &str, code: &str, page: &str) -> anyhow::Result<()> {
    let base: url::Url = host.parse()?;
    let mut ws = base.clone();
    ws.set_scheme(if base.scheme() == "https" { "wss" } else { "ws" }).ok();
    let pair = check.cx.update(|cx| {
        ConnectionRuntime::global(cx).pair(
            PairingRequest { http_base_url: base.clone(), ws_base_url: ws, credential: code.to_owned(), label: Some("e2e".into()) },
            None,
            cx,
        )
    });
    let started = Instant::now();
    while !check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.iter().any(|e| e.is_connected())) {
        check.cx.run_until_parked();
        if started.elapsed() > Duration::from_secs(30) {
            bail!("pairing did not connect");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(pair);
    check.note("paired and connected");
    let source = check.cx.update(|cx| {
        let runtime = ConnectionRuntime::global(cx);
        let id = ConnectionRuntime::state(cx).read(cx).environments[0].id().clone();
        LivePreviewSource::new(runtime.environment_link(&id).expect("link"), runtime.handle().clone())
    });
    let view: Entity<PreviewView> = check.cx.update_window(check.handle, |_, window, cx| {
        let view = cx.new(|cx| PreviewView::new(source as Rc<_>, ThreadId::new(format!("e2e-thread-{}", std::process::id())), "e2e".into(), None, window, cx));
        window.replace_root(cx, |_, _| ViewHost(view.clone()));
        view
    })?;
    check.frame()?;
    let page_url = page.to_owned();
    check.cx.update_window(check.handle, |_, window, cx| view.update(cx, |view, cx| view.navigate(page_url, window, cx)))?;
    let surface = |check: &mut Check| check.cx.update(|cx| view.read(cx).active_surface());
    check.wait("a tab opened", Duration::from_secs(30), |c| surface(c).is_some())?;
    let surface: Entity<ServerBrowserSurface> = surface(check).context("surface")?;
    let metrics = check.cx.update(|cx| surface.read(cx).metrics());
    check.wait("first frame", Duration::from_secs(60), |_| metrics.lock().unwrap().frames_painted > 0)?;
    check.wait("page one loaded", Duration::from_secs(30), |c| c.cx.update(|cx| view.read(cx).active_title()).contains("page one"))?;
    std::thread::sleep(Duration::from_millis(300));
    let controller = check.cx.update(|cx| surface.read(cx).control().map(|c| c.controller));
    check.note(format!("controller after connect: {controller:?}"));
    check.shot("preview-01-loaded.png")?;
    if controller != Some(Controller::You) {
        check.cx.update_window(check.handle, |_, w, cx| w.click("preview-control", cx))?;
        check.wait("took control", Duration::from_secs(10), |c| {
            c.cx.update(|cx| surface.read(cx).control().is_some_and(|ctl| ctl.controller == Controller::You))
        })?;
    }
    let point = |check: &mut Check, x: f64, y: f64| check.cx.update(|cx| surface.read(cx).window_point(x, y)).context("no frame yet");

    // Click the link at its centre (page CSS px 170, 64).
    let painted = metrics.lock().unwrap().frames_painted;
    let at = point(check, 170., 64.)?;
    check.note(format!("click link at window {at:?}"));
    check.cx.update_window(check.handle, |_, w, cx| {
        w.pointer_move(at, None, cx);
        w.pointer_down(at, MouseButton::Left, cx);
        w.pointer_up(at, MouseButton::Left, cx);
    })?;
    check.wait("page two painted", Duration::from_secs(15), |c| {
        let title = c.cx.update(|cx| view.read(cx).active_title());
        metrics.lock().unwrap().frames_painted > painted && title.contains("page two")
    })?;
    std::thread::sleep(Duration::from_millis(300));
    check.shot("preview-02-link-clicked.png")?;

    // Focus the field (page px 200, 120) and type through key events.
    let at = point(check, 200., 120.)?;
    check.cx.update_window(check.handle, |_, w, cx| {
        w.pointer_move(at, None, cx);
        w.pointer_down(at, MouseButton::Left, cx);
        w.pointer_up(at, MouseButton::Left, cx);
    })?;
    check.frame()?;
    for key in ["h", "e", "l", "l", "o", "space", "g", "p", "u", "i"] {
        check.cx.update_window(check.handle, |_, w, cx| w.press(key, cx))?;
        check.frame()?;
        std::thread::sleep(Duration::from_millis(40));
    }
    let painted = metrics.lock().unwrap().frames_painted;
    check.wait("typed text painted", Duration::from_secs(10), |_| metrics.lock().unwrap().frames_painted > painted)?;
    std::thread::sleep(Duration::from_millis(500));
    check.frame()?;
    check.shot("preview-03-typed.png")?;

    // Open the page's alert and dismiss it from the dialog overlay.
    let at = point(check, 140., 262.)?;
    check.cx.update_window(check.handle, |_, w, cx| {
        w.pointer_move(at, None, cx);
        w.pointer_down(at, MouseButton::Left, cx);
        w.pointer_up(at, MouseButton::Left, cx);
    })?;
    check.wait("page alert shown", Duration::from_secs(10), |c| {
        c.cx.update(|cx| surface.read(cx).control().is_some_and(|ctl| ctl.dialog.is_some()))
    })?;
    check.shot("preview-04-alert.png")?;
    check.cx.update_window(check.handle, |_, w, cx| w.click("dialog-dismiss", cx))?;
    check.wait("alert dismissed", Duration::from_secs(10), |c| {
        c.cx.update(|cx| surface.read(cx).control().is_some_and(|ctl| ctl.dialog.is_none()))
    })?;

    // Port forwarding: the environment runs on this machine, so the tunnel's
    // probe must reach our own listener and refuse the forward.
    let (sender, receiver) = std::sync::mpsc::channel();
    check.cx.update(|cx| {
        let runtime = ConnectionRuntime::global(cx);
        let id = ConnectionRuntime::state(cx).read(cx).environments[0].id().clone();
        let link = runtime.environment_link(&id).expect("link");
        let tunnel = std::sync::Arc::new(t3code_client_runtime::preview::loopback_tunnel::LoopbackTunnel::default());
        runtime.handle().spawn(async move {
            let started = Instant::now();
            let result = tunnel.forward(link, 37812).await.map_err(|e| e.to_string());
            _ = sender.send((result, started.elapsed()));
        });
    });
    let outcome = receiver.recv_timeout(Duration::from_secs(20)).ok();
    check.note(format!("port forward of 37812 on a same-machine environment: {outcome:?}"));

    // Stream a while for steady-state numbers.
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        check.frame()?;
        check.rss_samples.push(rss_kib());
        std::thread::sleep(Duration::from_millis(100));
    }
    let m = metrics.lock().unwrap().clone();
    let stats = |values: &[f64]| {
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        let pick = |q: f64| v.get(((v.len() as f64 - 1.) * q).round() as usize).copied().unwrap_or(0.);
        format!("n={} p50={:.2} p90={:.2} max={:.2}", v.len(), pick(0.5), pick(0.9), pick(1.0))
    };
    let rss = &check.rss_samples;
    let summary = format!(
        "frames painted {} skipped {} last size {:?}\ndecode ms: {}\ninput->frame ms: {}\nRSS KiB while streaming: min {} max {}",
        m.frames_painted,
        m.frames_skipped,
        m.last_frame_size,
        stats(&m.decode_ms),
        stats(&m.input_to_frame_ms),
        rss.iter().min().unwrap_or(&0),
        rss.iter().max().unwrap_or(&0),
    );
    check.note(&summary);
    std::fs::write(check.out.join("preview-metrics.txt"), summary)?;
    Ok(())
}

struct ViewHost(Entity<PreviewView>);

impl gpui_kit::Render for ViewHost {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
        self.0.clone()
    }
}
