// Scripted end-to-end check of the app shell in a headless window: drives
// hover, clicks, menus, keyboard, resize and window drag through real event
// dispatch, saves screenshots, samples pixels against the Graphite tokens,
// measures sidebar rows, and times scroll and selection frames.
// Usage: shell_check <out-dir> [--bench]
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, ensure};
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::*;
use image::RgbaImage;
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::sidebar::fixtures;
use t3code_web::components::sidebar::sidebar_notifications::SidebarNotifications;
use t3code_web::desktop_notifications::NotificationInbox;
use t3code_web::fixtures::FixtureStore;
use t3code_web::components::sidebar::model::SidebarSnapshot;
use t3code_web::components::sidebar_logic::SidebarListEntry;
use t3code_web::theme_palette::{Appearance, ThemeDocument, set_app_theme};
use t3code_web::{AppRoot, AppTheme, scenes};

struct SceneHost(AnyView);

impl Render for SceneHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

struct Check {
    cx: HeadlessAppContext,
    out: std::path::PathBuf,
    report: String,
    failures: usize,
}

fn options(width: f32, height: f32, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
        show: false,
        focus: true,
        ..component::TitleBar::window_options()
    }
}

fn hex(image: &RgbaImage, x: f32, y: f32) -> String {
    let p = image.get_pixel((x * 2.) as u32, (y * 2.) as u32);
    format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2])
}

/// Channel distance between two `#rrggbb` strings.
fn distance(a: &str, b: &str) -> u8 {
    let parse = |s: &str| {
        (0..3)
            .map(|i| u8::from_str_radix(&s[1 + i * 2..3 + i * 2], 16).unwrap_or(0))
            .collect::<Vec<_>>()
    };
    parse(a).iter().zip(parse(b)).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(255)
}

impl Check {
    fn set_appearance(&mut self, appearance: Appearance) {
        self.cx.update(|cx| set_app_theme(AppTheme::new(&ThemeDocument::graphite(), appearance), cx));
    }

    fn open_app(
        &mut self,
        snapshot: SidebarSnapshot,
        platform: TitlebarPlatform,
        width: f32,
        height: f32,
    ) -> anyhow::Result<(AnyWindowHandle, Entity<AppRoot>)> {
        let (handle, root) = self.cx.update(|cx| {
            gpui_kit::open_window(options(width, height, cx), cx, |window, cx| {
                cx.new(|cx| {
                    let root = AppRoot::with_snapshot(snapshot, platform, window, cx);
                    root.layout().update(cx, |l, _| {
                        l.force_caption_buttons(true);
                        l.count_window_moves_only(true);
                    });
                    root
                })
            })
        })?;
        self.frame(handle)?;
        Ok((handle, root))
    }

    fn open_scene(&mut self, name: &str, width: f32, height: f32) -> anyhow::Result<AnyWindowHandle> {
        let build = scenes::scene(name).context("scene")?;
        let (handle, _) = self.cx.update(|cx| {
            gpui_kit::open_window(options(width, height, cx), cx, |window, cx| {
                let view = build(window, cx);
                cx.new(|_| SceneHost(view))
            })
        })?;
        self.frame(handle)?;
        Ok(handle)
    }

    fn frame(&mut self, handle: AnyWindowHandle) -> anyhow::Result<()> {
        self.cx.update_window(handle, |_, w, cx| w.render_frame(cx))?;
        self.cx.run_until_parked();
        self.cx.update_window(handle, |_, w, cx| w.render_frame(cx))?;
        Ok(())
    }

    fn shot(&mut self, handle: AnyWindowHandle, name: &str) -> anyhow::Result<RgbaImage> {
        self.frame(handle)?;
        let image = self.cx.capture_screenshot(handle)?;
        image.save(self.out.join(name))?;
        writeln!(self.report, "- screenshot `{name}`")?;
        Ok(image)
    }

    fn expect(&mut self, ok: bool, what: impl std::fmt::Display) {
        writeln!(self.report, "- [{}] {what}", if ok { "pass" } else { "FAIL" }).ok();
        if !ok {
            self.failures += 1;
        }
    }

