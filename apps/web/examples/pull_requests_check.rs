//! Clicks through the pull request and issue surfaces headlessly: typing the
//! query syntax, applying a filter from the menu, saving a view, opening a
//! pull request from the keyboard, merging with its confirmation, reacting,
//! replying, marking a file viewed, stepping the guide, and commenting and
//! labelling an issue. Saves a screenshot per step, a transcript with every
//! RPC the fixture received, and two measurements: filtering 2,000 rows per
//! keystroke and opening the Code tab on 300 files.
//!
//!   cargo run -p t3code-web --features test-support --example pull_requests_check -- <out dir> [dark|light]

use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, IntoElement, Render, WindowBounds, WindowOptions, px, size, test::TestWindowExt as _};
use t3code_contracts::EnvironmentId;
use t3code_web::components::pull_request::pull_request_detail_panel::{DetailTab, PullRequestDetailPanel};
use t3code_web::components::pull_request::pull_request_fixture::{FixtureSource, fixtures, large_list, large_patch};
use t3code_web::components::pull_request::pull_request_source::{PullRequestDetailSource, pull_request_ref};
use t3code_web::components::pull_request::pull_requests_page::{EnvironmentListSource, PullRequestsPage};
use t3code_web::scenes::pull_requests::{IssuesWorkspace, PullRequestsWorkspace, SCENE_NOW_MS};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

