//! The owner profile of `parity-shot`: every reference view of the owner's Mac capture
//! (`artifacts/macparity/ref`) as a scene, rendered the way the owner's Mac shows the app.
//!
//! Usage: `parity-shot --mac <out-dir> [scene-filter]`, `parity-shot --list-mac`.
//! - `PARITY_CLIENT_SETTINGS=<client-settings.json>` (required): the install's client settings,
//!   applied whole (interface and code fonts, glass, contrast, chat width, grouping, ...).
//! - `PARITY_LAYOUT=<file|dir>`: the saved layout the app imports from the install's
//!   localStorage: a JSON object of localStorage keys, or a Chromium `Local Storage/leveldb`
//!   directory, read from a copy as the first-launch import reads it. It gives the sidebar width
//!   and open state and the Working, Snoozed and Settled shelf states.
//! - `PARITY_SIZE=<w>x<h>` (default 1710x1074, the owner's maximized window), device scale 2,
//!   the macOS titlebar, dark appearance.
//!
//! Each scene opens a fresh window on `FixtureStore::reference()` at the moment its reference was
//! captured, then drives it with real input. Targets are found by element id or measured from
//! the laid-out window, never by fixed coordinates, so a scene works at any window size.

#[path = "import/install.rs"]
#[allow(dead_code)]
mod install;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use serde_json::{Map, Value};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRoot, AppRootOptions, DataSource, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::components::sidebar_logic::SidebarListEntry;
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::desktop_notifications::NotificationInbox;
use t3code_web::fixtures::FixtureStore;
use t3code_web::hooks::use_settings::ClientSettingsStore;
use t3code_web::hooks::use_theme::ThemeStore;
use t3code_web::router::Route;
use t3code_web::shortcuts::commands::{
    NewTerminal, OpenUsage, ToggleCommandPalette, ToggleDiff, ToggleFilePicker, ToggleProjectSearch, ToggleRightPanel, ToggleSidebar,
    CloseTerminal, ToggleModelPicker, ToggleTerminal, ToggleThreadPanel,
};
use t3code_web::components::settings::provider_settings_panel::ProviderSettingsPanel;
use t3code_web::routes::settings::SettingsRoute;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::theme_palette::ThemeMode;
use t3code_web::ui_state_store::{LayoutState, StateDir, files};

/// Where a measured click lands inside an element: a fraction of its width and height plus an
/// offset in pixels.
#[derive(Clone, Copy)]
struct At(f32, f32, f32, f32);

const CENTER: At = At(0.5, 0.5, 0., 0.);

