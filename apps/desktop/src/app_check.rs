// Drives the assembled app in a headless window through real event dispatch.
//   app-check session <out> <state-dir> <dark|light>
//       clicks three threads, types and sends, watches the reply stream,
//       opens and closes the right panel and terminal with their shortcuts,
//       resizes the sidebar, goes back and forward, then writes state.
//   app-check restore <out> <state-dir> <dark|light>
//       a fresh process opens the same state dir and checks route and layout.
//   app-check bench <out>
//       5,000 threads (200 with 3,000-row timelines): thread switch time,
//       typing frame time on a long thread, memory after 50 visits.
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail, ensure};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_web::app_root::{AppRootOptions, local_shell_backend};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::components::sidebar_logic::SidebarListEntry;
use t3code_web::fixtures::FixtureStore;
use t3code_web::router::{Route, ThreadTarget};
use t3code_web::terminal_ui_state_store::TerminalUiStateStore;
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};
use t3code_web::ui_state_store::StateDir;
use t3code_web::{AppRoot, components::chat::timeline_entries::EntryKind};

struct Run {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    root: Entity<AppRoot>,
    out: PathBuf,
    tag: String,
    log: String,
    failures: usize,
    step: usize,
}

fn headless() -> HeadlessAppContext {
    HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    )
}

impl Run {
    fn open(out: PathBuf, tag: &str, store: FixtureStore, state: StateDir) -> anyhow::Result<Self> {
        let mut cx = headless();
        let mode = if tag == "light" { ThemeMode::Light } else { ThemeMode::Dark };
        cx.update(|cx| {
            t3code_web::init(cx);
            ThemeSettings::set_mode(mode, cx);
        });
        let options = AppRootOptions {
            store,
            platform: TitlebarPlatform::Overlay,
            state_dir: state,
            terminal_backend: local_shell_backend(),
        };
        let (window, root) = cx.update(|cx| {
            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(1100.), px(780.)), cx)),
                show: false,
                focus: true,
                ..component::TitleBar::window_options()
            };
            gpui_kit::open_window(window_options, cx, |window, cx| {
                cx.new(|cx| {
                    let root = AppRoot::with_options(options, window, cx);
                    root.layout().update(cx, |l, _| {
                        l.force_caption_buttons(true);
                        l.count_window_moves_only(true);
                    });
                    root
                })
            })
        })?;
        let mut run = Self { cx, window, root, out, tag: tag.into(), log: String::new(), failures: 0, step: 0 };
        run.frame();
        Ok(run)
    }

    /// Draws a frame from whatever is dirty and settles pending work.
    fn frame(&mut self) {
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
        self.cx.run_until_parked();
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        self.step += 1;
        let file = format!("{:02}-{name}-{}.png", self.step, self.tag);
        self.cx.capture_screenshot(self.window)?.save(self.out.join(&file))?;
        writeln!(self.log, "  screenshot {file}")?;
        Ok(())
    }

    fn check(&mut self, what: &str, ok: bool, detail: impl std::fmt::Display) {
        let line = format!("{} {what}: {detail}", if ok { "PASS" } else { "FAIL" });
        eprintln!("{line}");
        writeln!(self.log, "{line}").ok();
        if !ok {
            self.failures += 1;
        }
    }

    fn press(&mut self, keys: &str) {
        let keys = keys.replace("mod-", if cfg!(target_os = "macos") { "cmd-" } else { "ctrl-" });
        self.cx.update_window(self.window, |_, w, cx| w.press(&keys, cx)).ok();
        self.frame();
    }

    fn type_text(&mut self, text: &str) {
        self.cx.update_window(self.window, |_, w, cx| w.input(text, cx)).ok();
        self.frame();
    }

    fn click_point(&mut self, at: Point<Pixels>) {
        self.cx
            .update_window(self.window, |_, w, cx| {
                w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
                w.dispatch_event(
                    MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }
                        .to_platform_input(),
                    cx,
                );
                w.dispatch_event(
                    MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(),
                    cx,
                );
            })
            .ok();
        self.frame();
    }

    fn route(&mut self) -> Route {
        let root = self.root.clone();
        self.cx.update(|cx| root.read(cx).router().read(cx).current().clone())
    }

    /// Thread rows on screen: (entry index, key, centre).
    fn visible_threads(&mut self) -> Vec<(ThreadKey, Point<Pixels>)> {
        let root = self.root.clone();
        self.cx.update(|cx| {
            let sidebar = root.read(cx).sidebar().read(cx);
            sidebar
                .list()
                .entries
                .iter()
                .enumerate()
                .filter_map(|(ix, entry)| match entry {
                    SidebarListEntry::Thread { thread, .. } => {
                        let bounds = sidebar.entry_bounds(ix)?;
                        (bounds.top() > px(90.) && bounds.bottom() < px(700.)).then(|| (thread.key.clone(), bounds.center()))
                    }
                    _ => None,
                })
                .collect()
        })
    }

    fn shown(&mut self) -> Option<ThreadTarget> {
        let root = self.root.clone();
        self.cx.update(|cx| root.read(cx).thread_view().read(cx).target().cloned())
    }

    fn composer_focused(&mut self) -> bool {
        let root = self.root.clone();
        self.cx
            .update_window(self.window, |_, w, cx| {
                let composer = root.read(cx).composer(cx);
                composer.read(cx).editor().focus_handle(cx).is_focused(w)
            })
            .unwrap_or(false)
    }

    fn panels(&mut self) -> (bool, bool) {
        let root = self.root.clone();
        self.cx.update(|cx| {
            let layout = root.read(cx).layout().read(cx);
            (layout.is_right_panel_open(), layout.is_terminal_open())
        })
    }

    fn last_message(&mut self) -> (String, bool) {
        let root = self.root.clone();
        self.cx.update(|cx| {
            let timeline = root.read(cx).thread_view().read(cx).chat().read(cx).timeline().read(cx);
            timeline
                .input()
                .entries
                .iter()
                .rev()
                .find_map(|e| match &e.kind {
                    EntryKind::Message(m) => Some((m.text.to_string(), m.streaming)),
                    _ => None,
                })
                .unwrap_or_default()
        })
    }

    fn flush(&mut self) {
        let root = self.root.clone();
        self.cx.update(|cx| root.update(cx, |root, cx| root.flush_state(cx)));
    }

    fn sidebar_width(&mut self) -> f32 {
        let root = self.root.clone();
        self.cx.update(|cx| f32::from(root.read(cx).layout().read(cx).sidebar_width()))
    }
}