    /// Compares a sampled pixel to the token hex, allowing `tolerance` per channel
    /// (the surface grain lifts flat fills by about 1-2 levels).
    fn sample(&mut self, image: &RgbaImage, label: &str, x: f32, y: f32, want: &str, tolerance: u8) {
        let got = hex(image, x, y);
        let d = distance(&got, want);
        self.expect(d <= tolerance, format!("{label} at ({x},{y}): got {got}, token {want}, Δ{d}"));
    }

    fn with_window<R>(&mut self, handle: AnyWindowHandle, f: impl FnOnce(&mut Window, &mut App) -> R) -> anyhow::Result<R> {
        self.cx.update_window(handle, |_, w, cx| f(w, cx))
    }

    fn sidebar_entry_bounds(&mut self, root: &Entity<AppRoot>, ix: usize) -> Option<Bounds<Pixels>> {
        self.cx.update(|cx| {
            let sidebar = root.read(cx).layout().read(cx).sidebar().clone();
            sidebar.read(cx).entry_bounds(ix)
        })
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().context("usage: shell_check <out-dir> [--bench]")?);
    let bench = args.any(|a| a == "--bench");
    std::fs::create_dir_all(&out)?;
    let cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let mut check = Check {
        cx,
        out,
        report: String::from("# shell_check\n\n"),
        failures: 0,
    };
    check.cx.update(|cx| {
        t3code_web::init(cx);
        // The footer bell shows once an inbox has items (the desktop host loads them).
        NotificationInbox::set_items(FixtureStore::from_sidebar(fixtures::showcase_snapshot()).inbox_items(), cx);
    });

    if bench {
        run_bench(&mut check)?;
    } else {
        run_shots(&mut check)?;
        run_interactions(&mut check)?;
    }
    std::fs::write(check.out.join(if bench { "bench.md" } else { "report.md" }), &check.report)?;
    println!("{}", check.report);
    ensure!(check.failures == 0, "{} checks failed", check.failures);
    Ok(())
}

/// Static screenshots at the default and minimum size, dark and light, with
/// token samples.
fn run_shots(check: &mut Check) -> anyhow::Result<()> {
    for (appearance, tag) in [(Appearance::Dark, "dark"), (Appearance::Light, "light")] {
        check.set_appearance(appearance);
        writeln!(check.report, "\n## {tag}\n")?;
        let (app, _) = check.open_app(fixtures::showcase_snapshot(), TitlebarPlatform::Overlay, 1100., 780.)?;
        let image = check.shot(app, &format!("app-{tag}-1100x780.png"))?;
        let dark = appearance == Appearance::Dark;
        let t = |d: &'static str, l: &'static str| if dark { d } else { l };
        check.sample(&image, "main canvas", 700., 200., t("#141415", "#fcfcfc"), 3);
        // The surface grain lifts the fill about 4 levels in dark mode, as Chromium's does.
        check.sample(&image, "sidebar fill", 200., 20., t("#141415", "#f3f3f3"), 5);
        check.sample(&image, "sidebar right border", 255.5, 400., t("#262627", "#dddddd"), 4);
        // The window opens on a draft thread; the chat header has no bottom rule (`06-ui-chat.md` 2.2).
        check.sample(&image, "draft row fill", 200., 700., t("#1d1918", "#f1ece9"), 6);
        check.sample(&image, "bell unread dot", 177., 747., t("#ff6b66", "#be1744"), 24);
        let (min, _) = check.open_app(fixtures::showcase_snapshot(), TitlebarPlatform::Overlay, 400., 360.)?;
        check.shot(min, &format!("app-{tag}-400x360.png"))?;
        // Below 768px the toggle opens the sidebar as a sheet.
        let toggle = point(px(27.), px(20.));
        check.with_window(min, |w, cx| {
            w.pointer_down(toggle, MouseButton::Left, cx);
            w.pointer_up(toggle, MouseButton::Left, cx);
        })?;
        check.shot(min, &format!("app-{tag}-400x360-sheet.png"))?;
        let (mac, _) = check.open_app(fixtures::showcase_snapshot(), TitlebarPlatform::Mac, 1100., 780.)?;
        check.shot(mac, &format!("app-mac-{tag}-1100x780.png"))?;
        let (mac_min, _) = check.open_app(fixtures::showcase_snapshot(), TitlebarPlatform::Mac, 400., 360.)?;
        check.shot(mac_min, &format!("app-mac-{tag}-400x360.png"))?;
        let scene = check.open_scene("sidebar", 1100., 780.)?;
        let image = check.shot(scene, &format!("sidebar-{tag}.png"))?;
        check.sample(&image, "hovered card fill", 160., 610., t("#222224", "#e9e9e9"), 3);
    }
    check.set_appearance(Appearance::Dark);
    Ok(())
}

