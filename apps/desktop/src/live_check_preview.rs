// live-check phase `preview`, after `seed`, on a server whose browser reaches the page at
// `T3_PREVIEW_PAGE` (a local page titled "page one") and nothing listens on `T3_PREVIEW_CLOSED`.
// A browser tab the server opened shows in the right panel with the page's title; its favicon
// slot falls back to the globe for a private host; preview.find answers that streamed pages
// cannot be searched; the More menu's zoom, appearance, device toolbar, hard reload and
// clearing are each read back from `preview.list`; a page that does not load shows the fork's
// unreachable state; "Float preview over chat" closes the panel and floats the tab, the
// handle drags it, a corner resizes it, and "Open in right panel" puts it back.

use std::rc::Rc;

use t3code_contracts::{PreviewAppearancePreference, PreviewNavStatus, PreviewOpenInput, PreviewRuntime, PreviewSessionSnapshot, PreviewViewportSetting};
use t3code_web::components::preview::preview_source::PreviewSource;
use t3code_web::components::preview::preview_view::PreviewView;
use t3code_web::components::preview::thread_preview_mini_player::{self, MiniPlayerKey, PreviewMiniPlayers};
use t3code_web::components::ui::toast::Toaster;
use t3code_web::shortcuts::commands::FindInPreview;

use super::*;

/// The tab as the server has it now (`preview.list`).
fn server_tab(check: &mut Check, source: &Rc<dyn PreviewSource>, thread: &t3code_contracts::ThreadId, tab: &str) -> anyhow::Result<PreviewSessionSnapshot> {
    let list = source.list(thread);
    let task = check.cx.update(|cx| cx.spawn(async move |_| list.await));
    let result = check.wait_task(task)?.map_err(|e| anyhow::anyhow!("preview.list: {}", e.0))?;
    result.sessions.into_iter().find(|s| s.tab_id == tab).context("the tab is on the server")
}

fn browser(check: &mut Check) -> Option<Entity<PreviewView>> {
    check.read(|root, cx| root.right_panel().read(cx).active_browser(cx))
}

/// Presses at `from`, moves to `to` in ten steps and releases, drawing a frame after each
/// event as the real window would.
fn drag(check: &mut Check, from: Point<Pixels>, to: Point<Pixels>) {
    use gpui_kit::test::TestEventExt as _;
    check.cx.update_window(check.window, |_, w, cx| w.pointer_move(from, None, cx)).ok();
    check.frame();
    check.cx.update_window(check.window, |_, w, cx| w.pointer_down(from, MouseButton::Left, cx)).ok();
    check.frame();
    for step in 1..=10 {
        let at = from + (to - from) * (step as f32 / 10.);
        check.cx.update_window(check.window, |_, w, cx| w.pointer_move(at, Some(MouseButton::Left), cx)).ok();
        check.frame();
    }
    check.cx.update_window(check.window, |_, w, cx| w.pointer_up(to, MouseButton::Left, cx)).ok();
    check.frame();
}

fn click(check: &mut Check, id: &'static str) {
    check.cx.update_window(check.window, |_, w, cx| w.click(id, cx)).ok();
    check.frame();
}

/// Pumps until the server's copy of the tab satisfies `ok`.
fn until_server(
    check: &mut Check,
    source: &Rc<dyn PreviewSource>,
    thread: &t3code_contracts::ThreadId,
    tab: &str,
    what: &str,
    ok: impl Fn(&PreviewSessionSnapshot) -> bool,
) -> anyhow::Result<PreviewSessionSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = server_tab(check, source, thread, tab)?;
        if ok(&snapshot) {
            check.note(format!("ok: {what}"));
            return Ok(snapshot);
        }
        if Instant::now() > deadline {
            bail!("timed out waiting for {what}: {snapshot:?}");
        }
        check.wait(Duration::from_millis(200));
    }
}