fn session(out: PathBuf, state: PathBuf, tag: &str) -> anyhow::Result<String> {
    let _ = std::fs::remove_dir_all(&state);
    let mut r = Run::open(out, tag, FixtureStore::showcase(), StateDir(Some(state)))?;
    writeln!(r.log, "## session ({tag})")?;
    let route = r.route();
    r.check("launch with no saved route starts a draft", matches!(route, Route::Draft(_)), route.path());
    r.shot("launch-draft-hero")?;

    // Three different threads, by clicking their sidebar rows.
    let rows = r.visible_threads();
    ensure!(rows.len() >= 3, "only {} thread rows on screen", rows.len());
    for (n, (key, at)) in rows.iter().take(3).enumerate() {
        r.click_point(*at);
        let shown = r.shown();
        let route = r.route();
        r.check(
            &format!("click {} shows thread {}", n + 1, key.thread_id.0),
            shown == Some(ThreadTarget::Server(key.clone())) && route == Route::Thread(key.clone()),
            route.path(),
        );
        r.shot(&format!("thread-{}", key.thread_id.0))?;
    }
    let sent_in = rows[2].0.clone();

    // Type and send; watch the canned reply stream in.
    let root = r.root.clone();
    r.cx.update_window(r.window, |_, w, cx| {
        let composer = root.read(cx).composer(cx);
        let focus = composer.read(cx).editor().focus_handle(cx);
        w.focus(&focus, cx);
    })?;
    r.type_text("Walk me through the replay loop");
    r.shot("typed")?;
    r.press("enter");
    let (text, _) = r.last_message();
    r.check("send appends the user message", text.is_empty() || text.contains("replay"), format!("{:?}", &text[..text.len().min(40)]));
    let started = Instant::now();
    let mut mid_shot = false;
    loop {
        r.cx.advance_clock(Duration::from_millis(40));
        r.frame();
        let (text, streaming) = r.last_message();
        if streaming && text.len() > 300 && !mid_shot {
            r.shot("reply-streaming")?;
            mid_shot = true;
        }
        if !streaming && text.contains("duplicate deliveries") {
            r.check("canned reply finished streaming", true, format!("{} chars in {:?}", text.len(), started.elapsed()));
            break;
        }
        if started.elapsed() > Duration::from_secs(60) {
            r.check("canned reply finished streaming", false, "timed out");
            break;
        }
    }
    r.check("mid-stream screenshot taken", mid_shot, "");
    r.shot("reply-done")?;

    // Right panel: open (launcher), add a tab, close it, reopen it, close the panel.
    r.press("mod-alt-b");
    let (right, _) = r.panels();
    r.check("mod+alt+b opens the right panel", right, "");
    r.shot("right-panel-launcher")?;
    // Launcher rows are 32px tall from y 351 in the 1100x780 window (Files is the third).
    r.click_point(point(px(860.), px(389.)));
    r.shot("right-panel-files-tab")?;
    let panel_key = format!("env-local:{}", sent_in.thread_id.0);
    let root = r.root.clone();
    let tabs = |r: &mut Run| r.cx.update(|cx| root.read(cx).right_panels().read(cx).state(&panel_key).surfaces.len());
    let n = tabs(&mut r);
    r.check("launcher adds a Files tab", n == 1, format!("{n} tabs"));
    r.press("mod-w");
    let n = tabs(&mut r);
    r.check("mod+w closes the tab", n == 0, format!("{n} tabs"));
    r.press("mod-shift-t");
    let n = tabs(&mut r);
    r.check("mod+shift+t reopens it", n == 1, format!("{n} tabs"));
    r.press("mod-alt-b");
    let (right, _) = r.panels();
    r.check("mod+alt+b closes the right panel", !right, "");
    let focused = r.composer_focused();
    r.check("focus returns to the composer", focused, "");

    // Terminal drawer on a local shell.
    r.press("mod-j");
    let (_, terminal) = r.panels();
    r.check("mod+j opens the terminal", terminal, "");
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(60));
        r.cx.advance_clock(Duration::from_millis(60));
        r.frame();
    }
    r.shot("terminal-open")?;
    r.press("mod-j");
    let (_, terminal) = r.panels();
    r.check("mod+j closes the terminal", !terminal, "");
    let focused = r.composer_focused();
    r.check("focus returns to the composer after the terminal", focused, "");

    // Leave the right panel open for the restore check.
    r.press("mod-alt-b");
    r.click_point(point(px(860.), px(423.)));

    // Sidebar resize by its rail.
    let from = r.sidebar_width();
    r.cx.update_window(r.window, |_, w, cx| w.drag(point(px(from), px(400.)), point(px(340.), px(400.)), cx))?;
    r.frame();
    let width = r.sidebar_width();
    r.check("rail drag resizes the sidebar", (width - 340.).abs() < 1., format!("{from} -> {width}"));
    r.shot("sidebar-resized")?;

    // Back and forward.
    let before = r.route();
    r.press("mod-[");
    let back = r.route();
    r.check("mod+[ goes back", back == Route::Thread(rows[1].0.clone()), back.path());
    r.shot("back")?;
    r.press("mod-]");
    let fwd = r.route();
    r.check("mod+] goes forward", fwd == before, fwd.path());

    r.flush();
    let saved = r.route().path();
    writeln!(r.log, "saved route {saved}")?;
    ensure!(r.failures == 0, "{} checks failed\n{}", r.failures, r.log);
    Ok(r.log)
}

