//! Clicks through the source control controls headlessly: the git actions
//! menu and commit dialog, the branch picker with search and arrow keys, the
//! pull request checkout dialog, the run-context and editor menus, the items
//! menu, the scripts menu and editor, the composer strip popups, the
//! default-branch confirmation and a failed action. Saves a screenshot after
//! each step and a transcript of the RPCs the fixture received.
//!
//!   cargo run -p t3code-web --features test-support --example git_actions_check -- <out dir> [dark|light]

use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, IntoElement, Render, WindowBounds, WindowOptions, px,
    size, test::TestWindowExt as _,
};
use t3code_web::components::git_actions_control_fixture::{FixtureGitSource, fixture_open_pr, fixture_status};
use t3code_web::components::chat::thread_details_items::{ThreadDetailsItem, ThreadDetailsItemSettings};
use t3code_web::scenes::git_actions::GitPanelScene;
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

struct Host(Entity<GitPanelScene>);

impl Render for Host {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

struct Check {
    cx: HeadlessAppContext,
    out: std::path::PathBuf,
    suffix: String,
    log: String,
}

impl Check {
    fn open(&mut self, status: t3code_contracts::VcsStatusResult, fail: bool) -> anyhow::Result<(AnyWindowHandle, Entity<GitPanelScene>)> {
        let (handle, scene) = self.cx.update(|cx| -> anyhow::Result<_> {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(1100.), px(780.)), cx)),
                show: false,
                focus: false,
                ..Default::default()
            };
            let mut scene = None;
            let handle = gpui_kit::open_window(options, cx, |window, cx| {
                let mut source = FixtureGitSource::new(status);
                if fail {
                    source.fail_with = Some("pre-commit hook failed: lint found 3 problems".into());
                }
                let entity = cx.new(|cx| GitPanelScene::new(source, window, cx));
                scene = Some(entity.clone());
                cx.new(|_| Host(entity))
            })?;
            Ok((handle.0, scene.unwrap()))
        })?;
        self.cx.run_until_parked();
        Ok((handle, scene))
    }

    /// Runs `act` in the window, settles, and saves `name`.
    fn step(&mut self, handle: AnyWindowHandle, name: &str, act: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App)) -> anyhow::Result<()> {
        self.cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            act(window, cx);
        })?;
        self.cx.run_until_parked();
        self.cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
        let path = self.out.join(format!("{name}{}.png", self.suffix));
        self.cx.capture_screenshot(handle)?.save(&path)?;
        writeln!(self.log, "step {name}: saved {}", path.display())?;
        Ok(())
    }

    fn record(&mut self, scene: &Entity<GitPanelScene>, label: &str) {
        let (calls, events) = self.cx.update(|cx| {
            let s = scene.read(cx);
            (s.calls(), s.events.clone())
        });
        let _ = writeln!(self.log, "  after {label}: rpc calls {calls:?}");
        let _ = writeln!(self.log, "  after {label}: events {events:?}");
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().expect("output directory"));
    let light = args.next().as_deref() == Some("light");
    std::fs::create_dir_all(&out)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(if light { ThemeMode::Light } else { ThemeMode::Dark }, cx);
    });
    let mut check = Check { cx, out, suffix: if light { "-light".into() } else { String::new() }, log: String::new() };

    // A dirty feature branch with an open pull request.
    let mut status = fixture_status(Some("feature/thread-details"), true, true, 0, 0);
    status.pr = Some(fixture_open_pr());
    let (w, scene) = check.open(status, false)?;
    check.step(w, "01-panel", |_, _| {})?;
    check.step(w, "02-git-menu", |win, cx| win.click("git-menu-trigger", cx))?;
    check.step(w, "03-commit-dialog", |win, cx| win.click(("git-menu-item", 0usize), cx))?;
    check.step(w, "04-commit-edit-files", |win, cx| win.click("commit-edit-files", cx))?;
    check.step(w, "05-commit-file-excluded", |win, cx| win.click(("include-file", 1usize), cx))?;
    check.step(w, "06-commit-success", |win, cx| win.click("commit-confirm", cx))?;
    check.record(&scene, "commit");
    check.step(w, "07-branch-picker", |win, cx| win.click("branch-select", cx))?;
    check.step(w, "08-branch-search", |win, cx| win.input("feat", cx))?;
    check.step(w, "09-branch-keyboard", |win, cx| {
        win.press("down", cx);
        win.press("down", cx);
    })?;
    check.step(w, "10-branch-switched", |win, cx| win.press("enter", cx))?;
    check.record(&scene, "branch switch");
    check.step(w, "11-branch-create", |win, cx| {
        win.click("branch-select", cx);
        win.input("fix/new picker", cx);
    })?;
    check.step(w, "12a-picker-closed-by-escape", |win, cx| win.press("escape", cx))?;
    check.step(w, "12-branch-checkout-pr", |win, cx| {
        win.click("branch-select", cx);
        win.input("#418", cx);
    })?;
    check.step(w, "13-pr-dialog", |win, cx| win.press("enter", cx))?;
    check.step(w, "14-pr-dialog-prepared", |win, cx| win.click("pr-worktree", cx))?;
    check.record(&scene, "pull request checkout");
    check.step(w, "15-run-context-menu", |win, cx| win.click("run-context", cx))?;
    check.step(w, "15a-run-context-closed", |win, cx| win.click("run-context", cx))?;
    check.step(w, "16-open-in-menu", |win, cx| {
        win.click("open-in-menu-trigger", cx);
    })?;
    check.step(w, "16a-open-in-closed", |win, cx| win.click("open-in-menu-trigger", cx))?;
    check.step(w, "17-items-menu", |win, cx| {
        win.right_click("thread-details-card", cx);
    })?;
    // The menu's items are drawn by the shared menu primitive, which scripted
    // checks cannot address; choose "Project scripts" through the same setting it writes.
    let panel = check.cx.update(|cx| scene.read(cx).panel.clone());
    check.step(w, "18-scripts-shown", |_, cx| {
        let mut settings = ThreadDetailsItemSettings::get(cx);
        settings.set(ThreadDetailsItem::Scripts, true);
        cx.set_global(settings);
        panel.update(cx, |panel, cx| panel.close_items_menu(cx));
    })?;
    check.step(w, "19-scripts-menu", |win, cx| win.click("script-menu", cx))?;
    check.step(w, "19a-script-row-hovered", |win, cx| win.hover(("script-item", 1usize), cx))?;
    check.step(w, "20-script-editor", |win, cx| win.click(("script-edit", 1usize), cx))?;
    check.step(w, "20a-script-editor-cancelled", |win, cx| win.click("script-cancel", cx))?;
    check.step(w, "21-composer-workspace-menu", |win, cx| {
        win.click("strip-workspace", cx);
    })?;
    check.step(w, "21a-workspace-menu-closed", |win, cx| win.click("strip-workspace", cx))?;
    check.step(w, "22-composer-branch-picker", |win, cx| {
        win.click("strip-branch", cx);
    })?;
    check.record(&scene, "menus");

    // Ahead on the default branch: the push asks first.
    let (w, scene) = check.open(fixture_status(Some("main"), false, true, 2, 0), false)?;
    check.step(w, "23-default-branch-dialog", |win, cx| win.click("git-primary", cx))?;
    check.step(w, "24-default-branch-continue", |win, cx| win.click("default-continue", cx))?;
    check.record(&scene, "default branch push");

    // Behind: the primary action pulls.
    let (w, scene) = check.open(fixture_status(Some("feature/thread-details"), false, true, 0, 3), false)?;
    check.step(w, "25-pull-success", |win, cx| win.click("git-primary", cx))?;
    check.record(&scene, "pull");

    // A failing action raises an error toast.
    let (w, scene) = check.open(fixture_status(Some("feature/thread-details"), true, true, 0, 0), true)?;
    check.step(w, "26-action-error-toast", |win, cx| win.click("git-primary", cx))?;
    check.record(&scene, "failed action");

    let transcript = check.out.join(format!("transcript{}.txt", check.suffix));
    std::fs::write(&transcript, &check.log)?;
    print!("{}", check.log);
    Ok(())
}