/// One input or wait applied before the capture.
#[derive(Clone, Copy)]
enum Step {
    /// Clicks an element at a measured point.
    Click(&'static str, At),
    RightClick(&'static str, At),
    Hover(&'static str, At),
    /// Hovers or right-clicks the sidebar row of the thread titled so (measured from the
    /// sidebar's laid-out list).
    HoverRow(&'static str),
    RightClickRow(&'static str),
    /// A point in the main column: `fx` of its width right of the sidebar, `y` from the top
    /// (from the bottom when negative); clicked with the button, or hovered without one.
    Main(f32, f32, Option<MouseButton>),
    /// A point `dx` right of the sidebar's edge and `y` from the top (titlebar controls that
    /// sit after the sidebar), or in the sidebar when `dx` is negative (from its right edge).
    FromSidebar(f32, f32, Option<MouseButton>),
    /// A point given as a fraction of the window width and `y` from the bottom.
    AtWindow(f32, f32, MouseButton),
    /// Scrolls the element by pages of its height less 60px, as the capture did.
    ScrollPages(&'static str, f32),
    /// Scrolls the conversation to its first row.
    ConversationTop,
    /// Calls the window's app directly, for a state no input reaches by id (a right-panel tab).
    Root(fn(&Entity<AppRoot>, &mut Window, &mut App)),
    /// Calls the open settings modal directly (a dialog or recording state no id reaches).
    Settings(fn(&Entity<SettingsRoute>, &mut Window, &mut App)),
    Type(&'static str),
    /// A keystroke, `mod-` meaning cmd on macOS and ctrl elsewhere.
    Press(&'static str),
    Dispatch(fn() -> Box<dyn Action>),
    Navigate(&'static str),
    /// Opens the thread with this title, as clicking its row does.
    Thread(&'static str),
    /// Gives the settings modal the server pages over the fixture environment.
    ServerPages,
    /// Changes app state directly (a setting the capture turned on).
    Run(fn(&mut App)),
    /// Lets timers and background work run.
    Wait(u64),
}

use Step::*;

/// A reference view and how to reach it.
struct Scene {
    name: &'static str,
    steps: &'static [Step],
}

const CARD: Step = Dispatch(|| Box::new(ToggleThreadPanel));
const TERMINAL: &[Step] = &[
    CARD,
    Dispatch(|| Box::new(ToggleTerminal)),
    Wait(1500),
    Type("exec env PS1='scenario@host:\\W$ ' bash --norc --noprofile\n"),
    Wait(600),
    Type("clear; git status --short; ls src\n"),
    Wait(800),
    Dispatch(|| Box::new(NewTerminal)),
    Wait(1500),
];

macro_rules! settings {
    ($($name:literal => $slug:literal $(, $server:ident)?;)*) => {
        &[$(Scene { name: $name, steps: &[CARD, Navigate(concat!("/settings/", $slug)) $(, $server)?, Wait(300)] },)*]
    };
}

/// The plain settings panels: (view, route slug).
const SETTINGS: &[Scene] = settings! {
    "settings-general" => "general";
    "settings-appearance" => "appearance";
    "settings-chat" => "chat";
    "settings-diffs" => "diffs";
    "settings-keybindings" => "keybindings";
    "settings-voice" => "voice";
    "settings-snap-shot" => "snap-shot";
    "settings-labs" => "labs";
    "settings-beta" => "beta";
    "settings-archived" => "archived";
    "settings-about" => "about";
    "settings-connections" => "connections";
    "settings-providers" => "providers", ServerPages;
    "settings-threads" => "threads", ServerPages;
    "settings-source-control" => "source-control", ServerPages;
    "settings-worktrees" => "worktrees", ServerPages;
    "settings-projects" => "projects", ServerPages;
    "settings-scheduled-tasks" => "scheduled-tasks", ServerPages;
    "settings-browser" => "browser", ServerPages;
    "settings-devices" => "devices", ServerPages;
    "settings-access" => "access", ServerPages;
    "settings-maintenance" => "maintenance", ServerPages;
    "settings-secrets" => "secrets", ServerPages;
    "settings-integrations" => "integrations", ServerPages;
    "settings-storage" => "storage", ServerPages;
};

const SCENES: &[Scene] = &[
    Scene { name: "app-window", steps: &[CARD] },
    Scene { name: "app-home", steps: &[Navigate("/"), Wait(300), CARD] },
    Scene { name: "details", steps: &[CARD, Wait(300)] },
    Scene { name: "details-items-menu", steps: &[CARD, Wait(300), RightClick("thread-details-card", At(0.5, 1., 0., -12.))] },
    Scene { name: "open-in-editor-menu", steps: &[CARD, Wait(300), Click("open-in-menu-trigger", CENTER), Wait(300)] },
    Scene { name: "git-menu", steps: &[CARD, Wait(300), Click("git-menu-trigger", CENTER), Wait(300)] },
    Scene { name: "branch-picker", steps: &[CARD, Wait(300), Click("branch-select", CENTER), Wait(500)] },
    Scene { name: "commit-dialog", steps: &[CARD, Wait(300), Click("git-primary", CENTER), Wait(800)] },
    Scene { name: "composer-empty", steps: &[CARD] },
    Scene {
        name: "composer-markdown",
        steps: &[
            CARD,
            Main(0.5, -95., Some(MouseButton::Left)),
            Type("Use `inline code` and **bold** here"),
            Press("shift-enter"),
            Type("```ts"),
            Press("shift-enter"),
            Type("const x = sum(1, 2);"),
            Press("shift-enter"),
            Type("```"),
            Wait(300),
        ],
    },
    Scene { name: "composer-mention", steps: &[CARD, Main(0.5, -95., Some(MouseButton::Left)), Type("@"), Wait(1200)] },
    Scene { name: "composer-slash", steps: &[CARD, Main(0.5, -95., Some(MouseButton::Left)), Type("/"), Wait(1200)] },
    Scene { name: "composer-model-picker", steps: &[CARD, Main(0.5, -95., Some(MouseButton::Left)), Dispatch(|| Box::new(ToggleModelPicker)), Wait(500)] },
    Scene { name: "palette-root", steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(400)] },
    Scene { name: "palette-query", steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(200), Type("theme"), Wait(600)] },
    Scene { name: "palette-empty", steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(200), Type("zzqpv"), Wait(600)] },
    Scene { name: "palette-query-sync", steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(200), Type("sync"), Wait(600)] },
    Scene { name: "palette-messages", steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(200), Type("reconnect"), Wait(1200)] },
    Scene {
        name: "palette-nested-project",
        steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(200), Type("New thread in"), Wait(300), Press("enter"), Wait(500)],
    },
    Scene {
        name: "palette-nested-theme",
        steps: &[CARD, Dispatch(|| Box::new(ToggleCommandPalette)), Wait(200), Type("Change theme"), Wait(300), Press("enter"), Wait(500)],
    },
    Scene { name: "sidebar-hover", steps: &[CARD, HoverRow("Rotate relay TLS certificates"), Wait(400)] },
    Scene { name: "sidebar-thread-menu", steps: &[CARD, RightClickRow("Rotate relay TLS certificates"), Wait(400)] },
    Scene { name: "sidebar-search", steps: &[CARD, FromSidebar(-200., 75., Some(MouseButton::Left)), Type("relay"), Wait(800)] },
    Scene { name: "sidebar-collapsed", steps: &[CARD, Dispatch(|| Box::new(ToggleSidebar)), Wait(1200)] },
    // The owner keeps the inbox off; the capture turned it on.
    Scene {
        name: "sidebar-inbox-inbox-on",
        steps: &[
            CARD,
            Run(|cx| ClientSettingsStore::update(cx, |s| s.notification_inbox_enabled = true)),
            Wait(300),
            Click("sidebar-notifications", CENTER),
            Wait(400),
        ],
    },
    Scene { name: "timeline-top", steps: &[CARD, ConversationTop, Wait(600)] },
    // The capture scrolled the first code block to the centre, which the top of the thread already shows.
    Scene { name: "timeline-code", steps: &[CARD, ConversationTop, Wait(600)] },
    Scene { name: "timeline-bottom", steps: &[CARD, Wait(300)] },
    Scene { name: "timeline-approval", steps: &[CARD, Thread("Rotate relay TLS certificates"), Wait(1500)] },
    Scene { name: "timeline-question", steps: &[CARD, Thread("Draft launch copy for the changelog"), Wait(1500)] },
    Scene { name: "timeline-todo", steps: &[CARD, Thread("Port sidebar virtualization to GPUI"), Wait(1500)] },
    Scene { name: "timeline-working", steps: &[CARD, Thread("Profile the event sink"), Wait(1500)] },
    Scene { name: "timeline-failed", steps: &[CARD, Thread("Migrate CI runners to arm64"), Wait(1500)] },
    Scene { name: "thread-actions-menu", steps: &[FromSidebar(170., 26., Some(MouseButton::Left)), Wait(400)] },
    Scene { name: "terminal-two", steps: TERMINAL },
    Scene { name: "terminal-menu", steps: &[CARD, Dispatch(|| Box::new(ToggleTerminal)), Wait(1500), Dispatch(|| Box::new(NewTerminal)), Wait(1500), AtWindow(0.36, 100., MouseButton::Right), Wait(400)] },
    Scene {
        name: "terminal-close-confirm",
        steps: &[CARD, Dispatch(|| Box::new(ToggleTerminal)), Wait(1500), Dispatch(|| Box::new(NewTerminal)), Wait(1500), Dispatch(|| Box::new(CloseTerminal)), Wait(400)],
    },
    Scene { name: "right-panel-launcher", steps: &[Dispatch(|| Box::new(ToggleRightPanel)), Wait(1500)] },
    Scene { name: "diff-collapsed", steps: &[Dispatch(|| Box::new(ToggleDiff)), Wait(2500)] },
    Scene { name: "diff-stacked", steps: &[Dispatch(|| Box::new(ToggleDiff)), Wait(2500), Root(reveal_math), Wait(1500)] },
    Scene { name: "diff-split", steps: &[Dispatch(|| Box::new(ToggleDiff)), Wait(2500), Root(reveal_math), Wait(800), Click("diff-split", CENTER), Wait(1500)] },
    Scene { name: "diff-scope-menu", steps: &[Dispatch(|| Box::new(ToggleDiff)), Wait(2500), Click("diff-scope", CENTER), Wait(400)] },
    // The capture's "Diff options" click missed: its image shows the Files tab on src/math.ts.
    Scene { name: "diff-options-menu", steps: &[Dispatch(|| Box::new(ToggleRightPanel)), Wait(1000), Root(|r, w, cx| open_file(r, "src/math.ts", w, cx)), Wait(1800)] },
    Scene { name: "files-tree", steps: &[Dispatch(|| Box::new(ToggleRightPanel)), Wait(1000), Root(open_files), Wait(2000)] },
    Scene { name: "files-text", steps: &[Dispatch(|| Box::new(ToggleRightPanel)), Wait(1000), Root(|r, w, cx| open_file(r, "src/math.ts", w, cx)), Wait(1800)] },
    Scene { name: "files-markdown", steps: &[Dispatch(|| Box::new(ToggleRightPanel)), Wait(1000), Root(|r, w, cx| open_file(r, "README.md", w, cx)), Wait(1800)] },
    Scene { name: "files-image", steps: &[Dispatch(|| Box::new(ToggleRightPanel)), Wait(1000), Root(|r, w, cx| open_file(r, "logo.png", w, cx)), Wait(1800)] },
    Scene { name: "files-goto", steps: &[Dispatch(|| Box::new(ToggleFilePicker)), Wait(300), Type("ma"), Wait(1200)] },
    Scene { name: "files-search", steps: &[Dispatch(|| Box::new(ToggleProjectSearch)), Wait(300), Type("return"), Wait(2000)] },
    Scene { name: "usage", steps: &[CARD, Dispatch(|| Box::new(OpenUsage)), Wait(3000)] },
    Scene { name: "pull-requests", steps: &[Navigate("/pull-requests"), Wait(3000)] },
    Scene { name: "issues", steps: &[Navigate("/issues"), Wait(3000)] },
    Scene { name: "settings-providers-codex", steps: &[CARD, Navigate("/settings/providers"), ServerPages, Wait(300), Click("provider-row-codex", CENTER), Wait(1500)] },
    Scene { name: "settings-providers-replay", steps: &[CARD, Navigate("/settings/providers"), ServerPages, Wait(300), Settings(|s, w, cx| providers(s, cx, |p, _, cx| p.select("replayRich", cx), w)), Wait(1500)] },
    Scene { name: "settings-providers-add", steps: &[CARD, Navigate("/settings/providers"), ServerPages, Wait(300), Settings(|s, w, cx| providers(s, cx, |p, w, cx| p.open_add_dialog(w, cx), w)), Wait(1500)] },
    Scene { name: "settings-keybindings-recording", steps: &[CARD, Navigate("/settings/keybindings"), Wait(300), Settings(record_command_palette), Wait(1000)] },
    Scene { name: "settings-task-editor", steps: &[CARD, Navigate("/settings/scheduled-tasks"), ServerPages, Wait(300), Click("task-new", CENTER), Wait(1500)] },
    Scene { name: "settings-connections-add", steps: &[CARD, Navigate("/settings/connections"), Wait(300), Click("add-environment", CENTER), Wait(1500)] },
];

/// Settings pages the capture also scrolled: (view, pages). Page `i` scrolls the content by
/// `i * (viewport - 60)`, as the capture did.
const SCROLLED: &[(&str, usize)] = &[
    ("settings-appearance", 2),
    ("settings-browser", 1),
    ("settings-general", 1),
    ("settings-integrations", 1),
    ("settings-keybindings", 6),
    ("settings-maintenance", 1),
    ("settings-providers", 1),
    ("settings-secrets", 1),
    ("settings-source-control", 1),
    ("settings-storage", 1),
    ("settings-threads", 2),
    ("settings-voice", 1),
];

/// Reference views no scene reaches, and why.
pub const UNREACHABLE: &[(&str, &str)] = &[
    ("settings-connections-saved", "needs a second paired server; fixture windows have no saved environment in the connection catalog"),
    ("settings-connections-routes", "needs a saved environment (see settings-connections-saved)"),
    ("settings-connections-menu", "needs a saved environment (see settings-connections-saved)"),
];

/// When each reference view was captured (epoch seconds), so relative times read the same.
const CAPTURED_AT: &[(&str, i64)] = &[
    ("app-home", 1795492500), ("app-window", 1795492494), ("branch-picker", 1795493199), ("commit-dialog", 1795493213),
    ("composer-empty", 1795492605), ("composer-markdown", 1795492614), ("composer-mention", 1795492637),
    ("composer-model-picker", 1795492622), ("composer-slash", 1795492629), ("details-items-menu", 1795493545),
    ("details", 1795493175), ("diff-collapsed", 1795493157), ("diff-options-menu", 1795492862), ("diff-scope-menu", 1795493165),
    ("diff-split", 1795493162), ("diff-stacked", 1795493159), ("files-goto", 1795493070), ("files-image", 1795493067),
    ("files-markdown", 1795493003), ("files-search", 1795493074), ("files-text", 1795492939), ("files-tree", 1795492905),
    ("git-menu", 1795493188), ("issues", 1795493114), ("open-in-editor-menu", 1795493178), ("palette-empty", 1795492581),
    ("palette-messages", 1795493436), ("palette-nested-project", 1795492589), ("palette-nested-theme", 1795492599),
    ("palette-query-sync", 1795493426), ("palette-query", 1795492573), ("palette-root", 1795492564), ("pull-requests", 1795493104),
    ("right-panel-launcher", 1795492719), ("settings-about", 1795492348), ("settings-access", 1795492440),
    ("settings-appearance", 1795492266), ("settings-archived", 1795492338), ("settings-beta", 1795492488),
    ("settings-browser", 1795492413), ("settings-chat", 1795492277), ("settings-connections-add", 1795493261),
    ("settings-connections", 1795492431), ("settings-devices", 1795492422), ("settings-diffs", 1795492286),
    ("settings-general", 1795492257), ("settings-integrations", 1795492469), ("settings-keybindings-recording", 1795493245),
    ("settings-keybindings", 1795492295), ("settings-labs", 1795492329), ("settings-maintenance", 1795492450),
    ("settings-projects", 1795492395), ("settings-providers-add", 1795493238), ("settings-providers-codex", 1795493221),
    ("settings-providers-replay", 1795493229), ("settings-providers", 1795492357), ("settings-scheduled-tasks", 1795492404),
    ("settings-secrets", 1795492459), ("settings-snap-shot", 1795492320), ("settings-source-control", 1795492377),
    ("settings-storage", 1795492478), ("settings-task-editor", 1795493252), ("settings-threads", 1795492367),
    ("settings-voice", 1795492310), ("settings-worktrees", 1795492386), ("sidebar-collapsed", 1795493269),
    ("sidebar-hover", 1795492522), ("sidebar-inbox-inbox-on", 1795493312), ("sidebar-search", 1795492507),
    ("sidebar-thread-menu", 1795492514), ("terminal-close-confirm", 1795492708), ("terminal-menu", 1795492706),
    ("terminal-two", 1795492704), ("thread-actions-menu", 1795493084), ("timeline-approval", 1795492663),
    ("timeline-bottom", 1795492652), ("timeline-code", 1795492691), ("timeline-failed", 1795493289),
    ("timeline-question", 1795492673), ("timeline-todo", 1795492683), ("timeline-top", 1795492645),
    ("timeline-working", 1795493279), ("usage", 1795493095),
];

/// The shown thread's scoped key (`environmentId:threadId`), as the right panel keys it.
fn shown_thread(root: &Entity<AppRoot>, cx: &App) -> Option<String> {
    root.read(cx).sidebar().read(cx).active_thread().map(|key| format!("{}:{}", key.environment_id.0, key.thread_id.0))
}

/// Opens the Files tab of the shown thread, as its launcher row does.
fn open_files(root: &Entity<AppRoot>, _: &mut Window, cx: &mut App) {
    let Some(thread) = shown_thread(root, cx) else { return };
    root.read(cx).right_panels().clone().update(cx, |store, cx| {
        store.open(&thread, t3code_web::right_panel_store::SingletonKind::Files);
        cx.notify();
    });
}

/// Opens `path` in the Files tab, as clicking it in the file tree does.
fn open_file(root: &Entity<AppRoot>, path: &'static str, window: &mut Window, cx: &mut App) {
    root.read(cx).right_panel().clone().update(cx, |panel, cx| panel.open_file(path.into(), None, window, cx));
}

/// Expands src/math.ts in the Diff tab, as clicking its header does.
fn reveal_math(root: &Entity<AppRoot>, _: &mut Window, cx: &mut App) {
    root.read(cx).right_panel().clone().update(cx, |panel, cx| panel.open_diff(None, Some("src/math.ts".into()), cx));
}

/// Runs `call` on the open settings modal's Providers panel.
fn providers(
    settings: &Entity<SettingsRoute>,
    cx: &mut App,
    call: fn(&mut ProviderSettingsPanel, &mut Window, &mut Context<ProviderSettingsPanel>),
    window: &mut Window,
) {
    let panel = settings.read(cx).server_pages().and_then(|pages| pages.read(cx).typed::<ProviderSettingsPanel>(SettingsPath::Providers));
    if let Some(panel) = panel {
        panel.update(cx, |panel, cx| call(panel, window, cx));
    }
}

/// Starts recording a shortcut for the Command Palette row, as clicking its shortcut does.
fn record_command_palette(settings: &Entity<SettingsRoute>, window: &mut Window, cx: &mut App) {
    let keybindings = settings.read(cx).pages().read(cx).keybindings().clone();
    keybindings.update(cx, |page, cx| page.start_recording_for_scene("commandPalette.toggle", window, cx));
}

/// Every scene name with its steps; scroll pages are `<view>.scroll-<n>`.
fn all_scenes() -> Vec<(String, Vec<Step>)> {
    let mut scenes: Vec<(String, Vec<Step>)> = SCENES.iter().chain(SETTINGS).map(|s| (s.name.to_owned(), s.steps.to_vec())).collect();
    for (view, pages) in SCROLLED {
        let base = SETTINGS.iter().find(|s| s.name == *view).expect("scrolled views are settings panels").steps;
        for page in 1..=*pages {
            let mut steps = base.to_vec();
            steps.push(ScrollPages("settings-content", page as f32));
            steps.push(Wait(400));
            scenes.push((format!("{view}.scroll-{page}"), steps));
        }
    }
    scenes
}

/// The names `--mac` renders, one per line, for runners that render one scene per process.
pub fn list() {
    for (name, _) in all_scenes() {
        println!("{name}");
    }
}

/// The owner's saved layout: the localStorage values the real app imports.
struct Layout {
    sidebar_width: Option<f32>,
    sidebar_open: Option<bool>,
    working: Option<bool>,
    snoozed: Option<bool>,
    settled: Option<bool>,
}

impl Layout {
    /// Reads `PARITY_LAYOUT`: a JSON object of localStorage keys (or `{ origin: { key: value } }`),
    /// or a Chromium localStorage leveldb directory.
    fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let mut values: Map<String, Value> = Map::new();
        if let Some(path) = path {
            if path.is_dir() {
                for entry in install::app_entries(install::read_local_storage(path)?) {
                    values.insert(entry.key, Value::String(entry.value));
                }
            } else {
                let json: Map<String, Value> = serde_json::from_slice(&std::fs::read(path)?).context("layout file is a JSON object")?;
                let nested = json.values().all(Value::is_object) && !json.is_empty();
                values = if nested { json.into_values().filter_map(|v| v.as_object().cloned()).flatten().collect() } else { json };
            }
        }
        let text = |key: &str| values.get(key).map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()));
        let flag = |key: &str| text(key).and_then(|v| v.parse::<bool>().ok());
        Ok(Self {
            sidebar_width: text("chat_thread_sidebar_width").and_then(|v| v.parse().ok()),
            sidebar_open: flag("chat_thread_sidebar_open"),
            working: flag("t3code:sidebar:working-expanded"),
            snoozed: flag("t3code:sidebar:snoozed-expanded"),
            settled: flag("t3code:sidebar:settled-expanded"),
        })
    }
}

/// How every scene's window is set up.
struct Profile {
    size: (f32, f32),
    settings: PathBuf,
    layout: Layout,
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

fn wheel(cx: &mut HeadlessAppContext, window: AnyWindowHandle, at: Point<Pixels>, dy: f32) {
    cx.update_window(window, |_, w, cx| {
        w.dispatch_event(
            ScrollEvent { position: at, delta: ScrollDelta::Pixels(point(px(0.), px(dy))), ..Default::default() }.to_platform_input(),
            cx,
        );
    })
    .ok();
}

/// The bounds of element `id` in the last frame.
fn bounds(cx: &mut HeadlessAppContext, window: AnyWindowHandle, id: ElementId) -> anyhow::Result<Bounds<Pixels>> {
    cx.update_window(window, |_, w, _| w.try_find(id.clone()).filter(|e| e.visible()).map(|e| e.bounds()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible element {id:?}"))
}

fn point_in(b: Bounds<Pixels>, at: At) -> Point<Pixels> {
    point(b.origin.x + b.size.width * at.0 + px(at.2), b.origin.y + b.size.height * at.1 + px(at.3))
}

/// The sidebar list entry index of the thread titled `title`.
fn row_index(cx: &mut HeadlessAppContext, root: &Entity<AppRoot>, title: &str) -> anyhow::Result<usize> {
    cx.update(|cx| {
        root.read(cx).sidebar().read(cx).list().entries.iter().position(|entry| match entry {
            SidebarListEntry::Thread { thread, .. } => thread.title.as_ref() == title,
            _ => false,
        })
    })
    .with_context(|| format!("no sidebar row titled {title}"))
}

/// Where the main column starts: the saved sidebar width, or the default when none is saved.
fn sidebar_right(layout: &Layout) -> f32 {
    if layout.sidebar_open == Some(false) { 0. } else { layout.sidebar_width.unwrap_or(LayoutState::default().sidebar_width) }
}

fn captured_at(name: &str) -> Option<i64> {
    let view = name.split(".scroll-").next().unwrap_or(name);
    CAPTURED_AT.iter().find(|(n, _)| *n == view).map(|(_, s)| s * 1000)
}

/// Opens a fresh window on the reference world as the owner's Mac shows it, applies the steps
/// and saves the frame as `<out>/<name>.png`.
fn render(name: &str, steps: &[Step], profile: &Profile, out: &Path, data: &Path) -> anyhow::Result<()> {
    let mut cx = headless();
    cx.update(|cx| {
        t3code_web::init(cx);
        ClientSettingsStore::import_electron(&profile.settings, cx)?;
        // The owner's Mac runs in dark appearance; "system" would follow the headless platform.
        ThemeStore::set_appearance_mode(ThemeMode::Dark, cx);
        anyhow::Ok(())
    })?;
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
        data_dir: data.to_path_buf(),
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui parity".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| ConnectionRuntime::init(config, cx))?;

    let mut store = FixtureStore::reference();
    let now = captured_at(name).unwrap_or_else(|| store.snapshot(None).now.0);
    store.set_now(now);
    let threads: HashMap<String, ThreadKey> =
        store.snapshot(None).threads.iter().map(|t| (t.title.to_string(), t.key.clone())).collect();
    let active = store
        .thread_by_title(t3code_web::fixtures::reference_world::ACTIVE_TITLE)
        .context("the reference world has its active thread")?;
    let inbox = store.inbox_items();

    // The saved layout goes where the app reads it, so the window opens with it as the real one does.
    let state = data.join(format!("state-{name}"));
    std::fs::create_dir_all(&state)?;
    let defaults = LayoutState::default();
    let layout = LayoutState {
        last_route: format!("/{}/{}", active.environment_id.0, active.thread_id.0),
        sidebar_open: profile.layout.sidebar_open.unwrap_or(defaults.sidebar_open),
        sidebar_width: profile.layout.sidebar_width.unwrap_or(defaults.sidebar_width),
        right_panel_width: defaults.right_panel_width,
    };
    std::fs::write(state.join(files::LAYOUT), serde_json::to_string(&layout)?)?;

    let options = AppRootOptions { data: DataSource::Fixtures(store), platform: TitlebarPlatform::Mac, state_dir: StateDir(Some(state)) };
    let (width, height) = profile.size;
    let (window, parts) = cx.update(|cx| {
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
            show: false,
            focus: true,
            ..component::TitleBar::window_options()
        };
        WindowRoot::open_window(window_options, options, cx)
    })?;
    let root = parts.app.clone();
    let shell = parts.shell.clone();
    let route = |key: &ThreadKey| Route::parse(&format!("/{}/{}", key.environment_id.0, key.thread_id.0));
    cx.update(|cx| {
        let timeline = root.read(cx).thread_view().read(cx).chat().read(cx).timeline().clone();
        timeline.update(cx, |t, _| t.set_now_millis(now));
        root.update(cx, |root, cx| root.navigate(route(&active), cx));
        let shelves = (profile.layout.working, profile.layout.snoozed, profile.layout.settled);
        root.read(cx).sidebar().clone().update(cx, |sidebar, cx| {
            sidebar.options_mut(cx, |o| {
                if let Some(v) = shelves.0 {
                    o.working_expanded = v;
                }
                if let Some(v) = shelves.1 {
                    o.snoozed_expanded = v;
                }
                if let Some(v) = shelves.2 {
                    o.settled_expanded = v;
                }
            })
        });
        NotificationInbox::set_items(inbox.iter().filter(|n| !n.read).cloned().collect(), cx);
        if name.starts_with("settings-") {
            t3code_web::scenes::settings::seed_fixture(cx);
        }
    });
    frame(&mut cx, window);
    // The capture leaves the mouse over empty space near the bottom right before each view.
    let rest = point(px(width - 150.), px(height - 40.));
    mouse(&mut cx, window, rest, None);
    frame(&mut cx, window);

    for step in steps {
        match *step {
            Click(id, at) | RightClick(id, at) | Hover(id, at) => {
                let p = point_in(bounds(&mut cx, window, id.into())?, at);
                let button = match step {
                    Click(..) => Some(MouseButton::Left),
                    RightClick(..) => Some(MouseButton::Right),
                    _ => None,
                };
                mouse(&mut cx, window, p, button);
            }
            HoverRow(title) | RightClickRow(title) => {
                let ix = row_index(&mut cx, &root, title)?;
                let b = cx
                    .update(|cx| root.read(cx).sidebar().read(cx).entry_bounds(ix))
                    .with_context(|| format!("sidebar row {title} is not laid out"))?;
                mouse(&mut cx, window, b.center(), matches!(step, RightClickRow(_)).then_some(MouseButton::Right));
            }
            Main(fx, y, button) => {
                let sidebar = sidebar_right(&profile.layout);
                let y = if y < 0. { height + y } else { y };
                mouse(&mut cx, window, point(px(sidebar + fx * (width - sidebar)), px(y)), button);
            }
            FromSidebar(dx, y, button) => {
                let x = sidebar_right(&profile.layout) + dx;
                mouse(&mut cx, window, point(px(x), px(y)), button);
            }
            AtWindow(fx, from_bottom, button) => mouse(&mut cx, window, point(px(width * fx), px(height - from_bottom)), Some(button)),
            ScrollPages(id, pages) => {
                let b = bounds(&mut cx, window, id.into())?;
                wheel(&mut cx, window, b.center(), -pages * (f32::from(b.size.height) - 60.));
            }
            ConversationTop => {
                let sidebar = sidebar_right(&profile.layout);
                wheel(&mut cx, window, point(px(sidebar + (width - sidebar) / 2.), px(height / 2.)), 100_000.);
            }
            Root(call) => {
                cx.update_window(window, |_, w, cx| call(&root, w, cx)).ok();
            }
            Settings(call) => {
                let settings = cx.update(|cx| shell.read(cx).settings().cloned()).context("no settings modal open")?;
                cx.update_window(window, |_, w, cx| call(&settings, w, cx)).ok();
            }
            Type(text) => {
                cx.update_window(window, |_, w, cx| w.input(text, cx)).ok();
            }
            Press(keys) => {
                let keys = keys.replace("mod-", if cfg!(target_os = "macos") { "cmd-" } else { "ctrl-" });
                cx.update_window(window, |_, w, cx| w.press(&keys, cx)).ok();
            }
            Dispatch(action) => {
                cx.update_window(window, |_, w, cx| w.dispatch_action(action(), cx)).ok();
            }
            Navigate(path) => cx.update(|cx| root.update(cx, |root, cx| root.navigate(Route::parse(path), cx))),
            Thread(title) => {
                let key = threads.get(title).with_context(|| format!("no thread titled {title}"))?.clone();
                cx.update(|cx| root.update(cx, |root, cx| root.navigate(route(&key), cx)));
            }
            ServerPages => cx.update(|cx| {
                if let Some(settings) = shell.read(cx).settings().cloned() {
                    let (model, _) = t3code_web::scenes::settings_server::fixture_model(cx);
                    let pages = cx.new(|_| t3code_web::components::settings::server_settings_pages::ServerSettingsPages::new(model));
                    settings.update(cx, |s, cx| s.set_server_pages(pages, cx));
                }
            }),
            Run(run) => cx.update(run),
            Wait(ms) => {
                let until = Instant::now() + Duration::from_millis(ms);
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
    let file = out.join(format!("{name}.png"));
    cx.capture_screenshot(window)?.save(&file)?;
    println!("{} <- ref/{name}.png", file.display());
    Ok(())
}

/// `parity-shot --mac <out-dir> [scene-filter]`.
pub fn main(mut args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let out = PathBuf::from(args.next().context("usage: parity-shot --mac <out-dir> [scene-filter]")?);
    let filter = args.next();
    let settings = std::env::var_os("PARITY_CLIENT_SETTINGS").map(PathBuf::from).context("PARITY_CLIENT_SETTINGS must name the client settings file")?;
    let layout = Layout::load(std::env::var_os("PARITY_LAYOUT").map(PathBuf::from).as_deref())?;
    let size = match std::env::var("PARITY_SIZE") {
        Ok(v) => {
            let (w, h) = v.split_once('x').context("PARITY_SIZE is <w>x<h>")?;
            (w.parse()?, h.parse()?)
        }
        Err(_) => (1710., 1074.),
    };
    eprintln!(
        "owner profile: {}x{}, sidebar width {:?}, open {:?}, shelves working {:?} snoozed {:?} settled {:?}",
        size.0, size.1, layout.sidebar_width, layout.sidebar_open, layout.working, layout.snoozed, layout.settled
    );
    let profile = Profile { size, settings, layout };
    std::fs::create_dir_all(&out)?;
    let data = out.join(".runtime");
    std::fs::create_dir_all(&data)?;
    let mut failed = 0;
    for (name, steps) in all_scenes() {
        if filter.as_deref().is_some_and(|f| name != f && !(f.ends_with('*') && name.starts_with(f.trim_end_matches('*')))) {
            continue;
        }
        if let Err(error) = render(&name, &steps, &profile, &out, &data) {
            eprintln!("FAILED {name}: {error:#}");
            failed += 1;
        }
    }
    std::fs::remove_dir_all(&data).ok();
    anyhow::ensure!(failed == 0, "{failed} scenes failed");
    Ok(())
}