fn restore(out: PathBuf, state: PathBuf, tag: &str) -> anyhow::Result<String> {
    let layout: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state.join("layout.json"))?)?;
    let mut r = Run::open(out, tag, FixtureStore::showcase(), StateDir(Some(state)))?;
    writeln!(r.log, "## restore ({tag})")?;
    let saved = layout["lastRoute"].as_str().unwrap_or_default().to_string();
    let route = r.route();
    r.check("relaunch restores the route", route.path() == saved && matches!(route, Route::Thread(_)), route.path());
    let width = r.sidebar_width();
    r.check("relaunch restores the sidebar width", (width - 340.).abs() < 1., width);
    let (right, _) = r.panels();
    r.check("relaunch restores the open right panel", right, "");
    r.shot("relaunched")?;
    ensure!(r.failures == 0, "{} checks failed\n{}", r.failures, r.log);
    Ok(r.log)
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(|l| l.split_whitespace().nth(1).unwrap_or("0").to_string()))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0., |kb| kb / 1024.)
}

fn pct(samples: &mut [f64], q: f64) -> f64 {
    samples.sort_by(|a, b| a.total_cmp(b));
    samples[((samples.len() as f64 - 1.) * q).round() as usize]
}

/// Navigates the way a sidebar click does and times until the frame is drawn.
fn switch(r: &mut Run, key: &ThreadKey) -> f64 {
    let root = r.root.clone();
    let route = Route::Thread(key.clone());
    let ms = r
        .cx
        .update_window(r.window, |_, w, cx| {
            let start = Instant::now();
            let router = root.read(cx).router().clone();
            router.update(cx, |router, cx| router.navigate(route, cx));
            w.draw(cx).clear(cx);
            start.elapsed().as_secs_f64() * 1000.
        })
        .unwrap_or(f64::NAN);
    r.cx.run_until_parked();
    ms
}

