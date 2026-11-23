//! The sidebar, thread view and composer on live server state, with no fixtures. Starts the
//! connection runtime on `T3_LIVE_DATA_DIR` (secrets in a file there, no T3 Connect) when no
//! runtime runs yet, then the [`StateBridge`]. Pair an environment into that data directory
//! first (`live-check` does).
//!
//! [`LiveApp`] is also the reference for wiring the bridge into the assembled window: sidebar
//! events go through [`sidebar_actions::handle`], the open thread's events patch the timeline,
//! composer events go through [`composer::handle_event`].

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Window, div, px,
};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_contracts::ProviderApprovalDecision;

use crate::components::app_sidebar_layout::{AppSidebarLayout, TitlebarPlatform};
use crate::components::chat::composer::{ChatComposer, ComposerEvent};
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::chat_view::{ChatView, chat_view};
use crate::components::sidebar::model::ThreadKey;
use crate::components::sidebar::{Sidebar, SidebarEvent};
use crate::components::ui::toast::Toaster;
use crate::composer_draft_store::ComposerDraftStore;
use crate::connection::{ConnectionRuntime, RuntimeConfig};
use crate::state::composer::{self, CatalogCache, LiveDraftStore, LiveMenuSource, composer_state};
use crate::state::palette::LivePaletteSource;
use crate::state::shell::runtime_ids;
use crate::state::sidebar_actions::{self, SidebarOutcome};
use crate::state::{LiveEnvironment, StateBridge};
use crate::thread_sync::{LiveThread, LiveThreadEvent};
use crate::{ActiveAppTheme as _, i18n::t};

