// Drives each transition in the assembled app and proves it ends: panels
// (sidebar, right panel) at the owner's 125ms, a toast's enter and exit,
// then counts frame requests while idle. Writes mid-transition and settled
// screenshots plus `motion.log`.
// Usage: motion-check <out-dir>
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::ensure;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, DataSource, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::ui::toast::{ToastData, ToastKind, Toaster};
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::fixtures::FixtureStore;
use t3code_web::shortcuts::commands::{ToggleRightPanel, ToggleSidebar};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};
use t3code_web::ui_state_store::StateDir;

struct Check {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    out: PathBuf,
    log: String,
    failures: usize,
}

impl Check {
    /// Draws one frame and delivers the next-frame callbacks it asked for.
    /// Returns how many callbacks ran: 0 means the window asked for nothing.
    fn pump(&mut self) -> usize {
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
        self.cx.run_until_parked();
        self.cx.update_window(self.window, |_, w, cx| w.simulate_next_frame(cx)).unwrap_or(0)
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.cx.capture_screenshot(self.window)?.save(self.out.join(format!("{name}.png")))?;
        writeln!(self.log, "  screenshot {name}.png")?;
        Ok(())
    }

    fn check(&mut self, what: &str, ok: bool, detail: impl std::fmt::Display) {
        let line = format!("{} {what}: {detail}", if ok { "PASS" } else { "FAIL" });
        eprintln!("{line}");
        writeln!(self.log, "{line}").ok();
        self.failures += usize::from(!ok);
    }

    /// Moves the app's clock on: the headless executor's clock is simulated.
    fn advance(&mut self, by: Duration) {
        self.cx.advance_clock(by);
    }

    /// Frames with a pending request across `frames` idle pumps 10ms apart.
    fn idle_requests(&mut self, frames: usize) -> usize {
        (0..frames)
            .map(|_| {
                self.advance(Duration::from_millis(10));
                self.pump()
            })
            .filter(|n| *n > 0)
            .count()
    }

    /// Starts a transition with `trigger`, pumps frames for `span`, saves
    /// one frame from the middle, then checks the window goes quiet.
    fn transition(&mut self, name: &str, span: Duration, trigger: impl FnOnce(&mut Self)) -> anyhow::Result<()> {
        trigger(self);
        let frame = Duration::from_millis(8);
        let (mut elapsed, mut frames, mut requested, mut mid_saved) = (Duration::ZERO, 0, 0, false);
        while elapsed < span + Duration::from_millis(120) {
            requested += usize::from(self.pump() > 0);
            frames += 1;
            if !mid_saved && elapsed >= span / 2 {
                self.shot(&format!("{name}-mid"))?;
                mid_saved = true;
            }
            self.advance(frame);
            elapsed += frame;
        }
        self.check(&format!("{name} animates"), requested > 0, format!("{requested} of {frames} frames asked for another"));
        let idle = self.idle_requests(30);
        self.check(&format!("{name} then idle"), idle == 0, format!("{idle} of 30 idle frames asked for another"));
        self.shot(&format!("{name}-end"))
    }

    fn action(&mut self, action: Box<dyn Action>) {
        self.cx.update_window(self.window, |_, w, cx| w.dispatch_action(action, cx)).ok();
    }
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "motion-check".into()));
    std::fs::create_dir_all(&out)?;
    let data = out.join("data");
    std::fs::create_dir_all(&data)?;
    // SAFETY: set before any other thread reads the environment.
    unsafe {
        std::env::set_var("T3CODE_MOTION", "1");
        std::env::set_var("T3CODE_PANEL_MS", std::env::var("T3CODE_PANEL_MS").unwrap_or_else(|_| "125".into()));
    }
    let mut check = open(&out, &data)?;
    check.pump();
    let idle = check.idle_requests(30);
    check.check("idle before any transition", idle == 0, format!("{idle} of 30 frames asked for another"));
    let panel = Duration::from_millis(125);
    check.transition("sidebar-close", panel, |c| c.action(Box::new(ToggleSidebar)))?;
    check.transition("sidebar-open", panel, |c| c.action(Box::new(ToggleSidebar)))?;
    check.transition("right-panel-open", panel, |c| c.action(Box::new(ToggleRightPanel)))?;
    check.transition("right-panel-close", panel, |c| c.action(Box::new(ToggleRightPanel)))?;
    let toast = Duration::from_millis(500);
    let mut id = 0;
    check.transition("toast-enter", toast, |c| {
        id = c.cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Info, "Pull request opened"), cx));
    })?;
    check.transition("toast-exit", toast, |c| c.cx.update(|cx| Toaster::dismiss(id, cx)))?;
    writeln!(check.log, "{} failures", check.failures)?;
    std::fs::write(out.join("motion.log"), &check.log)?;
    ensure!(check.failures == 0, "{} checks failed; see motion.log", check.failures);
    Ok(())
}

fn open(out: &Path, data: &Path) -> anyhow::Result<Check> {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
        data_dir: data.to_path_buf(),
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui motion check".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
        ConnectionRuntime::init(config, cx)
    })?;
    let options = AppRootOptions {
        data: DataSource::Fixtures(FixtureStore::reference()),
        platform: TitlebarPlatform::Mac,
        state_dir: StateDir(None),
    };
    let (window, _parts) = cx.update(|cx| {
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(1100.), px(780.)), cx)),
            show: false,
            focus: true,
            ..component::TitleBar::window_options()
        };
        WindowRoot::open_window(window_options, options, cx)
    })?;
    cx.update(ThemeSettings::apply_env_overrides);
    Ok(Check { cx, window, out: out.to_path_buf(), log: String::new(), failures: 0 })
}
