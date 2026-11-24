// Drives each transition in the assembled app and proves it ends: panels
// (sidebar, right panel) at the owner's 125ms, a toast's enter and exit,
// then counts frame requests while idle. Then, in a probe window, closes a
// dialog and a popover built from the shared primitives and proves each keeps
// painting, fading, through its exit transition and then stops. Writes
// mid-transition and settled screenshots plus `motion.log`.
// Usage: motion-check <out-dir>
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::ensure;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, DataSource, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::ui::dialog::Modal;
use t3code_web::components::ui::popover::{PopoverSurface, below};
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
    window_width: Pixels,
}

/// A window with a dialog and a popover from the shared primitives, each shown while its
/// flag is set.
struct ExitProbe {
    dialog: bool,
    popover: bool,
}

impl Render for ExitProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(black())
            .child(
                div()
                    .absolute()
                    .top(px(40.))
                    .left(px(40.))
                    .w(px(200.))
                    .child("Anchor")
                    .when(self.popover, |d| {
                        d.child(below(PopoverSurface::new().title("Popover").child("Exit probe content")))
                    }),
            )
            .when(self.dialog, |d| {
                d.child(Modal::dialog(
                    "probe-dialog",
                    div().w(px(400.)).h(px(200.)).bg(white()).child("Dialog"),
                    |_, _| {},
                ))
            })
    }
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

    /// The mean brightness of `region` (logical px) in a fresh screenshot.
    fn brightness(&mut self, region: Bounds<Pixels>) -> anyhow::Result<f32> {
        let image = self.cx.capture_screenshot(self.window)?;
        let scale = image.width() as f32 / f32::from(self.window_width);
        let (x0, y0) = ((f32::from(region.left()) * scale) as u32, (f32::from(region.top()) * scale) as u32);
        let (x1, y1) = ((f32::from(region.right()) * scale) as u32, (f32::from(region.bottom()) * scale) as u32);
        let (mut sum, mut n) = (0f32, 0f32);
        for y in y0..y1.min(image.height()) {
            for x in x0..x1.min(image.width()) {
                let p = image.get_pixel(x, y).0;
                sum += (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.;
                n += 1.;
            }
        }
        Ok(sum / n.max(1.))
    }

    /// Closes a probe element with `close` and checks it keeps painting for its
    /// exit transition (`span`): halfway it sits between open and closed, then
    /// the window draws it gone and goes quiet.
    fn exit(&mut self, name: &str, span: Duration, region: Bounds<Pixels>, close: impl FnOnce(&mut Self)) -> anyhow::Result<()> {
        let open = self.brightness(region)?;
        close(self);
        let frame = Duration::from_millis(8);
        let (mut elapsed, mut requested, mut mid) = (Duration::ZERO, 0, None);
        while elapsed < span + Duration::from_millis(120) {
            requested += usize::from(self.pump() > 0);
            if mid.is_none() && elapsed >= span / 2 {
                mid = Some(self.brightness(region)?);
                self.shot(&format!("{name}-mid"))?;
            }
            self.advance(frame);
            elapsed += frame;
        }
        let closed = self.brightness(region)?;
        let mid = mid.unwrap_or(closed);
        let between = (mid - open) * (mid - closed) < 0.;
        self.check(&format!("{name} fades out"), between, format!("brightness open {open:.1}, halfway {mid:.1}, closed {closed:.1}"));
        let expected = (span.as_millis() / frame.as_millis()) as usize;
        self.check(
            &format!("{name} asks for frames only while leaving"),
            requested > 0 && requested <= expected + 2,
            format!("{requested} frames asked for another over {}ms", span.as_millis()),
        );
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

    // Exit transitions: the fork's `data-ending-style` on dialogs (200ms) and popovers (150ms).
    let probe = check.cx.open_window(size(px(1100.), px(780.)), |_, cx| cx.new(|_| ExitProbe { dialog: true, popover: false }))?;
    check.window = probe.into();
    check.window_width = px(1100.);
    for _ in 0..40 {
        check.advance(Duration::from_millis(10));
        check.pump();
    }
    let dialog_box = Bounds::new(point(px(500.), px(340.)), size(px(100.), px(100.)));
    check.exit("dialog-exit", Duration::from_millis(200), dialog_box, |c| {
        c.cx.update(|cx| probe.update(cx, |view, _, cx| {
            view.dialog = false;
            cx.notify();
        })).ok();
    })?;
    check.cx.update(|cx| probe.update(cx, |view, _, cx| {
        view.popover = true;
        cx.notify();
    })).ok();
    for _ in 0..30 {
        check.advance(Duration::from_millis(10));
        check.pump();
    }
    let popover_box = Bounds::new(point(px(40.), px(64.)), size(px(120.), px(40.)));
    check.exit("popover-exit", Duration::from_millis(150), popover_box, |c| {
        c.cx.update(|cx| probe.update(cx, |view, _, cx| {
            view.popover = false;
            cx.notify();
        })).ok();
    })?;
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
        data: DataSource::Fixtures(FixtureStore::audit_reference()),
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
    Ok(Check { cx, window, out: out.to_path_buf(), log: String::new(), failures: 0, window_width: px(1100.) })
}