/// Hover, click, context menu, inbox, keyboard, resize, toggles and drag.
fn run_interactions(check: &mut Check) -> anyhow::Result<()> {
    writeln!(check.report, "\n## interactions (dark, 1100x780)\n")?;
    let (app, root) = check.open_app(fixtures::showcase_snapshot(), TitlebarPlatform::Overlay, 1100., 780.)?;
    let sidebar = check.cx.update(|cx| root.read(cx).layout().read(cx).sidebar().clone());

    // Row metrics from the list layout.
    let entries = check.cx.update(|cx| sidebar.read(cx).list().entries.clone());
    writeln!(check.report, "\n### row metrics\n\n| entry | kind | top | height |\n|---|---|---|---|")?;
    for ix in 0..entries.len().min(10) {
        let Some(b) = check.sidebar_entry_bounds(&root, ix) else { continue };
        let kind = match &entries[ix] {
            SidebarListEntry::Thread { variant, .. } => format!("{variant:?}"),
            other => format!("{:?}", std::mem::discriminant(other)),
        };
        writeln!(check.report, "| {ix} | {kind} | {} | {} |", f32::from(b.origin.y), f32::from(b.size.height))?;
    }
    writeln!(check.report)?;
    let first = check.sidebar_entry_bounds(&root, 0).context("entry 0 laid out")?;
    let second = check.sidebar_entry_bounds(&root, 1).context("entry 1 laid out")?;
    check.expect(
        f32::from(first.size.height) == 82.,
        format!("card with meta: 78px box + 2x2px padding = 82 (got {})", f32::from(first.size.height)),
    );
    check.expect(
        f32::from(second.size.height) == 65.,
        format!("card without meta: 60 + 4 + 1px gap = 65 (got {})", f32::from(second.size.height)),
    );
    check.expect(
        f32::from(first.origin.y) == 88.,
        format!("list starts under topbar 40 + search group 48 = 88 (got {})", f32::from(first.origin.y)),
    );

    // Hover a row: fill swaps to sidebar-row-hover and the actions appear.
    let row = second.center();
    check.with_window(app, |w, cx| w.pointer_move(row, None, cx))?;
    let image = check.shot(app, "interact-hover.png")?;
    check.sample(&image, "hovered row fill", 120., f32::from(row.y) + 20., "#222224", 3);

    // Click it: it becomes the open thread.
    check.with_window(app, |w, cx| {
        w.pointer_down(row, MouseButton::Left, cx);
        w.pointer_up(row, MouseButton::Left, cx);
    })?;
    let active = check.cx.update(|cx| sidebar.read(cx).active_thread().map(|k| k.thread_id.0.clone()));
    check.expect(active.as_deref() == Some("a10"), format!("click opens the row (active = {active:?})"));
    check.with_window(app, |w, cx| w.pointer_move(point(px(700.), px(400.)), None, cx))?;
    let image = check.shot(app, "interact-click.png")?;
    check.sample(&image, "open row fill", 120., f32::from(row.y) + 20., "#28282b", 3);

    // Context menu.
    let third = check.sidebar_entry_bounds(&root, 2).context("entry 2")?.center();
    check.with_window(app, |w, cx| {
        w.pointer_down(third, MouseButton::Right, cx);
        w.pointer_up(third, MouseButton::Right, cx);
    })?;
    let open = check.cx.update(|cx| sidebar.read(cx).is_context_menu_open());
    check.expect(open, "right-click opens the web-styled context menu");
    // Hover the Snooze row to open its submenu.
    check.with_window(app, |w, cx| w.pointer_move(third + point(px(40.), px(4. + 28. * 3. + 14.)), None, cx))?;
    check.shot(app, "interact-context-menu.png")?;
    check.with_window(app, |w, cx| w.press("escape", cx))?;
    let open = check.cx.update(|cx| sidebar.read(cx).is_context_menu_open());
    check.expect(!open, "escape closes the menu");

    // Notifications inbox from the bell (5th footer button).
    let bell = point(px(8. + 4. * 36. + 16.), px(780. - 8. - 16.));
    check.with_window(app, |w, cx| {
        w.pointer_move(bell, None, cx);
        w.pointer_down(bell, MouseButton::Left, cx);
        w.pointer_up(bell, MouseButton::Left, cx);
    })?;
    let open = check.cx.update(|cx| SidebarNotifications::view(sidebar.downgrade(), cx).read(cx).is_open());
    check.expect(open, "bell opens the notifications inbox");
    check.shot(app, "interact-inbox.png")?;
    let outside = point(px(900.), px(300.));
    check.with_window(app, |w, cx| {
        w.pointer_down(outside, MouseButton::Left, cx);
        w.pointer_up(outside, MouseButton::Left, cx);
    })?;
    let open = check.cx.update(|cx| SidebarNotifications::view(sidebar.downgrade(), cx).read(cx).is_open());
    check.expect(!open, "a click outside closes the inbox");

    // Keyboard: focus moves with arrows, enter opens.
    check.with_window(app, |w, cx| {
        w.pointer_down(first.center(), MouseButton::Left, cx);
        w.pointer_up(first.center(), MouseButton::Left, cx);
        w.press("down", cx);
        w.press("down", cx);
    })?;
    check.shot(app, "interact-keyboard.png")?;
    check.with_window(app, |w, cx| w.press("enter", cx))?;
    let active = check.cx.update(|cx| sidebar.read(cx).active_thread().map(|k| k.thread_id.0.clone()));
    check.expect(active.as_deref() == Some("a2"), format!("down, down, enter opens the third row (active = {active:?})"));

    // Resize the sidebar from its rail, then past the 640px main-pane reserve.
    let layout = check.cx.update(|cx| root.read(cx).layout().clone());
    check.with_window(app, |w, cx| w.drag(point(px(256.), px(400.)), point(px(360.), px(400.)), cx))?;
    let width = check.cx.update(|cx| f32::from(layout.read(cx).sidebar_width()));
    check.expect(width == 360., format!("rail drag to x=360 sets width 360 (got {width})"));
    check.shot(app, "interact-resized.png")?;
    check.with_window(app, |w, cx| w.drag(point(px(360.), px(400.)), point(px(900.), px(400.)), cx))?;
    let width = check.cx.update(|cx| f32::from(layout.read(cx).sidebar_width()));
    check.expect(width == 460., format!("width stops at 1100 - 640 = 460 (got {width})"));
    check.with_window(app, |w, cx| w.drag(point(px(460.), px(400.)), point(px(20.), px(400.)), cx))?;
    let width = check.cx.update(|cx| f32::from(layout.read(cx).sidebar_width()));
    check.expect(width == 208., format!("width stops at the 208 minimum (got {width})"));

    // Drag strip vs controls: press-and-drift on a toggle clicks it and never
    // moves the window; the same on empty header space moves it.
    let toggle = point(px(13. + 14.), px(20.));
    check.with_window(app, |w, cx| {
        w.pointer_down(toggle, MouseButton::Left, cx);
        w.pointer_move(toggle + point(px(2.), px(0.)), Some(MouseButton::Left), cx);
        w.pointer_up(toggle + point(px(2.), px(0.)), MouseButton::Left, cx);
    })?;
    let (moves, open) = check.cx.update(|cx| (layout.read(cx).window_move_count(), layout.read(cx).is_sidebar_open()));
    check.expect(moves == 0 && !open, format!("press-drift on the sidebar toggle collapses it without a window move (moves {moves})"));
    check.shot(app, "interact-collapsed.png")?;
    for at in [point(px(600.), px(20.)), point(px(160.), px(20.))] {
        check.with_window(app, |w, cx| {
            w.pointer_down(at, MouseButton::Left, cx);
            w.pointer_move(at + point(px(3.), px(0.)), Some(MouseButton::Left), cx);
            w.pointer_up(at + point(px(3.), px(0.)), MouseButton::Left, cx);
        })?;
    }
    let moves = check.cx.update(|cx| layout.read(cx).window_move_count());
    check.expect(moves == 2, format!("press-drift on empty titlebar space starts a window move (moves {moves})"));

    // Terminal and right panel toggles swap icons and open their slots.
    for x in [1100. - 138. - 13. - 28. - 4. - 14., 1100. - 138. - 13. - 14.] {
        let at = point(px(x), px(20.));
        check.with_window(app, |w, cx| {
            w.pointer_down(at, MouseButton::Left, cx);
            w.pointer_up(at, MouseButton::Left, cx);
        })?;
    }
    let (terminal, right) = check.cx.update(|cx| (layout.read(cx).is_terminal_open(), layout.read(cx).is_right_panel_open()));
    check.expect(terminal && right, "terminal and right panel toggles open their slots");
    check.with_window(app, |w, cx| w.pointer_move(point(px(600.), px(400.)), None, cx))?;
    check.shot(app, "interact-panels.png")?;
    Ok(())
}

