//! Walks every bindable command, palette event, sidebar project and header thread menu row
//! against the real window root (`WindowRoot`: shortcut root, environment shell, `AppRoot`) on
//! the Mac reference world, and fails for any that reaches no handler.
//!
//! A command counts as handled when some element on the focus path of the place it is used
//! (the composer of a shown thread, a focused terminal, the open model picker, the Usage page, a
//! browser preview tab) handles its action: the same test GPUI's key dispatch applies. An app
//! event counts as handled when the app's handler reports [`Wiring::Ran`]; one that belongs to a
//! part of the app another package builds reports [`Wiring::Unavailable`] and tells the user.
//! Writes `wiring.txt` to the out dir.
//!
//!   cargo run -p t3code-web --features test-support --example command_wiring_check -- <out dir>

use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_shared::keybindings::{KeybindingCommand, ScriptId, StaticCommand};
use t3code_web::app_root::{AppRootOptions, DataSource, WindowRoot, Wiring};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::chat_view::ChatViewEvent;
use t3code_web::components::command_palette_logic::{AddProjectStep, PaletteEvent, PortForwardRequest};
use t3code_web::components::preview::preview_source::FixturePreviewSource;
use t3code_web::components::preview::preview_view::PreviewView;
use t3code_web::components::sidebar::SidebarEvent;
use t3code_web::components::sidebar::context_menu::{MenuCommand, ThreadMenuState, project_menu_items, thread_menu_items};
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::fixtures::FixtureStore;
use t3code_web::router::Route;
use t3code_web::shortcuts::commands::{ToggleTerminal, command_action, command_title, pending_command_owner};
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};
use t3code_web::ui_state_store::StateDir;

/// Where a command is used, so where its handler must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Composer,
    Terminal,
    ModelPicker,
    Usage,
    Preview,
    PullRequests,
}

fn place(command: StaticCommand) -> Place {
    use StaticCommand as C;
    match command {
        C::TerminalSplit | C::TerminalSplitVertical | C::TerminalNew | C::TerminalClose => Place::Terminal,
        C::ModelPickerPreviousProvider
        | C::ModelPickerNextProvider
        | C::ModelPickerJump1
        | C::ModelPickerJump2
        | C::ModelPickerJump3
        | C::ModelPickerJump4
        | C::ModelPickerJump5
        | C::ModelPickerJump6
        | C::ModelPickerJump7
        | C::ModelPickerJump8
        | C::ModelPickerJump9 => Place::ModelPicker,
        C::UsageCost | C::UsageTokens | C::UsageLimits | C::UsagePeriodDay | C::UsagePeriodWeek | C::UsagePeriodMonth | C::UsagePeriodQuarter => {
            Place::Usage
        }
        C::PreviewRefresh | C::PreviewFocusUrl | C::PreviewZoomIn | C::PreviewZoomOut | C::PreviewResetZoom | C::PreviewFind => Place::Preview,
        _ => Place::Composer,
    }
}

struct Check {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    parts: WindowRoot,
    log: String,
    /// Reached nothing, with no owner named.
    unhandled: Vec<String>,
    /// Reached a notice that another package builds it.
    left: Vec<String>,
}

impl Check {
    fn frame(&mut self) {
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
        self.cx.run_until_parked();
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
    }

    fn note(&mut self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
        let _ = writeln!(self.log, "{}", line.as_ref());
    }

    fn record(&mut self, what: String, wiring: Wiring) {
        match wiring {
            Wiring::Ran => self.note(format!("ok         {what}")),
            Wiring::Unavailable(owner) => {
                self.note(format!("LEFT       {what}  (owner: {owner})"));
                self.left.push(format!("{what} ({owner})"));
            }
            Wiring::Dropped => {
                self.note(format!("UNHANDLED  {what}"));
                self.unhandled.push(what);
            }
        }
    }

