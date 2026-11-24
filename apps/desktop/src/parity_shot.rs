// Renders the visual-parity scenes: the real window root on the reference
// fixtures (`FixtureStore::reference`), with the owner's settings (macOS
// titlebar, Graphite, glass opacity 45%), driven into each reference view by
// real input events, then captured at 1100x780 @2x.
// Usage: parity-shot <out-dir> [scene-filter]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, WindowRoot, local_shell_backend, local_script_backend};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::sidebar::Sidebar;
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::desktop_notifications::{DesktopNotification, DesktopNotificationKind, NotificationInbox};
use t3code_web::fixtures::FixtureStore;
use t3code_web::router::Route;
use t3code_web::shortcuts::commands::{ToggleCommandPalette, ToggleDiff, ToggleTerminal};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};
use t3code_web::theme_tokens::GlassOpacity;
use t3code_web::ui_state_store::StateDir;

/// The owner's Glass opacity setting.
const OWNER_GLASS_OPACITY: u8 = 45;
const THREAD: &str = "/env-local/r-sync";

/// One input or wait applied before the capture.
enum Step {
    /// Mouse move without buttons (hover).
    Move(f32, f32),
    Click(f32, f32),
    RightClick(f32, f32),
    /// Clicks an element by id.
    ClickId(&'static str),
    /// Scroll wheel at a point, in pixels (positive scrolls content down, toward the top).
    Wheel(f32, f32, f32),
    Type(&'static str),
    /// A keystroke, `mod-` meaning cmd on macOS and ctrl elsewhere.
    Press(&'static str),
    Action(fn() -> Box<dyn Action>),
    Navigate(&'static str),
    /// Lets background work (a shell) run before the next step.
    Wait(u64),
    /// Puts the sidebar in a state no single input reaches (shelves open).
    Sidebar(fn(&mut Sidebar, &mut Window, &mut Context<Sidebar>)),
    /// Holds a modifier (`cmd` on macOS, `ctrl` elsewhere) without a key.
    HoldModifier,
    /// Presses the left button at a point and drags it to another in ten moves, without releasing.
    Drag(f32, f32, f32, f32),
}

/// A reference view and how to reach it.
struct Scene {
    /// The port image name; the reference is `ref/<reference>.png`.
    name: &'static str,
    reference: &'static str,
    light: bool,
    steps: &'static [Step],
}

const TERMINAL_STEPS: &[Step] = &[
    Step::Action(|| Box::new(ToggleTerminal)),
    Step::Wait(1500),
    Step::Type("exec env PS1='scenario@host:t3code$ ' bash --norc --noprofile\n"),
    Step::Wait(800),
    Step::Type("clear; printf ' \\033[31mM\\033[0m src/math.ts\\n\\033[31m??\\033[0m src/retry.ts\\nmath.ts  retry.ts\\n'\n"),
    Step::Wait(1200),
];

const SCENES: &[Scene] = &[
    Scene { name: "window-thread", reference: "window-thread", light: false, steps: &[] },
    Scene { name: "window-thread-light", reference: "window-thread-light", light: true, steps: &[] },
    Scene { name: "sidebar-selected", reference: "sidebar-selected", light: false, steps: &[] },
    Scene { name: "sidebar-hover", reference: "sidebar-hover", light: false, steps: &[Step::Move(127.5, 224.), Step::Wait(300)] },
    Scene { name: "sidebar-context-menu", reference: "sidebar-context-menu", light: false, steps: &[Step::RightClick(127.5, 224.)] },
    Scene { name: "sidebar-inbox", reference: "sidebar-inbox", light: false, steps: &[Step::ClickId("sidebar-notifications")] },
    Scene { name: "timeline-top", reference: "timeline-top", light: false, steps: &[Step::Wheel(700., 400., 20_000.)] },
    Scene {
        name: "timeline-tools-expanded",
        reference: "timeline-tools-expanded",
        light: false,
        steps: &[Step::Wheel(700., 400., 20_000.), Step::Click(340., 142.)],
    },
    Scene { name: "timeline-bottom", reference: "timeline-bottom", light: false, steps: &[] },
    Scene { name: "timeline-user-message", reference: "timeline-user-message", light: false, steps: &[Step::Wheel(700., 400., 600.)] },
    Scene { name: "composer-empty", reference: "composer-empty", light: false, steps: &[] },
    Scene { name: "composer-focused", reference: "composer-focused", light: false, steps: &[Step::Click(700., 690.)] },
    Scene {
        name: "composer-multiline",
        reference: "composer-multiline",
        light: false,
        steps: &[
            Step::Click(700., 690.),
            Step::Type("First line of a longer prompt"),
            Step::Press("shift-enter"),
            Step::Type("Second line with `inline code`"),
            Step::Press("shift-enter"),
            Step::Type("Third line"),
            Step::Press("shift-enter"),
            Step::Type("Fourth line"),
        ],
    },
    Scene {
        name: "composer-markdown",
        reference: "composer-markdown",
        light: false,
        steps: &[
            Step::Click(700., 690.),
            Step::Type("Use `inline code` and **bold** here"),
            Step::Press("shift-enter"),
            Step::Type("```ts"),
            Step::Press("shift-enter"),
            Step::Type("const x = sum(1, 2);"),
            // The capture typed a closing fence, which the web keeps as code; the
            // port shows its closing fence while the caret is on it.
            Step::Press("shift-enter"),
            Step::Type("```"),
        ],
    },
    Scene { name: "thread-actions-menu", reference: "thread-actions-menu", light: false, steps: &[Step::Click(430., 26.)] },
    Scene { name: "composer-model-picker", reference: "composer-model-picker", light: false, steps: &[Step::Click(960., 733.)] },
    Scene { name: "composer-mention", reference: "composer-mention", light: false, steps: &[Step::Click(700., 690.), Step::Type("@"), Step::Wait(300)] },
    Scene { name: "command-palette", reference: "command-palette", light: false, steps: &[Step::Action(|| Box::new(ToggleCommandPalette))] },
    Scene { name: "settings-environments", reference: "settings-environments", light: false, steps: &[Step::Navigate("/settings/connections")] },
    Scene { name: "settings-general", reference: "settings-general", light: false, steps: &[Step::Navigate("/settings/general")] },
    Scene { name: "settings-appearance", reference: "settings-appearance", light: false, steps: &[Step::Navigate("/settings/appearance")] },
    Scene { name: "settings-chat", reference: "settings-chat", light: false, steps: &[Step::Navigate("/settings/chat")] },
    Scene { name: "settings-diffs", reference: "settings-diffs", light: false, steps: &[Step::Navigate("/settings/diffs")] },
    Scene { name: "settings-keybindings", reference: "settings-keybindings", light: false, steps: &[Step::Navigate("/settings/keybindings")] },
    Scene { name: "settings-voice", reference: "settings-voice", light: false, steps: &[Step::Navigate("/settings/voice")] },
    Scene { name: "settings-snap-shot", reference: "settings-snap-shot", light: false, steps: &[Step::Navigate("/settings/snap-shot")] },
    Scene { name: "settings-labs", reference: "settings-labs", light: false, steps: &[Step::Navigate("/settings/labs")] },
    Scene { name: "settings-archived", reference: "settings-archived", light: false, steps: &[Step::Navigate("/settings/archived")] },
    Scene { name: "settings-about", reference: "settings-about", light: false, steps: &[Step::Navigate("/settings/about")] },
    Scene { name: "terminal-drawer", reference: "terminal-drawer", light: false, steps: TERMINAL_STEPS },
    Scene { name: "terminal-menu", reference: "terminal-menu", light: false, steps: &[Step::Action(|| Box::new(ToggleTerminal)), Step::Wait(1500), Step::Press("mod-n"), Step::Wait(1200), Step::RightClick(400.5, 626.)] },
    Scene { name: "terminal-close-confirm", reference: "terminal-close-confirm", light: false, steps: &[Step::Action(|| Box::new(ToggleTerminal)), Step::Wait(1500), Step::Press("mod-n"), Step::Wait(1200), Step::Click(1088., 512.5)] },
    Scene { name: "right-panel-diff", reference: "right-panel-diff", light: false, steps: &[Step::Action(|| Box::new(ToggleDiff)), Step::Wait(300)] },
];

/// The owner-configuration sidebar views (`artifacts/fixchat2/cap`): the
/// owner's client settings (`T3CODE_CLIENT_SETTINGS`, a path to their
/// client-settings JSON; never in the repo) on `sidebar_fixtures::owner_snapshot`.
const OWNER_THREAD: &str = "/env-local/o-sidebar";
fn expand_shelves(sidebar: &mut Sidebar, _: &mut Window, cx: &mut Context<Sidebar>) {
    sidebar.options_mut(cx, |o| {
        o.working_expanded = true;
        o.snoozed_expanded = true;
        o.settled_expanded = true;
    });
}
fn rename_tls(sidebar: &mut Sidebar, window: &mut Window, cx: &mut Context<Sidebar>) {
    let key = sidebar.snapshot().threads.iter().find(|t| t.title.starts_with("Rotate relay TLS")).map(|t| t.key.clone());
    if let Some(key) = key {
        sidebar.start_rename(key, window, cx);
    }
}
const OWNER_SCENES: &[Scene] = &[
    Scene { name: "owner-sb-top", reference: "sb-top", light: false, steps: &[] },
    Scene { name: "owner-sb-shelves-collapsed", reference: "sb-shelves-collapsed", light: false, steps: &[Step::Wheel(127., 400., -20_000.)] },
    Scene { name: "owner-sb-shelves-expanded", reference: "sb-shelves-expanded", light: false, steps: &[Step::Sidebar(expand_shelves), Step::Wheel(127., 400., -20_000.), Step::Move(127., 715.)] },
    Scene { name: "owner-sb-thread-menu", reference: "sb-thread-menu", light: false, steps: &[Step::RightClick(127., 520.)] },
    Scene { name: "owner-sb-draft-menu", reference: "sb-draft-menu", light: false, steps: &[Step::RightClick(127., 213.)] },
    Scene { name: "owner-sb-draft-hover", reference: "sb-draft-hover", light: false, steps: &[Step::Move(127.5, 213.), Step::Wait(100)] },
    Scene { name: "owner-sb-rename", reference: "sb-rename", light: false, steps: &[Step::Move(127., 540.), Step::Sidebar(rename_tls), Step::Press("end"), Step::Type(" now")] },
    Scene { name: "owner-sb-jump-hints", reference: "sb-jump-hints", light: false, steps: &[Step::HoldModifier, Step::Wait(1200)] },
    Scene { name: "owner-sb-drag", reference: "sb-drag", light: false, steps: &[Step::Drag(107.5, 521.5, 107.5, 381.5), Step::Wait(300)] },
    Scene { name: "owner-sb-drag-down", reference: "sb-drag-down", light: false, steps: &[Step::Drag(107.5, 521.5, 107.5, 381.5), Step::Drag(107.5, 381.5, 107.5, 779.), Step::Wait(300)] },
    Scene { name: "owner-sb-filter", reference: "sb-filter", light: false, steps: &[Step::Click(182., 74.5)] },
];

/// The reference inbox: the approval and the question, both unread.
fn reference_inbox() -> Arc<[DesktopNotification]> {
    let item = |thread: &str, title: &str, project: &str, kind| DesktopNotification {
        id: format!("parity-{thread}").into(),
        environment_id: "env-local".into(),
        thread_id: thread.to_string().into(),
        kind,
        thread_title: title.to_string().into(),
        project_title: project.to_string().into(),
        environment_title: "Scenario".into(),
        created_at: "2026-11-23T14:58:00.000Z".into(),
        read: false,
        resolved: false,
    };
    Arc::from([
        item("r-tls", "Rotate relay TLS certificates", "relay", DesktopNotificationKind::Approval),
        item("r-launch", "Draft launch copy for the changelog", "marketing-site", DesktopNotificationKind::Input),
    ])
}

fn headless() -> HeadlessAppContext {
    HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    )
}

fn frame(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, w, cx| w.render_frame(cx)).ok();
    cx.run_until_parked();
    cx.update_window(window, |_, w, cx| w.render_frame(cx)).ok();
}

fn mouse(cx: &mut HeadlessAppContext, window: AnyWindowHandle, at: Point<Pixels>, button: Option<MouseButton>) {
    cx.update_window(window, |_, w, cx| {
        w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        if let Some(button) = button {
            w.dispatch_event(
                MouseDownEvent { button, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(),
                cx,
            );
            w.dispatch_event(MouseUpEvent { button, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
        }
    })
    .ok();
}

/// Opens a fresh window on the reference world, applies `scene`'s steps and saves the frame.
fn render(scene: &Scene, owner: bool, out: &std::path::Path, data: &std::path::Path) -> anyhow::Result<()> {
    let mut cx = headless();
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
        data_dir: data.to_path_buf(),
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui parity".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(if scene.light { ThemeMode::Light } else { ThemeMode::Dark }, cx);
        if owner {
            t3code_web::scenes::settings::apply_client_settings_override(cx);
            // The capture server's relay-ops project, kept separate by a per-project override.
            t3code_web::hooks::use_settings::ClientSettingsStore::update(cx, |s| {
                s.sidebar_project_grouping_overrides =
                    [("env-local:/work/relay-ops".to_string(), t3code_contracts::SidebarProjectGroupingMode::Separate)].into();
            });
            // The owner's Mac is in dark appearance; headless has no system appearance.
            t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(ThemeMode::Dark, cx);
        } else {
            ThemeSettings::set_glass_opacity(GlassOpacity::new(OWNER_GLASS_OPACITY), cx);
        }
        ConnectionRuntime::init(config, cx)
    })?;
    let options = AppRootOptions {
        store: if owner {
            FixtureStore::from_sidebar(t3code_web::components::sidebar::fixtures::owner_snapshot())
        } else {
            FixtureStore::reference()
        },
        platform: TitlebarPlatform::Mac,
        state_dir: StateDir(None),
        terminal_backend: local_shell_backend(),
        script_backend: local_script_backend(),
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
    let root = parts.app.clone();
    cx.update(|cx| {
        let timeline = root.read(cx).thread_view().read(cx).chat().read(cx).timeline().clone();
        timeline.update(cx, |t, _| t.set_now_millis(t3code_web::fixtures::timelines::NOW_MS));
        root.update(cx, |root, cx| root.navigate(Route::parse(if owner { OWNER_THREAD } else { THREAD }), cx));
        NotificationInbox::set_items(reference_inbox(), cx);
        // Settings pages read a connected server's config, as the reference's had.
        if scene.name.starts_with("settings-") && scene.name != "settings-environments" {
            t3code_web::scenes::settings::seed_fixture(cx);
        }
    });
    frame(&mut cx, window);
    // The capture script leaves the mouse over the conversation before each view.
    mouse(&mut cx, window, point(px(700.), px(400.)), None);
    frame(&mut cx, window);
    for step in scene.steps {
        match step {
            Step::Move(x, y) => mouse(&mut cx, window, point(px(*x), px(*y)), None),
            Step::Click(x, y) => mouse(&mut cx, window, point(px(*x), px(*y)), Some(MouseButton::Left)),
            Step::RightClick(x, y) => mouse(&mut cx, window, point(px(*x), px(*y)), Some(MouseButton::Right)),
            Step::ClickId(id) => {
                cx.update_window(window, |_, w, cx| w.click(*id, cx)).ok();
            }
            Step::Wheel(x, y, dy) => {
                cx.update_window(window, |_, w, cx| {
                    w.dispatch_event(
                        ScrollEvent {
                            position: point(px(*x), px(*y)),
                            delta: ScrollDelta::Pixels(point(px(0.), px(*dy))),
                            ..Default::default()
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .ok();
            }
            Step::Type(text) => {
                cx.update_window(window, |_, w, cx| w.input(text, cx)).ok();
            }
            Step::Press(keys) => {
                let keys = keys.replace("mod-", if cfg!(target_os = "macos") { "cmd-" } else { "ctrl-" });
                cx.update_window(window, |_, w, cx| w.press(&keys, cx)).ok();
            }
            Step::Action(action) => {
                cx.update_window(window, |_, w, cx| w.dispatch_action(action(), cx)).ok();
            }
            Step::Sidebar(edit) => {
                let sidebar = cx.update(|cx| root.read(cx).sidebar().clone());
                cx.update_window(window, |_, w, cx| sidebar.update(cx, |s, cx| edit(s, w, cx))).ok();
            }
            Step::Drag(x0, y0, x1, y1) => {
                cx.update_window(window, |_, w, cx| {
                    let at = point(px(*x0), px(*y0));
                    w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
                    w.dispatch_event(
                        MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }
                            .to_platform_input(),
                        cx,
                    );
                })
                .ok();
                for i in 1..=10 {
                    let t = i as f32 / 10.;
                    let at = point(px(x0 + (x1 - x0) * t), px(y0 + (y1 - y0) * t));
                    cx.update_window(window, |_, w, cx| {
                        w.dispatch_event(
                            MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::default() }.to_platform_input(),
                            cx,
                        );
                    })
                    .ok();
                    frame(&mut cx, window);
                }
            }
            Step::HoldModifier => {
                let modifiers = if cfg!(target_os = "macos") { Modifiers::command() } else { Modifiers::control() };
                cx.update_window(window, |_, w, cx| {
                    w.dispatch_event(ModifiersChangedEvent { modifiers, capslock: Default::default() }.to_platform_input(), cx);
                })
                .ok();
            }
            Step::Navigate(route) => cx.update(|cx| root.update(cx, |root, cx| root.navigate(Route::parse(route), cx))),
            Step::Wait(ms) => {
                let until = Instant::now() + Duration::from_millis(*ms);
                while Instant::now() < until {
                    frame(&mut cx, window);
                    std::thread::sleep(Duration::from_millis(50));
                    // Timers on the headless dispatcher run on its simulated clock.
                    cx.advance_clock(Duration::from_millis(50));
                }
            }
        }
        frame(&mut cx, window);
    }
    frame(&mut cx, window);
    let file = out.join(format!("{}.png", scene.name));
    cx.capture_screenshot(window)?.save(&file)?;
    println!("{} <- ref/{}.png", file.display(), scene.reference);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().context("usage: parity-shot <out-dir> [scene-filter]")?);
    let filter = args.next();
    std::fs::create_dir_all(&out)?;
    let data = out.join(".runtime");
    std::fs::create_dir_all(&data)?;
    for scene in SCENES.iter().filter(|s| filter.as_deref().is_none_or(|f| s.name.contains(f))) {
        if let Err(error) = render(scene, false, &out, &data) {
            eprintln!("FAILED {}: {error:#}", scene.name);
        }
    }
    let owner_settings = std::env::var_os("T3CODE_CLIENT_SETTINGS").is_some();
    for scene in OWNER_SCENES.iter().filter(|s| filter.as_deref().is_none_or(|f| s.name.contains(f))) {
        if !owner_settings {
            eprintln!("SKIPPED {}: set T3CODE_CLIENT_SETTINGS to the owner's client settings file", scene.name);
            continue;
        }
        if let Err(error) = render(scene, true, &out, &data) {
            eprintln!("FAILED {}: {error:#}", scene.name);
        }
    }
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