fn percentile(samples: &mut [Duration], p: f64) -> f64 {
    samples.sort();
    let ix = ((samples.len() as f64 - 1.) * p).round() as usize;
    samples[ix].as_secs_f64() * 1000.
}

/// 5,000 threads over 40 projects: scroll and selection frame times.
fn run_bench(check: &mut Check) -> anyhow::Result<()> {
    let snapshot = fixtures::large_snapshot(5000, 40);
    let start = Instant::now();
    let (app, root) = check.open_app(snapshot, TitlebarPlatform::Overlay, 1100., 780.)?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.;
    let sidebar = check.cx.update(|cx| root.read(cx).layout().read(cx).sidebar().clone());
    let rows = check.cx.update(|cx| sidebar.read(cx).list().entries.len());
    writeln!(check.report, "{} list entries; first open and two frames: {open_ms:.1} ms\n", rows)?;

    let list_point = point(px(128.), px(400.));
    let mut scroll = Vec::new();
    for step in 0..400 {
        let delta = if step < 300 { -120. } else { 120. };
        let t = check.with_window(app, |w, cx| {
            w.dispatch_event(
                ScrollEvent {
                    position: list_point,
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    source: ScrollSource::Pointer,
                }
                .to_platform_input(),
                cx,
            );
            let t = Instant::now();
            w.render_frame(cx);
            t.elapsed()
        })?;
        scroll.push(t);
    }
    let mut hover = Vec::new();
    let mut select = Vec::new();
    for i in 0..200 {
        let y = px(100. + (i % 8) as f32 * 80.);
        let at = point(px(128.), y);
        hover.push(check.with_window(app, |w, cx| {
            w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
            let t = Instant::now();
            w.render_frame(cx);
            t.elapsed()
        })?);
        select.push(check.with_window(app, |w, cx| {
            w.dispatch_event(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(), cx);
            w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
            let t = Instant::now();
            w.render_frame(cx);
            t.elapsed()
        })?);
    }
    writeln!(check.report, "| interaction | frames | p50 ms | p95 ms | max ms |\n|---|---|---|---|---|")?;
    for (name, samples) in [("scroll 120px", &mut scroll), ("hover move", &mut hover), ("click select", &mut select)] {
        let n = samples.len();
        let max = samples.iter().max().copied().unwrap_or_default().as_secs_f64() * 1000.;
        writeln!(check.report, "| {name} | {n} | {:.2} | {:.2} | {max:.2} |", percentile(samples, 0.5), percentile(samples, 0.95))?;
    }
    check.shot(app, "bench-5000.png")?;
    Ok(())
}