    /// Focuses the composer of the shown thread.
    fn focus_composer(&mut self) -> FocusHandle {
        let app = self.parts.app.clone();
        let focus = self.cx.update(|cx| app.read(cx).composer(cx).focus_handle(cx));
        self.cx.update_window(self.window, |_, w, cx| w.focus(&focus, cx)).ok();
        self.frame();
        focus
    }

    fn available(&mut self, action: &dyn Action, focus: &FocusHandle) -> bool {
        self.cx.update_window(self.window, |_, w, _| w.is_action_available_in(action, focus)).unwrap_or(false)
    }
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).context("usage: command_wiring_check <out dir>")?);
    std::fs::create_dir_all(&out)?;
    let data = out.join("data");
    std::fs::create_dir_all(&data)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
        data_dir: data,
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui wiring check".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
        ConnectionRuntime::init(config, cx)
    })?;
    let options = AppRootOptions { data: DataSource::Fixtures(FixtureStore::reference()), platform: TitlebarPlatform::Overlay, state_dir: StateDir(None) };
    let (window, parts) = cx.update(|cx| {
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(1710.), px(1074.)), cx)),
            show: false,
            focus: true,
            ..Default::default()
        };
        WindowRoot::open_window(window_options, options, cx)
    })?;
    let mut c = Check { cx, window, parts, log: String::new(), unhandled: Vec::new(), left: Vec::new() };
    c.frame();

    // A server thread on screen, its project and environment.
    let app = c.parts.app.clone();
    let snapshot = c.cx.update(|cx| app.read(cx).data().snapshot(None, cx));
    let thread = snapshot.threads.iter().find(|t| !t.archived).context("a thread")?.clone();
    let key = thread.key.clone();
    let project = thread.project_ref();
    let environment = project.environment_id.clone();
    c.cx.update(|cx| { let router = app.read(cx).router().clone(); router.update(cx, |r, cx| r.navigate(Route::Thread(key.clone()), cx)) });
    c.frame();
    c.note(format!("== thread on screen: {}", thread.title));

    c.note("== commands, at the place each is used");
    let composer = c.focus_composer();
    let mut commands: Vec<KeybindingCommand> = StaticCommand::ALL.iter().copied().map(KeybindingCommand::Static).collect();
    commands.push(KeybindingCommand::Script(ScriptId::new("test").expect("script id")));
    for command in &commands {
        let at = match command {
            KeybindingCommand::Static(command) => place(*command),
            KeybindingCommand::Script(_) => Place::Composer,
        };
        let focus = match at {
            Place::Composer => Some(composer.clone()),
            Place::Terminal => {
                c.cx.update_window(c.window, |_, w, cx| w.dispatch_action(Box::new(ToggleTerminal), cx)).ok();
                c.frame();
                let terminal_key = t3code_web::terminal_ui_state_store::ThreadKey(t3code_web::fixtures::scoped_key(&key).to_string());
                let focus = c.cx.update(|cx| app.read(cx).terminals().read(cx).drawer(&terminal_key).map(|d| d.focus_handle(cx)));
                focus
            }
            Place::ModelPicker => {
                let composer_view = c.cx.update(|cx| app.read(cx).composer(cx));
                c.cx.update_window(c.window, |_, w, cx| composer_view.update(cx, |composer, cx| composer.open_picker(w, cx))).ok();
                c.frame();
                let focus = c.cx.update(|cx| composer_view.read(cx).picker().map(|p| p.focus_handle(cx)));
                focus
            }
            Place::Usage => {
                c.cx.update(|cx| { let router = app.read(cx).router().clone(); router.update(cx, |r, cx| r.navigate(Route::Usage, cx)) });
                c.frame();
                let focus = c.cx.update(|cx| app.read(cx).usage_route().map(|u| u.focus_handle(cx)));
                focus
            }
            Place::Preview => None,
            Place::PullRequests => {
                c.cx.update(|cx| { let router = app.read(cx).router().clone(); router.update(cx, |r, cx| r.navigate(Route::parse("/pull-requests"), cx)) });
                c.frame();
                let focus = c.cx.update_window(c.window, |_, w, cx| w.focused(cx)).ok().flatten();
                focus
            }
        };
        let action = command_action(command);
        let title = format!("{command} ({})", command_title(command));
        let wiring = match (at, focus) {
            (Place::Preview, _) => preview_has_handler(action.as_ref(), &mut c)?,
            (_, Some(focus)) if c.available(action.as_ref(), &focus) => match command {
                KeybindingCommand::Static(command) => pending_command_owner(*command).map_or(Wiring::Ran, Wiring::Unavailable),
                KeybindingCommand::Script(_) => Wiring::Ran,
            },
            (_, Some(_)) => Wiring::Dropped,
            (_, None) => {
                c.note(format!("NO PLACE   {title}: could not reach {at:?}"));
                Wiring::Dropped
            }
        };
        c.record(format!("command {title} at {at:?}"), wiring);
        if at == Place::Terminal {
            c.cx.update_window(c.window, |_, w, cx| w.dispatch_action(Box::new(ToggleTerminal), cx)).ok();
        }
        if at != Place::Composer {
            c.cx.update(|cx| { let router = app.read(cx).router().clone(); router.update(cx, |r, cx| r.navigate(Route::Thread(key.clone()), cx)) });
            c.frame();
            c.focus_composer();
        }
    }

    c.note("== palette events");
    let events = [
        PaletteEvent::NewThreadIn(project.clone()),
        PaletteEvent::NewThreadWithoutProject(environment.clone()),
        PaletteEvent::Triage,
        PaletteEvent::CopyPromptLink { with_context: false },
        PaletteEvent::CopyPromptLink { with_context: true },
        PaletteEvent::LinkPullRequest,
        PaletteEvent::ShowLinkedItems,
        PaletteEvent::ShareWithGuest,
        PaletteEvent::RestartAgentSession,
        PaletteEvent::NewProject,
        PaletteEvent::AddProject(AddProjectStep::ConnectEnvironment),
        PaletteEvent::AddProject(AddProjectStep::BrowseFolders(environment.clone())),
        PaletteEvent::AddProject(AddProjectStep::CloneRepository(environment.clone())),
        PaletteEvent::OpenPullRequests,
        PaletteEvent::OpenIssues,
        PaletteEvent::UseInAnotherApp,
        PaletteEvent::OpenSettings,
        PaletteEvent::OpenSetting("appearance".into()),
        PaletteEvent::ToggleProjectPin(project.clone()),
        PaletteEvent::OpenProjectSettings(project.clone()),
        PaletteEvent::PortForward(PortForwardRequest::Start { environment_id: environment.clone(), port: 5173 }),
        PaletteEvent::PortForward(PortForwardRequest::Open { environment_id: environment.clone(), port: 5173 }),
        PaletteEvent::PortForward(PortForwardRequest::CopyAddress { environment_id: environment.clone(), port: 5173 }),
        PaletteEvent::PortForward(PortForwardRequest::Stop { environment_id: environment.clone(), port: 5173 }),
        PaletteEvent::OpenThread { thread: key.clone(), query: "".into() },
    ];
    for event in events {
        let wiring = c.cx.update_window(c.window, |_, w, cx| app.update(cx, |app, cx| app.run_palette_event(&event, w, cx))).unwrap();
        c.frame();
        c.record(format!("palette {event:?}"), wiring);
        reset(&mut c, &key);
    }

    c.note("== sidebar: project menu and Add project");
    // `OpenProjectFilter` is left out: nothing emits it (rows filter by project in the sidebar).
    let mut sidebar_events = vec![SidebarEvent::AddProject];
    for pinned in [false, true] {
        for item in project_menu_items(pinned) {
            if let Some(command) = item.command {
                sidebar_events.push(SidebarEvent::ProjectCommand(project.clone(), command));
            }
        }
    }
    for event in sidebar_events {
        let wiring = c.cx.update_window(c.window, |_, w, cx| app.update(cx, |app, cx| app.run_sidebar_event(&event, w, cx))).unwrap();
        c.frame();
        c.record(format!("sidebar {event:?}"), wiring);
        reset(&mut c, &key);
    }

    c.note("== chat header \"Thread actions\" menu");
    let presets = [];
    let state = ThreadMenuState {
        branch: Some("main"),
        project_label: "project",
        is_pinned: false,
        is_settled: false,
        is_snoozed: false,
        is_running: false,
        can_snooze_now: true,
        project_filter_active: false,
        show_project_filter: false,
        can_find: true,
        auto_settle_enabled: true,
        snooze_presets: &presets,
    };
    let mut header: Vec<MenuCommand> = Vec::new();
    for item in thread_menu_items(&state) {
        header.extend(item.command);
        header.extend(item.children.iter().filter_map(|child| child.command));
    }
    header.dedup();
    let thread_view = c.cx.update(|cx| app.read(cx).thread_view().clone());
    for command in header.into_iter().filter(|c| *c != MenuCommand::FindInThread) {
        let event = ChatViewEvent::ThreadCommand(key.clone(), command);
        let wiring = c.cx.update_window(c.window, |_, w, cx| thread_view.update(cx, |view, cx| view.run_chat_event(&event, w, cx))).unwrap();
        c.frame();
        c.record(format!("header {command:?}"), wiring);
        reset(&mut c, &key);
    }

    c.note(format!("== unhandled: {}", c.unhandled.len()));
    for line in c.unhandled.clone() {
        c.note(format!("  {line}"));
    }
    c.note(format!("== left for other packages: {}", c.left.len()));
    for line in c.left.clone() {
        c.note(format!("  {line}"));
    }
    std::fs::write(out.join("wiring.txt"), &c.log)?;
    if !c.unhandled.is_empty() {
        bail!("{} commands or events reach no handler", c.unhandled.len());
    }
    Ok(())
}