pub(super) fn run_preview(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    let page = std::env::var("T3_PREVIEW_PAGE").context("T3_PREVIEW_PAGE")?;
    let closed = std::env::var("T3_PREVIEW_CLOSED").context("T3_PREVIEW_CLOSED")?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, &format!("Preview {}", now_ms() % 100_000))?;
    check.go(&key);
    check.wait(Duration::from_millis(500));
    let environment = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let thread = t3code_contracts::ThreadId::new(key.thread_id.0.as_ref());
    let data = check.read(|root, _| root.data().clone());
    let source = check.cx.update(|cx| data.preview_source(&environment, cx)).context("preview source")?;

    // A tab the server opened, as an agent's preview_open leaves it.
    let open = source.open(PreviewOpenInput {
        thread_id: thread.clone(),
        url: Some(page.clone()),
        viewport: None,
        profile_id: None,
        runtime: Some(PreviewRuntime::Server),
        color_scheme: None,
        zoom_factor: None,
        reveal: None,
    });
    let task = check.cx.update(|cx| cx.spawn(async move |_| open.await));
    let opened = check.wait_task(task)?.map_err(|e| anyhow::anyhow!("preview.open: {}", e.0))?;
    let tab = opened.tab_id.clone();
    let panel = check.read(|root, _| root.right_panel().clone());
    let open_tab = tab.clone();
    check.cx.update(|cx| panel.update(cx, |panel, cx| panel.open_browser_tab(&open_tab, cx)));
    check.pump(|c| browser(c).is_some(), Duration::from_secs(10), "the browser tab mounts the preview view")?;
    let view = browser(check).context("preview view")?;
    check.pump(
        |c| c.cx.update(|cx| view.read(cx).active_surface().is_some_and(|s| s.read(cx).settled())),
        Duration::from_secs(60),
        "the first frame",
    )?;
    check.pump(|c| c.cx.update(|cx| view.read(cx).tab_title() == "page one"), Duration::from_secs(30), "the page's title")?;
    check.wait(Duration::from_millis(600));
    check.shot("preview-00-tab.png")?;
    let (title, url) = check.cx.update(|cx| (view.read(cx).tab_title(), view.read(cx).tab_url().map(str::to_owned)));
    check.check("the tab strip names the page (surfaceTitle)", title == "page one", &title);
    let host = url.as_deref().and_then(t3code_web::components::chat::timeline_link_favicons::external_link_host).unwrap_or_default();
    check.check(
        "a private host keeps the globe (isPublicFaviconHost)",
        !t3code_web::components::chat::timeline_link_favicons::is_public_favicon_host(&host),
        &host,
    );

    // preview.find on a streamed page.
    let focus = check.cx.update(|cx| view.focus_handle(cx));
    check.cx.update_window(check.window, |_, w, cx| w.focus(&focus, cx)).ok();
    check.action(FindInPreview);
    let toasts = check.cx.update(|cx| Toaster::titles(cx));
    check.check(
        "preview.find says find is unavailable for a streamed page",
        toasts.iter().any(|t| t == "Find on page isn't available for this page"),
        format!("{toasts:?}"),
    );

    // Control, then the More menu.
    let surface = check.cx.update(|cx| view.read(cx).active_surface()).context("surface")?;
    let you = |c: &mut Check| c.cx.update(|cx| surface.read(cx).control().is_some_and(|ctl| ctl.controller == t3code_client_runtime::preview::server_browser_stream::Controller::You));
    if !you(check) {
        click(check, "preview-control");
        check.pump(you, Duration::from_secs(10), "took control")?;
    }
    click(check, "preview-more");
    check.wait(Duration::from_millis(300));
    check.shot("preview-01-more-menu.png")?;
    click(check, "preview-zoom-in");
    let snapshot = until_server(check, &source, &thread, &tab, "zoom in reads back 110%", |s| s.zoom_factor == Some(1.1))?;
    check.check("Zoom in sets 1.1 on the server", snapshot.zoom_factor == Some(1.1), format!("{:?}", snapshot.zoom_factor));
    check.wait(Duration::from_millis(300));
    check.shot("preview-02-zoomed.png")?;
    click(check, "preview-zoom-out");
    click(check, "preview-zoom-out");
    let snapshot = until_server(check, &source, &thread, &tab, "zoom out twice reads back 90%", |s| s.zoom_factor == Some(0.9))?;
    check.check("Zoom out steps along the fork's levels", snapshot.zoom_factor == Some(0.9), format!("{:?}", snapshot.zoom_factor));
    click(check, "preview-zoom-reset");
    let snapshot = until_server(check, &source, &thread, &tab, "reset zoom reads back 100%", |s| s.zoom_factor == Some(1.0))?;
    check.check("Reset zoom sets 1.0", snapshot.zoom_factor == Some(1.0), format!("{:?}", snapshot.zoom_factor));
    click(check, "preview-appearance");
    check.wait(Duration::from_millis(200));
    check.shot("preview-03-appearance.png")?;
    click(check, "preview-color-scheme-dark");
    let snapshot = until_server(check, &source, &thread, &tab, "dark appearance reads back", |s| s.color_scheme == Some(PreviewAppearancePreference::Dark))?;
    check.check("Appearance > Dark reaches the server", snapshot.color_scheme == Some(PreviewAppearancePreference::Dark), format!("{:?}", snapshot.color_scheme));
    check.wait(Duration::from_millis(500));
    check.shot("preview-04-dark.png")?;
    click(check, "preview-more");
    click(check, "preview-device-toolbar");
    let snapshot = until_server(check, &source, &thread, &tab, "the device toolbar's freeform viewport", |s| matches!(s.viewport, Some(PreviewViewportSetting::Freeform(_))))?;
    check.check("Show device toolbar sets a freeform viewport", matches!(snapshot.viewport, Some(PreviewViewportSetting::Freeform(_))), format!("{:?}", snapshot.viewport));
    check.wait(Duration::from_millis(500));
    check.shot("preview-05-device.png")?;
    click(check, "preview-more");
    click(check, "preview-device-toolbar");
    until_server(check, &source, &thread, &tab, "back to fill", |s| matches!(s.viewport, Some(PreviewViewportSetting::Fill(_)) | None))?;
    for (id, what) in [("preview-hard-reload", "Hard reload"), ("preview-clear-cookies", "Clear cookies"), ("preview-clear-cache", "Clear cache")] {
        click(check, "preview-more");
        click(check, id);
        check.wait(Duration::from_millis(800));
        let toasts = check.cx.update(|cx| Toaster::titles(cx));
        check.check(&format!("{what} is accepted by the server"), !toasts.iter().any(|t| t == "Unable to change the browser tab"), format!("{toasts:?}"));
    }
    until_server(check, &source, &thread, &tab, "the page settles after the hard reload", |s| matches!(s.nav_status, PreviewNavStatus::Success(_)))?;

    // A page that does not load.
    check.cx.update_window(check.window, |_, w, cx| view.update(cx, |view, cx| view.navigate(closed.clone(), w, cx))).ok();
    let snapshot = until_server(check, &source, &thread, &tab, "the unreachable page is LoadFailed", |s| matches!(s.nav_status, PreviewNavStatus::LoadFailed(_)))?;
    check.pump(
        |c| c.cx.update_window(c.window, |_, w, _| w.try_find("preview-unreachable-details").is_some()).unwrap_or(false),
        Duration::from_secs(10),
        "the view shows the unreachable state",
    )?;
    check.wait(Duration::from_millis(300));
    check.shot("preview-06-unreachable.png")?;
    check.check("the server reports LoadFailed", matches!(snapshot.nav_status, PreviewNavStatus::LoadFailed(_)), format!("{:?}", snapshot.nav_status));
    click(check, "preview-unreachable-details");
    check.wait(Duration::from_millis(200));
    check.shot("preview-07-unreachable-details.png")?;
    click(check, "preview-unreachable-reload");
    check.cx.update_window(check.window, |_, w, cx| view.update(cx, |view, cx| view.navigate(page.clone(), w, cx))).ok();
    until_server(check, &source, &thread, &tab, "back on the page", |s| matches!(&s.nav_status, PreviewNavStatus::Success(n) if n.title == "page one"))?;
    check.wait(Duration::from_millis(500));

    // The mini player.
    let mini = MiniPlayerKey { environment: environment.clone(), thread: thread.clone() };
    let scoped = t3code_web::fixtures::scoped_key(&key).to_string();
    click(check, "preview-float");
    let panel_scoped = scoped.clone();
    check.pump(move |c| c.read(|root, cx| !root.right_panels().read(cx).is_open(&panel_scoped)), Duration::from_secs(10), "floating closes the panel")?;
    let mini_tab = tab.clone();
    let floating_key = mini.clone();
    check.pump(move |c| c.cx.update(|cx| PreviewMiniPlayers::tab(&floating_key, cx) == Some(mini_tab.as_str())), Duration::from_secs(5), "the thread's player shows the tab")?;
    check.pump(|c| c.cx.update(|cx| thread_preview_mini_player::current_frame(cx).is_some()), Duration::from_secs(10), "the player lays out")?;
    check.wait(Duration::from_millis(1500));
    check.shot("preview-08-mini-player.png")?;
    let start = check.cx.update(|cx| thread_preview_mini_player::current_frame(cx)).context("frame")?;
    check.note(format!("mini player frame {start:?}"));
    let found = check.cx.update_window(check.window, |_, w, _| (w.try_find("preview-mini-player").map(|e| e.bounds()), w.try_find("mini-player-handle").map(|e| e.bounds()))).ok();
    check.note(format!("mini player element bounds {found:?}"));
    // Drag the handle 200px left and 120px up.
    // Hovering the handle shows the pill over it; the pill's own padding is its grab area.
    check.cx.update_window(check.window, |_, w, cx| w.hover("mini-player-handle", cx)).ok();
    check.frame();
    let pill = check.cx.update_window(check.window, |_, w, _| w.try_find("mini-player-pill").map(|e| e.bounds())).ok().flatten().context("pill bounds")?;
    let from = pill.origin + point(px(1.5), pill.size.height / 2.);
    let to = from + point(px(-200.), px(-120.));
    drag(check, from, to);
    check.wait(Duration::from_millis(300));
    let moved = check.cx.update(|cx| PreviewMiniPlayers::get(&mini, cx).cloned()).context("player")?;
    let dragged = check.cx.update(|cx| thread_preview_mini_player::current_frame(cx)).context("frame")?;
    check.check(
        "the handle drags the player (position stored per thread)",
        moved.position.is_some() && (dragged.x - start.x).abs() > 1. && (dragged.y - start.y).abs() > 1.,
        format!("{start:?} -> {dragged:?}"),
    );
    check.shot("preview-09-dragged.png")?;
    // Resize from the bottom-left corner, 120px out.
    let corner = check.cx.update_window(check.window, |_, w, _| w.try_find("mini-player-resize-SouthWest").map(|e| e.bounds())).ok().flatten().context("corner bounds")?;
    let from = corner.center();
    let to = from + point(px(-120.), px(80.));
    drag(check, from, to);
    check.wait(Duration::from_millis(300));
    let resized = check.cx.update(|cx| thread_preview_mini_player::current_frame(cx)).context("frame")?;
    let aspect = |f: &thread_preview_mini_player::Frame| f.width / f.height;
    check.check(
        "a corner resizes the player and keeps the page's shape",
        resized.width > dragged.width + 20. && (aspect(&resized) - aspect(&dragged)).abs() < 0.02,
        format!("{dragged:?} -> {resized:?}"),
    );
    check.shot("preview-10-resized.png")?;
    // Back into the panel.
    let handle = check.cx.update_window(check.window, |_, w, _| w.try_find("mini-player-handle").map(|e| e.bounds())).ok().flatten().context("handle bounds")?;
    check.cx.update_window(check.window, |_, w, cx| w.hover("mini-player-handle", cx)).ok();
    let _ = handle;
    check.wait(Duration::from_millis(200));
    check.shot("preview-11-pill.png")?;
    click(check, "mini-player-open-panel");
    let panel_scoped = scoped.clone();
    check.pump(move |c| c.read(|root, cx| root.right_panels().read(cx).is_open(&panel_scoped)), Duration::from_secs(10), "Open in right panel reopens the panel")?;
    let closed_key = mini.clone();
    check.pump(move |c| c.cx.update(|cx| PreviewMiniPlayers::get(&closed_key, cx).is_none()), Duration::from_secs(5), "the player closes")?;
    check.wait(Duration::from_millis(800));
    check.shot("preview-12-back-in-panel.png")?;
    Ok(())
}