/// Where the live scene keeps its catalog, secrets and caches.
pub fn data_dir() -> PathBuf {
    std::env::var_os("T3_LIVE_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("t3code-gpui-live"))
}

/// Starts the connection runtime and the bridge if they are not running.
pub fn ensure_runtime(cx: &mut App) {
    if !ConnectionRuntime::is_running(cx) {
        let data_dir = data_dir();
        let config = RuntimeConfig {
            secrets: Some(Arc::new(FileSecretStore::new(data_dir.join("secrets.json")))),
            data_dir,
            cloud: None,
            presentation: ClientPresentation { label: "t3code-gpui live".into(), ..Default::default() },
            electron: None,
        };
        if let Err(error) = ConnectionRuntime::init(config, cx) {
            eprintln!("could not start the connection runtime: {error:#}");
            return;
        }
    }
    if !StateBridge::is_running(cx) {
        StateBridge::init(cx);
    }
}

/// When the last click was dispatched, for click-to-frame measurements.
#[derive(Default, Clone, Copy)]
pub struct SwitchTiming {
    pub clicked_at: Option<Instant>,
    /// Time inside the click handler: open, map and hand the timeline its input.
    pub handler_micros: u64,
    /// Parts of it: opening the thread in the bridge, the timeline's `set_thread`, and the
    /// composer's state.
    pub open_micros: u64,
    pub timeline_micros: u64,
    pub composer_micros: u64,
}

/// The live window content.
pub struct LiveApp {
    layout: Entity<AppSidebarLayout>,
    sidebar: Entity<Sidebar>,
    chat: Entity<ChatView>,
    composer: Option<Entity<ChatComposer>>,
    menu_source: Option<Rc<LiveMenuSource>>,
    draft_store: Rc<RefCell<dyn ComposerDraftStore>>,
    thread: Option<Entity<LiveThread>>,
    environment: Option<Entity<LiveEnvironment>>,
    catalog: CatalogCache,
    palette: Rc<LivePaletteSource>,
    rename: Option<(ThreadKey, Entity<InputState>, Subscription)>,
    timing: SwitchTiming,
    /// Text-only and structure updates handed to the timeline, for measurements.
    pub text_updates: u64,
    pub structure_updates: u64,
    thread_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl LiveApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ensure_runtime(cx);
        let shell = StateBridge::shell(cx);
        let snapshot = shell.read(cx).snapshot().clone();
        let sidebar = cx.new(|cx| Sidebar::new(snapshot, window, cx));
        let layout = cx.new(|cx| AppSidebarLayout::new(sidebar.clone(), TitlebarPlatform::Overlay, cx));
        let chat = chat_view(cx);
        layout.update(cx, |layout, cx| layout.set_main(chat.clone().into(), cx));
        let palette = LivePaletteSource::new();
        let subscriptions = vec![
            cx.observe(&shell, |this, shell, cx| {
                let snapshot = shell.read(cx).snapshot().clone();
                this.sidebar.update(cx, |sidebar, cx| sidebar.set_snapshot(snapshot, cx));
                this.palette.refresh(shell.read(cx), cx);
                this.refresh_composer(cx);
            }),
            cx.subscribe_in(&sidebar, window, |this, _, event: &SidebarEvent, window, cx| this.on_sidebar(event, window, cx)),
            cx.subscribe(&chat.read(cx).timeline().clone(), |this, _, action: &TimelineAction, cx| this.on_timeline(action, cx)),
        ];
        let draft_store: Rc<RefCell<dyn ComposerDraftStore>> = Rc::new(RefCell::new(LiveDraftStore::new(StateBridge::drafts(cx))));
        Self {
            layout,
            sidebar,
            chat,
            composer: None,
            menu_source: None,
            draft_store,
            thread: None,
            environment: None,
            catalog: CatalogCache::default(),
            palette,
            rename: None,
            timing: SwitchTiming::default(),
            text_updates: 0,
            structure_updates: 0,
            thread_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub fn chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    pub fn composer(&self) -> Option<&Entity<ChatComposer>> {
        self.composer.as_ref()
    }

    pub fn thread(&self) -> Option<&Entity<LiveThread>> {
        self.thread.as_ref()
    }

    pub fn palette_source(&self) -> &Rc<LivePaletteSource> {
        &self.palette
    }

    pub fn timing(&self) -> SwitchTiming {
        self.timing
    }

    /// The rename field, while open.
    pub fn rename_field(&self) -> Option<&Entity<InputState>> {
        self.rename.as_ref().map(|(_, field, _)| field)
    }

    fn on_sidebar(&mut self, event: &SidebarEvent, window: &mut Window, cx: &mut Context<Self>) {
        let started = Instant::now();
        match sidebar_actions::handle(event, cx) {
            SidebarOutcome::Opened(_) => {
                let open_micros = started.elapsed().as_micros() as u64;
                self.show_active_thread(window, cx);
                self.timing = SwitchTiming {
                    clicked_at: Some(started),
                    handler_micros: started.elapsed().as_micros() as u64,
                    open_micros,
                    ..self.timing
                };
            }
            SidebarOutcome::Rename(key) => self.open_rename(key, window, cx),
            _ => {}
        }
    }

    fn on_timeline(&mut self, action: &TimelineAction, cx: &mut Context<Self>) {
        let Some(thread) = &self.thread else { return };
        let key = thread.read(cx).key().clone();
        match action {
            TimelineAction::RespondToApproval { request, approve } => {
                let decision = if *approve { ProviderApprovalDecision::Accept } else { ProviderApprovalDecision::Decline };
                composer::respond_to_approval(&key, request.as_str(), decision, cx)
            }
            _ => {}
        }
    }

    fn open_rename(&mut self, key: ThreadKey, window: &mut Window, cx: &mut Context<Self>) {
        let title = StateBridge::shell(cx).read(cx).snapshot().threads.iter().find(|t| t.key == key).map(|t| t.title.clone()).unwrap_or_default();
        let field = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let subscription = cx.subscribe(&field, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.confirm_rename(cx);
            }
        });
        self.rename = Some((key, field, subscription));
        cx.notify();
    }

    /// Sends the rename field's title, if it holds one, and closes it.
    pub fn confirm_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((key, field, _)) = self.rename.take() {
            let title = field.read(cx).value().to_string();
            if !title.trim().is_empty() {
                sidebar_actions::rename(&key, title.trim().to_owned(), cx);
            }
        }
        cx.notify();
    }

    /// Shows the thread [`ThreadViews`](crate::state::threads::ThreadViews) has open: its
    /// timeline in this frame, then live updates.
    pub fn show_active_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = StateBridge::threads(cx).read(cx).active().cloned() else { return };
        let key = thread.read(cx).key().clone();
        let (environment_id, _) = runtime_ids(&key);
        let environment = StateBridge::environment(&environment_id, cx);
        let shell = StateBridge::shell(cx);
        let (project, title) = {
            let shell = shell.read(cx);
            let row = shell.snapshot().threads.iter().find(|t| t.key == key).cloned();
            let project = row
                .as_ref()
                .and_then(|row| shell.snapshot().projects.iter().find(|p| p.project_ref == row.project_ref()))
                .map(|p| p.title.clone())
                .unwrap_or_default();
            (project, row.map(|row| row.title.clone()).unwrap_or_default())
        };
        let input = thread.read(cx).input().clone();
        let thread_key: SharedString = format!("{}/{}", key.environment_id.0, key.thread_id.0).into();
        let timeline_started = Instant::now();
        self.chat.update(cx, |chat, cx| chat.set_thread(thread_key, project, title, input, cx));
        self.timing.timeline_micros = timeline_started.elapsed().as_micros() as u64;
        self.thread_subscriptions = vec![cx.subscribe(&thread, |this, thread, event: &LiveThreadEvent, cx| {
            let timeline = this.chat.read(cx).timeline().clone();
            match event {
                LiveThreadEvent::Structure => {
                    this.structure_updates += 1;
                    let input = thread.read(cx).input().clone();
                    timeline.update(cx, |timeline, cx| timeline.update_input(input, cx));
                    this.refresh_composer(cx);
                }
                LiveThreadEvent::Text(changes) => {
                    this.text_updates += 1;
                    timeline.update(cx, |timeline, cx| {
                        for (id, text, streaming) in changes.iter() {
                            timeline.update_message_text(id, text.clone(), *streaming, cx);
                        }
                    });
                }
                LiveThreadEvent::Status => {}
            }
        })];
        if let Some(environment) = &environment {
            self.thread_subscriptions.push(cx.observe(environment, |this, environment, cx| {
                if let Some(source) = &this.menu_source {
                    source.set_config(environment.read(cx).config.as_deref());
                }
                this.refresh_composer(cx);
            }));
        }
        self.thread = Some(thread);
        self.environment = environment;
        let composer_started = Instant::now();
        self.mount_composer(window, cx);
        self.timing.composer_micros = composer_started.elapsed().as_micros() as u64;
        cx.notify();
    }

    fn composer_state(&mut self, cx: &App) -> Option<crate::components::chat::composer::ComposerState> {
        let thread = self.thread.as_ref()?.read(cx);
        let environment = self.environment.as_ref()?.read(cx);
        let catalog = self.catalog.get(environment.config.as_ref());
        let shell = StateBridge::shell(cx);
        let row = shell.read(cx).row(thread.key()).cloned();
        Some(composer_state(thread.key(), row.as_deref(), thread, environment, catalog))
    }

    fn refresh_composer(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.composer_state(cx) else { return };
        if let Some(composer) = &self.composer {
            composer.update(cx, |composer, cx| composer.set_state(state, cx));
        }
    }

    fn mount_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.composer_state(cx) else { return };
        if let Some(composer) = &self.composer {
            composer.update(cx, |composer, cx| composer.set_state(state, cx));
            return;
        }
        let Some(environment) = self.environment.clone() else { return };
        let cwd = environment
            .read(cx)
            .shell
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.projects.first().map(|p| p.workspace_root.clone()));
        let draft_store = Rc::clone(&self.draft_store);
        let source = LiveMenuSource::new(&environment, cwd, cx);
        let menu: Rc<dyn crate::components::chat::composer_command_menu::ComposerMenuSource> = source.clone();
        let composer = cx.new(|cx| ChatComposer::new(state, menu, draft_store, cx));
        source.attach(&composer);
        let weak = composer.downgrade();
        self._subscriptions.push(cx.subscribe(&composer, move |this, _, event: &ComposerEvent, cx| {
            if let Some(thread) = &this.thread {
                composer::handle_event(event, thread, &weak, cx);
            }
        }));
        let focus = composer.focus_handle(cx);
        self.chat.update(cx, |chat, cx| chat.set_composer(composer.clone().into(), focus, cx));
        let focus = composer.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.menu_source = Some(source);
        self.composer = Some(composer);
    }
}

impl Render for LiveApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors.clone();
        div()
            .size_full()
            .relative()
            .bg(colors.canvas)
            .child(self.layout.clone())
            .child(Toaster::view(cx))
            .children(self.rename.as_ref().map(|(_, field, _)| {
                div()
                    .absolute()
                    .top(px(80.))
                    .left(px(360.))
                    .w(px(420.))
                    .p_3()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.surface_overlay)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(colors.foreground()).child(t!("Rename thread")))
                    .child(Input::new(field))
            }))
    }
}

/// The scene: the live app on `T3_LIVE_DATA_DIR`.
pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| LiveApp::new(window, cx)).into()
}