/// Back on the thread with the composer focused, after an event that navigated or opened a dialog.
fn reset(c: &mut Check, key: &t3code_web::components::sidebar::model::ThreadKey) {
    let app = c.parts.app.clone();
    c.cx.update(|cx| { let router = app.read(cx).router().clone(); router.update(cx, |r, cx| r.navigate(Route::Thread(key.clone()), cx)) });
    c.frame();
    c.focus_composer();
}

/// Whether a browser preview tab handles `action` (the preview's own commands).
fn preview_has_handler(action: &dyn Action, c: &mut Check) -> anyhow::Result<Wiring> {
    let mut found = None;
    let (handle, _) = c.cx.update(|cx| {
        let options = WindowOptions { window_bounds: Some(WindowBounds::centered(None, size(px(800.), px(600.)), cx)), show: false, focus: true, ..Default::default() };
        gpui_kit::open_window(options, cx, |window, cx| {
            let source = FixturePreviewSource::new(Vec::new());
            let view = cx.new(|cx| PreviewView::new(source as Rc<_>, t3code_contracts::ThreadId::new("wiring"), "wiring".into(), None, window, cx));
            found = Some(view.focus_handle(cx));
            view
        })
    })?;
    let focus = found.context("preview view")?;
    c.cx.update_window(handle, |_, w, cx| {
        w.focus(&focus, cx);
        w.render_frame(cx);
    })
    .ok();
    let available = c.cx.update_window(handle, |_, w, _| w.is_action_available_in(action, &focus)).unwrap_or(false);
    c.cx.update_window(handle, |_, w, _| w.remove_window()).ok();
    c.cx.run_until_parked();
    Ok(if available { Wiring::Ran } else { Wiring::Dropped })
}
