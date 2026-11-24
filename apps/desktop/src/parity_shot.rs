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
    Scene { name: "sidebar-hover", reference: "sidebar-hover", light: false, steps: &[Step::Move(127.5, 224.)] },
    Scene { name: "sidebar-context-menu", reference: "sidebar-context-menu", light: false, steps: &[Step::RightClick(127.5, 224.)] },
    Scene { name: "sidebar-inbox", reference: "sidebar-inbox", light: false, steps: &[Step::ClickId("sidebar-notifications")] },
    Scene { name: "timeline-top", reference: "timeline-top", light: false, steps: &[Step::Wheel(700., 400., 20_000.)] },
    Scene {
        name: "timeline-tools-expanded",
        reference: "timeline-tools-expanded",
        light: false,
        // The port's turn sits 39px lower than the fork's: its first prompt is a
        // bubble, not the fork's one-line Agent pill.
        steps: &[Step::Wheel(700., 400., 20_000.), Step::Click(340., 181.)],
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
    Scene { name: "composer-model-picker", reference: "composer-model-picker", light: false, steps: &[Step::Click(960., 733.)] },
    Scene { name: "composer-mention", reference: "composer-mention", light: false, steps: &[Step::Click(700., 690.), Step::Type("@"), Step::Wait(300)] },
    Scene { name: "command-palette", reference: "command-palette", light: false, steps: &[Step::Action(|| Box::new(ToggleCommandPalette))] },
    Scene { name: "settings-environments", reference: "settings-environments", light: false, steps: &[Step::Navigate("/settings/connections")] },
    Scene { name: "terminal-drawer", reference: "terminal-drawer", light: false, steps: TERMINAL_STEPS },
    Scene { name: "terminal-menu", reference: "terminal-menu", light: false, steps: &[Step::Action(|| Box::new(ToggleTerminal)), Step::Wait(1500), Step::Press("mod-n"), Step::Wait(1200), Step::RightClick(400.5, 626.)] },
    Scene { name: "terminal-close-confirm", reference: "terminal-close-confirm", light: false, steps: &[Step::Action(|| Box::new(ToggleTerminal)), Step::Wait(1500), Step::Press("mod-n"), Step::Wait(1200), Step::Click(1088., 512.5)] },
    Scene { name: "right-panel-diff", reference: "right-panel-diff", light: false, steps: &[Step::Action(|| Box::new(ToggleDiff)), Step::Wait(300)] },
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
fn render(scene: &Scene, out: &std::path::Path, data: &std::path::Path) -> anyhow::Result<()> {
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
        ThemeSettings::set_glass_opacity(GlassOpacity::new(OWNER_GLASS_OPACITY), cx);
        ConnectionRuntime::init(config, cx)
    })?;
    let options = AppRootOptions {
        store: FixtureStore::reference(),
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
        root.update(cx, |root, cx| root.navigate(Route::parse(THREAD), cx));
        NotificationInbox::set_items(reference_inbox(), cx);
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
            Step::Navigate(route) => cx.update(|cx| root.update(cx, |root, cx| root.navigate(Route::parse(route), cx))),
            Step::Wait(ms) => {
                let until = Instant::now() + Duration::from_millis(*ms);
                while Instant::now() < until {
                    frame(&mut cx, window);
                    std::thread::sleep(Duration::from_millis(50));
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
        if let Err(error) = render(scene, &out, &data) {
            eprintln!("FAILED {}: {error:#}", scene.name);
        }
    }
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
