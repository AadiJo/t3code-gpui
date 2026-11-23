// Drives the assembled app in a headless window through real event dispatch.
//   app-check session <out> <state-dir> <dark|light>
//       clicks three threads, types and sends, watches the reply stream,
//       opens and closes the right panel and terminal with their shortcuts,
//       resizes the sidebar, goes back and forward, then writes state.
//   app-check restore <out> <state-dir> <dark|light>
//       a fresh process opens the same state dir and checks route and layout.
//   app-check flow <out> <state-dir> <dark|light>
//       the merged window root end to end: palette jump, settings from the
//       sidebar gear and the palette, the notification inbox, panels by
//       shortcut and by the menu's actions, back and forward.
//   app-check first-run <out> <data-dir> <dark|light>
//       no fixtures and an empty data directory: the first-run empty state.
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
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, WindowRoot, local_shell_backend};
use t3code_web::connection::shell::EnvironmentShell;
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::components::command_palette::CommandPalette;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::components::sidebar::sidebar_notifications::SidebarNotifications;
use t3code_web::desktop_notifications::NotificationInbox;
use t3code_web::shortcuts::ShortcutRoot;
use t3code_web::shortcuts::commands::{NavigateBack, NavigateForward, ToggleRightPanel, ToggleSidebar, ToggleTerminal};
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
    shell: Entity<EnvironmentShell>,
    shortcuts: Entity<ShortcutRoot>,
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
    /// Opens the real window root (shortcut root, environment shell, app)
    /// over `store`, with the connection runtime on `data` (no T3 Connect).
    fn open(out: PathBuf, tag: &str, store: FixtureStore, state: StateDir, data: PathBuf) -> anyhow::Result<Self> {
        let mut cx = headless();
        let mode = if tag == "light" { ThemeMode::Light } else { ThemeMode::Dark };
        let config = RuntimeConfig {
            secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
            data_dir: data,
            cloud: None,
            presentation: ClientPresentation { label: "t3code-gpui app check".into(), ..Default::default() },
            electron: None,
        };
        cx.update(|cx| {
            t3code_web::init(cx);
            ThemeSettings::set_mode(mode, cx);
            ConnectionRuntime::init(config, cx)
        })?;
        let options = AppRootOptions {
            store,
            platform: TitlebarPlatform::Overlay,
            state_dir: state,
            terminal_backend: local_shell_backend(),
        };
        let (window, parts) = cx.update(|cx| {
            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(1100.), px(780.)), cx)),
                show: false,
                focus: true,
                ..component::TitleBar::window_options()
            };
            WindowRoot::open_window(window_options, options, cx)
        })?;
        let WindowRoot { shortcuts, shell, app: root } = parts;
        cx.update(|cx| {
            let layout = root.read(cx).layout().clone();
            layout.update(cx, |l, _| {
                l.force_caption_buttons(true);
                l.count_window_moves_only(true);
            })
        });
        let mut run = Self {
            cx,
            window,
            root,
            shell,
            shortcuts,
            out,
            tag: tag.into(),
            log: String::new(),
            failures: 0,
            step: 0,
        };
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

    fn click_id(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.frame();
        self.cx.update_window(self.window, |_, w, cx| w.click(id, cx)).ok();
        self.frame();
    }

    /// Dispatches `action` to the focused element, as a menu item does.
    fn menu(&mut self, action: impl Action) {
        self.cx.update_window(self.window, |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)).ok();
        self.frame();
    }

    fn palette(&mut self) -> Entity<CommandPalette> {
        let shortcuts = self.shortcuts.clone();
        self.cx.update(|cx| shortcuts.read(cx).palette().clone())
    }

    fn settings_path(&mut self) -> Option<SettingsPath> {
        let shell = self.shell.clone();
        self.cx.update(|cx| shell.read(cx).settings().map(|s| s.read(cx).path()))
    }

    fn thread_title(&mut self, key: &ThreadKey) -> String {
        let root = self.root.clone();
        let key = key.clone();
        self.cx.update(|cx| {
            root.read(cx).sidebar().read(cx).list().entries.iter().find_map(|entry| match entry {
                SidebarListEntry::Thread { thread, .. } if thread.key == key => Some(thread.title.to_string()),
                _ => None,
            })
        })
        .unwrap_or_default()
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
    let mut r = Run::open(out, tag, FixtureStore::showcase(), StateDir(Some(state.clone())), state.join("connections"))?;
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
    let root = r.root.clone();
    let sent = r.cx.update(|cx| {
        let timeline = root.read(cx).thread_view().read(cx).chat().read(cx).timeline().read(cx);
        timeline.input().entries.iter().any(|e| matches!(&e.kind, EntryKind::Message(m)
            if m.role == t3code_web::components::chat::timeline_entries::MessageRole::User && m.text.as_ref() == "Walk me through the replay loop"))
    });
    let composer_empty = r.cx.update(|cx| root.read(cx).composer(cx).read(cx).editor().read(cx).text().is_empty());
    r.check("send appends the user message and clears the composer", sent && composer_empty, format!("sent {sent}, composer empty {composer_empty}"));
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
    let mut r = Run::open(out, tag, FixtureStore::showcase(), StateDir(Some(state.clone())), state.join("connections"))?;
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


/// The merged root end to end, in one session.
fn flow(out: PathBuf, state: PathBuf, tag: &str) -> anyhow::Result<String> {
    let _ = std::fs::remove_dir_all(&state);
    let mut r = Run::open(out, tag, FixtureStore::showcase(), StateDir(Some(state.clone())), state.join("connections"))?;
    writeln!(r.log, "## flow ({tag})")?;
    let empty = r.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.is_empty());
    let (ok, detail) = (empty && matches!(r.route(), Route::Draft(_)), r.route().path());
    r.check("fixtures show with no saved environment", ok, detail);
    r.shot("launch-fixtures")?;

    // A thread by its sidebar row.
    let rows = r.visible_threads();
    ensure!(rows.len() >= 3, "only {} thread rows on screen", rows.len());
    let (first, at) = rows[0].clone();
    r.click_point(at);
    let (ok, detail) = (r.route() == Route::Thread(first.clone()), r.route().path());
    r.check("sidebar click shows the thread", ok, detail);
    r.shot("thread-clicked")?;

    // The palette from its shortcut, then a jump to another thread.
    r.press("mod-k");
    let palette = r.palette();
    let open = r.cx.update(|cx| palette.read(cx).is_open());
    r.check("mod+k opens the palette", open, "");
    let unavailable = r.cx.update(|cx| palette.read(cx).unavailable_titles());
    let shown = r.cx.update(|cx| palette.read(cx).shown_titles());
    let wanted = ["Sidebar: Toggle", "Terminal: Toggle", "Right Panel: Toggle", "Navigation: Back", "Chat: New"];
    let missing: Vec<_> = wanted.iter().filter(|w| unavailable.iter().any(|u| u.as_ref() == **w)).collect();
    r.check("app commands are available in the palette", missing.is_empty(), format!("unavailable: {unavailable:?}; shown {}", shown.len()));
    r.shot("palette-open")?;
    let target = rows[2].0.clone();
    let title = r.thread_title(&target);
    r.type_text(&title);
    r.shot("palette-typed")?;
    r.press("enter");
    let open = r.cx.update(|cx| palette.read(cx).is_open());
    let (ok, detail) = (!open && r.route() == Route::Thread(target.clone()), format!("{} via {title:?}", r.route().path()));
    r.check("palette jump opens the thread", ok, detail);
    r.shot("palette-jumped")?;
    let thread = r.route();

    // Settings from the sidebar gear, then the Environments panel.
    r.click_id("sidebar-settings");
    let path = r.settings_path();
    let (ok, detail) = (path == Some(SettingsPath::General) && r.route().is_modal(), r.route().path());
    r.check("the gear opens settings on General", ok, detail);
    r.shot("settings-from-gear")?;
    open_environments(&mut r);
    let path = r.settings_path();
    let (ok, detail) = (path == Some(SettingsPath::Connections), format!("{path:?} at {}", r.route().path()));
    r.check("the nav lands on Environments", ok, detail);
    r.shot("settings-environments-from-gear")?;
    r.click_id("settings-close");
    let (ok, detail) = (r.route() == thread && r.settings_path().is_none(), r.route().path());
    r.check("closing settings returns to the thread", ok, detail);
    r.shot("settings-closed")?;

    // Settings from the palette.
    r.press("mod-k");
    r.type_text("Open settings");
    r.press("enter");
    let path = r.settings_path();
    let (ok, detail) = (path == Some(SettingsPath::General), r.route().path());
    r.check("the palette opens settings", ok, detail);
    open_environments(&mut r);
    r.shot("settings-environments-from-palette")?;
    r.press("escape");
    let (ok, detail) = (r.route() == thread && r.settings_path().is_none(), r.route().path());
    r.check("escape closes settings back to the thread", ok, detail);

    // The notification inbox, and a thread opened from it.
    let items = r.cx.update(|cx| {
        let store = FixtureStore::showcase();
        let items = store.inbox_items();
        NotificationInbox::set_items(items.clone(), cx);
        items
    });
    let unread: Vec<_> = items.iter().filter(|item| !item.read).cloned().collect();
    ensure!(!unread.is_empty(), "no unread fixture notifications");
    r.click_id("sidebar-notifications");
    let root = r.root.clone();
    let open = r.cx.update(|cx| SidebarNotifications::view(root.read(cx).sidebar().downgrade(), cx).read(cx).is_open());
    r.check("the bell opens the inbox", open, format!("{} unread", unread.len()));
    r.shot("inbox-open")?;
    let pick = unread.iter().position(|item| Route::Thread(item.thread_key()) != thread).unwrap_or(0);
    r.click_id(("notification", pick));
    let want = Route::Thread(unread[pick].thread_key());
    let (ok, detail) = (r.route() == want, r.route().path());
    r.check("an inbox row opens its thread", ok, detail);
    r.shot("inbox-opened-thread")?;
    // A native notification click arrives as `InboxEvent::OpenThread`.
    r.cx.update(|cx| NotificationInbox::open_thread(thread_key(&thread), cx));
    r.frame();
    let (ok, detail) = (r.route() == thread, r.route().path());
    r.check("InboxEvent::OpenThread navigates", ok, detail);

    // Terminal and right panel by shortcut, then by the menu's actions.
    focus_composer(&mut r)?;
    r.press("mod-j");
    let (ok, detail) = (r.panels().1, "");
    r.check("mod+j opens the terminal", ok, detail);
    r.shot("terminal-shortcut")?;
    r.press("mod-j");
    let (ok, detail) = (!r.panels().1, "");
    r.check("mod+j closes the terminal", ok, detail);
    r.menu(ToggleTerminal);
    let (ok, detail) = (r.panels().1, "");
    r.check("View > Toggle terminal drawer opens it", ok, detail);
    r.shot("terminal-menu")?;
    r.menu(ToggleTerminal);
    let (ok, detail) = (!r.panels().1, "");
    r.check("and closes it", ok, detail);
    r.press("mod-alt-b");
    let (ok, detail) = (r.panels().0, "");
    r.check("mod+alt+b opens the right panel", ok, detail);
    r.shot("right-panel-shortcut")?;
    r.press("mod-alt-b");
    let (ok, detail) = (!r.panels().0, "");
    r.check("mod+alt+b closes it", ok, detail);
    r.menu(ToggleRightPanel);
    let (ok, detail) = (r.panels().0, "");
    r.check("View > Toggle right panel opens it", ok, detail);
    r.shot("right-panel-menu")?;
    r.menu(ToggleRightPanel);
    let (ok, detail) = (!r.panels().0, "");
    r.check("and closes it", ok, detail);
    r.menu(ToggleSidebar);
    let open = r.cx.update(|cx| root.read(cx).layout().read(cx).is_sidebar_open());
    r.check("View > Toggle Sidebar hides the sidebar", !open, "");
    r.shot("sidebar-hidden-menu")?;
    r.press("mod-b");
    let open = r.cx.update(|cx| root.read(cx).layout().read(cx).is_sidebar_open());
    r.check("mod+b shows it again", open, "");

    // Back and forward, by shortcut and by the menu.
    let here = r.route();
    r.press("mod-[");
    let back = r.route();
    r.check("mod+[ goes back", back == want, back.path());
    r.shot("back")?;
    r.press("mod-]");
    let (ok, detail) = (r.route() == here, r.route().path());
    r.check("mod+] goes forward", ok, detail);
    r.menu(NavigateBack);
    let (ok, detail) = (r.route() == want, r.route().path());
    r.check("View > Back goes back", ok, detail);
    r.menu(NavigateForward);
    let (ok, detail) = (r.route() == here, r.route().path());
    r.check("View > Forward goes forward", ok, detail);
    r.shot("forward")?;

    ensure!(r.failures == 0, "{} checks failed\n{}", r.failures, r.log);
    Ok(r.log)
}