struct Host(gpui_kit::AnyView);

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
    fn open<V: Render>(&mut self, width: f32, height: f32, build: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> Entity<V>) -> anyhow::Result<(AnyWindowHandle, Entity<V>)> {
        let (handle, view) = self.cx.update(|cx| -> anyhow::Result<_> {
            let options = WindowOptions { window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)), show: false, focus: false, ..Default::default() };
            let mut view = None;
            let handle = gpui_kit::open_window(options, cx, |window, cx| {
                let entity = build(window, cx);
                view = Some(entity.clone());
                cx.new(|_| Host(entity.into()))
            })?;
            Ok((handle.0, view.unwrap()))
        })?;
        self.cx.run_until_parked();
        Ok((handle, view))
    }

    fn step(&mut self, handle: AnyWindowHandle, name: &str, act: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App)) -> anyhow::Result<()> {
        self.cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            act(window, cx);
        })?;
        self.cx.run_until_parked();
        self.cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
        self.cx.run_until_parked();
        self.cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
        let path = self.out.join(format!("{name}{}.png", self.suffix));
        self.cx.capture_screenshot(handle)?.save(&path)?;
        writeln!(self.log, "step {name}: {}", path.display())?;
        Ok(())
    }

    fn note(&mut self, line: impl AsRef<str>) {
        let _ = writeln!(self.log, "  {}", line.as_ref());
    }

    fn calls(&mut self, fixture: &FixtureSource, since: usize) -> usize {
        let calls = fixture.calls();
        for call in &calls[since..] {
            let input: String = call.input.chars().take(220).collect();
            let _ = writeln!(self.log, "  rpc {} {input}", call.method);
        }
        calls.len()
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
    let mut check = Check { cx, out: out.clone(), suffix: if light { "-light".into() } else { String::new() }, log: String::new() };

    // --- The page and the detail -------------------------------------------
    let (w, ws) = check.open(1500., 900., |window, cx| cx.new(|cx| PullRequestsWorkspace::new(None, window, cx)))?;
    let (page, fixture) = check.cx.update(|cx| (ws.read(cx).page.clone(), ws.read(cx).fixture.clone()));
    let mut seen = 0;
    check.step(w, "01-list", |_, _| {})?;
    seen = check.calls(&fixture, seen);
    let visible = |check: &mut Check| check.cx.update(|cx| page.read(cx).visible_keys());
    { let line = format!("rows: {}", visible(&mut check).len()); check.note(line); }

    let p = page.clone();
    check.step(w, "02-query-label", |win, cx| {
        p.update(cx, |p, cx| p.focus_search(win, cx));
        win.input("label:performance", cx);
    })?;
    { let line = format!("after typing label:performance: {:?}", visible(&mut check)); check.note(line); }
    let p = page.clone();
    check.step(w, "03-query-cleared", |win, cx| p.update(cx, |p, cx| p.set_query("", win, cx)))?;
    check.step(w, "04-filters-menu", |win, cx| win.click("pr-filters-trigger", cx))?;
    check.step(w, "05a-filter-review-submenu", |win, cx| win.click("pr-filter-sub-review", cx))?;
    check.step(w, "05-filter-approved", |win, cx| win.click("pr-filter-opt-approved", cx))?;
    { let line = format!("after Review: Approved: {:?}", visible(&mut check)); check.note(line); }
    check.step(w, "06-views-menu", |win, cx| win.click("saved-views-trigger", cx))?;
    check.step(w, "07-save-view-dialog", |win, cx| win.click("saved-views-save", cx))?;
    check.step(w, "08-save-view-named", |win, cx| win.input("Approved work", cx))?;
    check.step(w, "09-view-saved", |win, cx| win.click("saved-view-save", cx))?;
    let views = check.cx.update(|cx| {
        let menu = page.read(cx).views_menu().clone();
        menu.read(cx).views().iter().map(|v| (v.name.clone(), serde_json::to_string(&v.preferences).unwrap())).collect::<Vec<_>>()
    });
    { let line = format!("saved views: {views:?}"); check.note(line); }
    let p = page.clone();
    check.step(w, "10-filters-cleared", |_, cx| p.update(cx, |p, cx| p.set_menu_filters(Default::default(), cx)))?;
    let p = page.clone();
    check.step(w, "11-keyboard-select", |win, cx| {
        p.update(cx, |p, cx| p.focus_list(win, cx));
        win.press("down", cx);
        win.press("down", cx);
    })?;
    { let line = format!("selected: {:?}", check.cx.update(|cx| page.read(cx).selected_key())); check.note(line); }
    check.step(w, "12-keyboard-open", |win, cx| win.press("enter", cx))?;
    let detail = check.cx.update(|cx| ws.read(cx).detail.clone()).expect("detail opened");
    { let line = format!("opened #{:?}", check.cx.update(|cx| detail.read(cx).detail().map(|d| d.number))); check.note(line); }
    // Open #482 for the rest.
    let entry = fixture.state.lock().unwrap().data.list_studio.entries[0].clone();
    let ws2 = ws.clone();
    check.step(w, "13-summary", |win, cx| ws2.update(cx, |ws, cx| ws.open(pull_request_ref(&entry), DetailTab::Summary, win, cx)))?;
    let detail = check.cx.update(|cx| ws.read(cx).detail.clone()).unwrap();
    seen = check.calls(&fixture, seen);
    check.step(w, "14-merge-method-menu", |win, cx| win.click("pr-merge-method-trigger", cx))?;
    check.step(w, "15-merge-method-rebase", |win, cx| win.click(("menu-item", 2usize), cx))?;
    check.step(w, "16-merge-confirm", |win, cx| win.click("pr-merge", cx))?;
    check.step(w, "17-merged", |win, cx| win.click("pr-merge-confirm", cx))?;
    { let line = format!("state after merge: {:?}", check.cx.update(|cx| detail.read(cx).detail().map(|d| d.state))); check.note(line); }
    seen = check.calls(&fixture, seen);
    check.step(w, "18-timeline", |win, cx| win.click("pr-tab-Timeline", cx))?;
    check.step(w, "19-reaction-picker", |win, cx| win.click("react-add-r2", cx))?;
    check.step(w, "19b-reaction", |win, cx| win.click("react-pick-r2-thumbs-up", cx))?;
    let reactions = check.cx.update(|cx| detail.read(cx).activity().and_then(|a| a.comments.iter().find(|c| c.id == "r2").and_then(|c| c.reactions.clone())));
    { let line = format!("r2 reactions after toggle: {reactions:?}"); check.note(line); }
    check.step(w, "20-reply-open", |win, cx| win.click("reply-th2", cx))?;
    check.step(w, "21-reply-typed", |win, cx| win.input("Yes: the column shows position/size now.", cx))?;
    check.step(w, "22-reply-sent", |win, cx| win.click("reply-send-th2", cx))?;
    let replies = check.cx.update(|cx| detail.read(cx).activity().and_then(|a| a.review_threads.iter().find(|t| t.id == "th2").map(|t| t.comments.len())));
    { let line = format!("th2 comments after reply: {replies:?}"); check.note(line); }
    seen = check.calls(&fixture, seen);
    check.step(w, "23-code", |win, cx| win.click("pr-tab-Code", cx))?;
    let path = "apps/web/src/components/pull_request/pull_requests_page.rs";
    check.step(w, "24-file-viewed", |win, cx| win.click(format!("viewed-{path}"), cx))?;
    { let line = format!("viewed {path}: {}", check.cx.update(|cx| detail.read(cx).is_viewed(path))); check.note(line); }
    seen = check.calls(&fixture, seen);
    check.step(w, "25-guide", |win, cx| win.click("pr-tab-Guide", cx))?;
    check.step(w, "26-guide-next", |win, cx| win.click("guide-next", cx))?;
    check.calls(&fixture, seen);

    // --- Issues ---------------------------------------------------------------
    let (wi, iws) = check.open(1400., 900., |window, cx| cx.new(|cx| IssuesWorkspace::new(window, cx)))?;
    let ifixture = check.cx.update(|cx| iws.read(cx).fixture.clone());
    check.step(wi, "30-issues", |_, _| {})?;
    let panel = check.cx.update(|cx| iws.read(cx).panel.clone()).unwrap();
    check.step(wi, "31-issue-react", |win, cx| win.click("issue-react-heart", cx))?;
    check.step(wi, "32-issue-labels-open", |win, cx| win.click("issue-labels-edit", cx))?;
    check.step(wi, "33-issue-label-picked", |win, cx| win.click("issue-label-bug", cx))?;
    check.step(wi, "34-issue-labels-saved", |win, cx| win.click("issue-labels-edit", cx))?;
    { let line = format!("labels: {:?}", check.cx.update(|cx| panel.read(cx).detail().map(|d| d.labels.iter().map(|l| l.name.clone()).collect::<Vec<_>>()))); check.note(line); }
    check.step(wi, "35-issue-state-menu", |win, cx| win.click("issue-state-trigger", cx))?;
    check.step(wi, "36-issue-closed", |win, cx| win.click(("menu-item", 0usize), cx))?;
    { let line = format!("state: {:?}", check.cx.update(|cx| panel.read(cx).detail().map(|d| d.state.name.clone()))); check.note(line); }
    check.step(wi, "37-issue-new", |win, cx| win.click("issues-new", cx))?;
    check.step(wi, "38-issue-template", |win, cx| win.click("issue-template-bug", cx))?;
    check.step(wi, "39-issue-created", |win, cx| win.click("issue-new-submit", cx))?;
    check.calls(&ifixture, 0);

    // --- Measurements -----------------------------------------------------------
    let mut measurements = String::new();
    let base = fixtures();
    let big = FixtureSource::standard().with_list(large_list(&base.list_studio, 2000), false);
    let (wp, big_page) = check.open(1300., 900., |window, cx| {
        let sources = vec![EnvironmentListSource { environment_id: EnvironmentId::new("env-studio"), label: "Studio".into(), source: Rc::new(big.clone()) }];
        cx.new(|cx| {
            let mut page = PullRequestsPage::new(sources, window, cx);
            page.now_ms = Some(SCENE_NOW_MS);
            page
        })
    })?;
    check.cx.update_window(wp, |_, window, cx| window.render_frame(cx))?;
    let bp = big_page.clone();
    check.cx.update_window(wp, |_, window, cx| bp.update(cx, |p, cx| p.focus_search(window, cx)))?;
    writeln!(measurements, "filter per keystroke, 2,000 rows (debug build unless noted)")?;
    writeln!(measurements, "key | rows shown | refilter µs | keystroke to frame µs")?;
    let mut worst_filter = 0u128;
    let mut worst_frame = 0u128;
    for ch in "sidebar perf".chars() {
        let started = Instant::now();
        check.cx.update_window(wp, |_, window, cx| {
            window.input(&ch.to_string(), cx);
            window.render_frame(cx);
        })?;
        let frame = started.elapsed().as_micros();
        let timing = check.cx.update(|cx| big_page.read(cx).last_filter);
        worst_filter = worst_filter.max(timing.micros);
        worst_frame = worst_frame.max(frame);
        writeln!(measurements, "{ch:?} | {} of {} | {} | {}", timing.rows_out, timing.rows_in, timing.micros, frame)?;
    }
    writeln!(measurements, "worst refilter: {worst_filter} µs; worst keystroke to frame: {worst_frame} µs")?;
    check.cx.update_window(wp, |_, window, cx| window.render_frame(cx))?;
    let path = out.join(format!("40-2000-rows-filtered{}.png", check.suffix));
    check.cx.capture_screenshot(wp)?.save(&path)?;

    let code_fixture = FixtureSource::new(fixtures(), large_patch(300));
    let reference = pull_request_ref(&base.list_studio.entries[0]);
    let (wc, panel) = check.open(900., 900., |_, cx| {
        let source: Rc<dyn PullRequestDetailSource> = Rc::new(code_fixture.clone());
        cx.new(|cx| PullRequestDetailPanel::new(source, reference, cx))
    })?;
    check.cx.update_window(wc, |_, window, cx| window.render_frame(cx))?;
    let started = Instant::now();
    check.cx.update_window(wc, |_, window, cx| window.click("pr-tab-Code", cx))?;
    check.cx.run_until_parked();
    check.cx.update_window(wc, |_, window, cx| window.render_frame(cx))?;
    let to_frame = started.elapsed().as_secs_f64() * 1000.;
    check.cx.update_window(wc, |_, window, cx| window.render_frame(cx))?;
    let measured = check.cx.update(|cx| panel.read(cx).code_first_paint_ms);
    let files = check.cx.update(|cx| t3code_web::components::pull_request::pull_request_detail_panel::code_file_paths(panel.read(cx)).len());
    writeln!(measurements, "\nCode tab, {files} files: click to first painted frame {to_frame:.1} ms (panel's own measure {measured:?} ms)")?;
    let path = out.join(format!("41-code-300-files{}.png", check.suffix));
    check.cx.capture_screenshot(wc)?.save(&path)?;

    std::fs::write(out.join(format!("transcript{}.txt", check.suffix)), &check.log)?;
    std::fs::write(out.join(format!("measurements{}.txt", check.suffix)), &measurements)?;
    print!("{measurements}");
    Ok(())
}