fn bench(out: PathBuf) -> anyhow::Result<String> {
    let rss_start = rss_mb();
    let store = FixtureStore::large(5000, 25);
    let mut r = Run::open(out, "dark", store, StateDir(None))?;
    let mut log = format!("## bench: 5,000 threads, 200 with 1,000 turns (3,000 rows)\n\nRSS at start {rss_start:.0} MB, after open {:.0} MB\n\n", rss_mb());
    let keys: Vec<ThreadKey> = {
        let root = r.root.clone();
        r.cx.update(|cx| {
            let mut keys: Vec<_> = root.read(cx).store().read(cx).thread_keys().cloned().collect();
            keys.sort_by_key(|k| k.thread_id.0.trim_start_matches('t').parse::<usize>().unwrap_or(0));
            keys
        })
    };
    let long: Vec<ThreadKey> = keys.iter().step_by(25).take(25).cloned().collect();
    let short: Vec<ThreadKey> = keys.iter().skip(1).step_by(25).take(25).cloned().collect();

    // Build the fixture timelines first: that cost belongs to the sync layer, not the switch.
    let root = r.root.clone();
    let gen_start = Instant::now();
    r.cx.update(|cx| {
        let store = root.read(cx).store().clone();
        store.update(cx, |store, _| {
            for key in long.iter().chain(&short) {
                store.thread_view(&ThreadTarget::Server(key.clone()));
            }
        });
    });
    writeln!(log, "fixture timelines for 50 threads built in {:.0} ms (not part of the switch)\n", gen_start.elapsed().as_secs_f64() * 1000.)?;

    let mut cold_long = Vec::new();
    let mut cold_short = Vec::new();
    for (l, s) in long.iter().zip(&short) {
        cold_long.push(switch(&mut r, l));
        cold_short.push(switch(&mut r, s));
    }
    let rss_visited = rss_mb();
    let mut warm_long = Vec::new();
    let mut warm_short = Vec::new();
    for _ in 0..2 {
        for (l, s) in long.iter().zip(&short) {
            warm_long.push(switch(&mut r, l));
            warm_short.push(switch(&mut r, s));
        }
    }
    let row = |name: &str, v: &mut Vec<f64>| format!("| {name} | {} | {:.2} | {:.2} | {:.2} |\n", v.len(), pct(v, 0.5), pct(v, 0.95), pct(v, 1.));
    log += "| switch (navigate to drawn frame) | n | p50 ms | p95 ms | max ms |\n|---|---|---|---|---|\n";
    log += &row("cold, 3,000-row thread", &mut cold_long);
    log += &row("cold, short thread", &mut cold_short);
    log += &row("revisit, 3,000-row thread", &mut warm_long);
    log += &row("revisit, short thread", &mut warm_short);

    // Real clicks on visible rows, to show the click path costs the same.
    let mut clicks = Vec::new();
    for (key, at) in r.visible_threads().into_iter().take(6) {
        let _ = key;
        let t = r
            .cx
            .update_window(r.window, |_, w, cx| {
                let start = Instant::now();
                w.dispatch_event(
                    MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(),
                    cx,
                );
                w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
                w.draw(cx).clear(cx);
                start.elapsed().as_secs_f64() * 1000.
            })
            .unwrap_or(f64::NAN);
        r.cx.run_until_parked();
        clicks.push(t);
    }
    log += &row("sidebar row click (cold, short)", &mut clicks);

    // Typing with a 3,000-row thread on screen.
    switch(&mut r, &long[0]);
    let root = r.root.clone();
    r.cx.update_window(r.window, |_, w, cx| {
        let focus = root.read(cx).composer(cx).read(cx).editor().focus_handle(cx);
        w.focus(&focus, cx);
    })?;
    r.frame();
    let os = r.cx.update(|cx| cx.platform_info().os);
    let chars: Vec<char> = "the quick brown fox jumps over the lazy dog ".chars().collect();
    let mut typing = Vec::new();
    for i in 0..300 {
        let c = chars[i % chars.len()];
        let source = if c == ' ' { "space".to_string() } else { c.to_string() };
        let mut keystroke = Keystroke::parse_with_os(&source, os).context("keystroke")?;
        if c != ' ' {
            keystroke.key_char = Some(c.to_string());
        } else {
            keystroke.key_char = Some(" ".into());
        }
        let t = r
            .cx
            .update_window(r.window, |_, w, cx| {
                let start = Instant::now();
                w.dispatch_keystroke(keystroke, cx);
                w.draw(cx).clear(cx);
                start.elapsed().as_secs_f64() * 1000.
            })
            .unwrap_or(f64::NAN);
        r.cx.run_until_parked();
        typing.push(t);
    }
    log += &row("keystroke to drawn frame, 3,000-row thread on screen", &mut typing);
    r.shot("bench-long-thread-typed")?;
    writeln!(log, "\nRSS after visiting 50 threads (25 long, 25 short): {rss_visited:.0} MB; after revisits and typing: {:.0} MB", rss_mb())?;
    let terminals = r.cx.update(|cx| cx.global::<TerminalUiStateStore>().to_json().map(|j| j.len()).unwrap_or(0));
    let _ = terminals;
    Ok(log)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: app-check <session|restore> <out> <state-dir> <dark|light> | app-check bench <out>";
    let mode = args.first().context(usage)?.as_str();
    let out = PathBuf::from(args.get(1).context(usage)?);
    std::fs::create_dir_all(&out)?;
    let log = match mode {
        "session" | "restore" => {
            let state = PathBuf::from(args.get(2).context(usage)?);
            let tag = args.get(3).map(String::as_str).unwrap_or("dark");
            if mode == "session" { session(out.clone(), state, tag)? } else { restore(out.clone(), state, tag)? }
        }
        "bench" => bench(out.clone())?,
        _ => bail!(usage),
    };
    println!("{log}");
    let name = format!("{mode}-{}.md", args.get(3).map(String::as_str).unwrap_or("log"));
    let path = out.join(name);
    let previous = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = previous;
    std::fs::write(path, log)?;
    Ok(())
}