/// Scrolls the settings nav to its Connections group, then opens Environments.
fn open_environments(r: &mut Run) {
    // Over the nav list, left side of the modal.
    let at = point(px(150.), px(400.));
    r.cx.update_window(r.window, |_, w, cx| {
        w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        w.dispatch_event(
            ScrollEvent {
                position: at,
                delta: ScrollDelta::Pixels(point(px(0.), px(-400.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            }
            .to_platform_input(),
            cx,
        );
    })
    .ok();
    r.frame();
    r.click_id("settings-nav-connections");
}

fn thread_key(route: &Route) -> ThreadKey {
    match route {
        Route::Thread(key) => key.clone(),
        _ => panic!("not a thread route: {}", route.path()),
    }
}

fn focus_composer(r: &mut Run) -> anyhow::Result<()> {
    let root = r.root.clone();
    r.cx.update_window(r.window, |_, w, cx| {
        let focus = root.read(cx).composer(cx).read(cx).editor().focus_handle(cx);
        w.focus(&focus, cx);
    })?;
    r.frame();
    Ok(())
}

/// No fixtures, an empty data directory: the first-run empty state.
fn first_run(out: PathBuf, data: PathBuf, tag: &str) -> anyhow::Result<String> {
    let _ = std::fs::remove_dir_all(&data);
    let options_store = FixtureStore::empty();
    let mut r = Run::open(out, tag, options_store, StateDir(Some(data.join("ui"))), data.join("connections"))?;
    writeln!(r.log, "## first run ({tag})")?;
    let root = r.root.clone();
    let (environments, has_data) = r.cx.update(|cx| {
        (ConnectionRuntime::state(cx).read(cx).environments.len(), root.read(cx).has_fixture_data(cx))
    });
    let empty_drawn = r.cx.update_window(r.window, |_, w, _| w.try_find("empty-add-pairing").is_some()).unwrap_or(false);
    r.check("no environments and no fixtures", environments == 0 && !has_data, format!("{environments} environments"));
    r.check("the first-run empty state is drawn", empty_drawn, "");
    r.shot("first-run-empty")?;
    r.click_id("empty-add-pairing");
    let path = r.settings_path();
    let (ok, detail) = (path == Some(SettingsPath::Connections), r.route().path());
    r.check("its button opens settings on Environments", ok, detail);
    r.shot("first-run-environments")?;
    r.click_id("settings-close");
    let (ok, detail) = (r.settings_path().is_none(), r.route().path());
    r.check("closing settings shows the empty state again", ok, detail);
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

/// Runs `act` in one update (its effects flush when that update ends, as a
/// real event's do), then draws the frame; returns the time for both.
fn timed(r: &mut Run, act: impl FnOnce(&mut Window, &mut App)) -> f64 {
    let start = Instant::now();
    r.cx.update_window(r.window, |_, w, cx| act(w, cx)).ok();
    r.cx.update_window(r.window, |_, w, cx| w.draw(cx).clear(cx)).ok();
    let ms = start.elapsed().as_secs_f64() * 1000.;
    r.cx.run_until_parked();
    ms
}

/// Navigates the way a sidebar click does and times until the frame is drawn.
fn switch(r: &mut Run, key: &ThreadKey) -> f64 {
    let router = r.root.read_with(&r.cx, |root, _| root.router().clone());
    let route = Route::Thread(key.clone());
    timed(r, move |_, cx| router.update(cx, |router, cx| router.navigate(route, cx)))
}

fn bench(out: PathBuf) -> anyhow::Result<String> {
    let rss_start = rss_mb();
    let store = FixtureStore::large(5000, 25);
    let data = std::env::temp_dir().join(format!("t3code-app-check-bench-{}", std::process::id()));
    let mut r = Run::open(out, "dark", store, StateDir(None), data)?;
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
    let rss_revisited = rss_mb();

    // Where a long-thread switch goes: store read, chat view update, then the draw.
    let (mut read, mut set, mut draw) = (Vec::new(), Vec::new(), Vec::new());
    for key in long.iter().take(10) {
        switch(&mut r, &short[0]);
        let root = r.root.clone();
        let target = ThreadTarget::Server(key.clone());
        let (a, b) = r.cx.update(|cx| {
            let store = root.read(cx).store().clone();
            let start = Instant::now();
            let view = store.update(cx, |store, _| store.thread_view(&target)).expect("thread");
            let a = start.elapsed().as_secs_f64() * 1000.;
            let chat = root.read(cx).thread_view().read(cx).chat().clone();
            let start = Instant::now();
            chat.update(cx, |chat, cx| chat.set_thread(view.key, view.project, view.title, view.timeline, cx));
            (a, start.elapsed().as_secs_f64() * 1000.)
        });
        let start = Instant::now();
        r.cx.update_window(r.window, |_, w, cx| w.draw(cx).clear(cx)).ok();
        draw.push(start.elapsed().as_secs_f64() * 1000.);
        r.cx.run_until_parked();
        read.push(a);
        set.push(b);
    }
    log += &row("breakdown: store read + timeline clone", &mut read);
    log += &row("breakdown: chat view set_thread (row derivation)", &mut set);
    log += &row("breakdown: draw", &mut draw);

    // A short-thread switch split into the route update (effects flushed) and the draw.
    let (mut apply, mut paint) = (Vec::new(), Vec::new());
    for key in short.iter().chain(&short) {
        let router = r.root.read_with(&r.cx, |root, _| root.router().clone());
        let route = Route::Thread(key.clone());
        let start = Instant::now();
        r.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
        apply.push(start.elapsed().as_secs_f64() * 1000.);
        let start = Instant::now();
        r.cx.update_window(r.window, |_, w, cx| w.draw(cx).clear(cx)).ok();
        paint.push(start.elapsed().as_secs_f64() * 1000.);
        r.cx.run_until_parked();
    }
    log += &row("short switch: route applied", &mut apply);
    log += &row("short switch: draw", &mut paint);

    // Real clicks on visible rows, to show the click path costs the same.
    let mut clicks = Vec::new();
    for (key, at) in r.visible_threads().into_iter().take(6) {
        let _ = key;
        let t = timed(&mut r, |w, cx| {
            w.dispatch_event(
                MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(),
                cx,
            );
            w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
        });
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
        let t = timed(&mut r, |w, cx| {
            w.dispatch_keystroke(keystroke, cx);
        });
        typing.push(t);
    }
    log += &row("keystroke to drawn frame, 3,000-row thread on screen", &mut typing);
    r.shot("bench-long-thread-typed")?;
    writeln!(log, "\nRSS after visiting 50 threads (25 long, 25 short): {rss_visited:.0} MB; after two revisit rounds: {rss_revisited:.0} MB; after typing 300 keys: {:.0} MB", rss_mb())?;
    let terminals = r.cx.update(|cx| cx.global::<TerminalUiStateStore>().to_json().map(|j| j.len()).unwrap_or(0));
    let _ = terminals;
    Ok(log)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: app-check <session|restore|flow|first-run> <out> <state-dir> <dark|light> | app-check bench <out>";
    let mode = args.first().context(usage)?.as_str();
    let out = PathBuf::from(args.get(1).context(usage)?);
    std::fs::create_dir_all(&out)?;
    let log = match mode {
        "session" | "restore" | "flow" | "first-run" => {
            let state = PathBuf::from(args.get(2).context(usage)?);
            let tag = args.get(3).map(String::as_str).unwrap_or("dark");
            match mode {
                "session" => session(out.clone(), state, tag)?,
                "restore" => restore(out.clone(), state, tag)?,
                "flow" => flow(out.clone(), state, tag)?,
                _ => first_run(out.clone(), state, tag)?,
            }
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
