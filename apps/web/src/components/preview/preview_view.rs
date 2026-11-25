//! One browser tab of the right panel, streamed from the server. Ports the streamed parts
//! of `PreviewPanel.tsx`, `PreviewView.tsx` and `PreviewChromeRow.tsx`: the address bar
//! with back, forward and reload, the loading bar, "Float preview over chat", the More
//! menu, the live surface, the empty state, a page that did not load, and the panel a
//! session without preview access sees. The right panel's tab strip names the tab
//! ([`PreviewView::tab_title`], [`PreviewView::tab_url`]).

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use futures::StreamExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription, Window,
    div, prelude::*, px, relative,
};
use t3code_client_runtime::preview::server_browser_stream::{Controller, PreviewStreamInput};
use t3code_contracts::{
    DiscoveredLocalServer, EnvironmentId, PreviewAdjustInput, PreviewAdjustInputClear, PreviewAppearancePreference, PreviewCloseInput,
    PreviewEvent, PreviewNavStatus, PreviewOpenInput, PreviewResizeInput, PreviewRuntime, PreviewSessionSnapshot, PreviewViewportSetting,
    ThreadId,
};

use super::preview_more_menu::{PreviewMoreMenu, PreviewMoreMenuActions, PreviewMoreMenuState};
use super::preview_source::PreviewSource;
use super::preview_unreachable::{PreviewFileNotShown, PreviewUnreachable};
use super::thread_preview_mini_player::{MiniPlayerKey, PreviewMiniPlayers};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::browser::server_browser_surface::{ServerBrowserSurface, SurfaceEvent};
use crate::browser_history_store::{BrowserHistory, normalize_history_url};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::motion::Transition;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;
use crate::shortcuts::commands::{FindInPreview, FocusPreviewUrl, RefreshPreview, ResetPreviewZoom, ZoomInPreview, ZoomOutPreview};
use crate::theme_color::with_alpha;
use crate::theme_tokens::CubicBezier;

/// `PREVIEW_ZOOM_LEVELS`.
const ZOOM_LEVELS: [f64; 17] = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];
/// `BROWSER_DEVICE_TOOLBAR_HEIGHT` and `BROWSER_VIEWPORT_RESIZE_RAIL_SIZE`.
const DEVICE_TOOLBAR_HEIGHT: f32 = 38.;
const RESIZE_RAIL: f32 = 10.;
/// `PREVIEW_VIEWPORT_MIN_DIMENSION`, `_MAX_DIMENSION` and `_MAX_AREA`.
const VIEWPORT_MIN: f32 = 240.;
const VIEWPORT_MAX: f32 = 3840.;
const VIEWPORT_MAX_AREA: f32 = 3840. * 2160.;
/// `.preview-loading-progress[data-loading=true]`: 0.04 to 0.9 of the width over 5.3s.
const LOADING: Transition = Transition::new(5300, CubicBezier(0.1, 0.5, 0.2, 1.));
/// The bar fills in 150ms when loading ends, then fades for 150ms after 220ms.
const LOADED: Transition = Transition::new(370, CubicBezier(0., 0., 1., 1.));
/// "Default" and "Incognito" (`BUILT_IN_BROWSER_PROFILES`).
const DEFAULT_PROFILE: &str = "default";
const INCOGNITO_PROFILE: &str = "incognito";

/// The tab's address, or empty while it is idle.
pub fn nav_url(snapshot: &PreviewSessionSnapshot) -> &str {
    match &snapshot.nav_status {
        PreviewNavStatus::Loading(s) => &s.url,
        PreviewNavStatus::Success(s) => &s.url,
        PreviewNavStatus::LoadFailed(s) => &s.url,
        _ => "",
    }
}

/// The right panel's tab title (`surfaceTitle` for `preview`): the page title, else the
/// host, else "Browser".
pub fn nav_title(snapshot: &PreviewSessionSnapshot) -> SharedString {
    let title = match &snapshot.nav_status {
        PreviewNavStatus::Loading(s) => s.title.as_str(),
        PreviewNavStatus::Success(s) => s.title.as_str(),
        PreviewNavStatus::LoadFailed(s) => s.title.as_str(),
        _ => return t!("Browser"),
    };
    if !title.trim().is_empty() {
        return title.to_owned().into();
    }
    url::Url::parse(nav_url(snapshot))
        .ok()
        .and_then(|u| u.host_str().map(|h| match u.port() {
            Some(port) => format!("{h}:{port}"),
            None => h.to_owned(),
        }))
        .filter(|h| !h.is_empty())
        .map_or_else(|| t!("Browser"), SharedString::from)
}

fn is_idle(snapshot: &PreviewSessionSnapshot) -> bool {
    !matches!(snapshot.nav_status, PreviewNavStatus::Loading(_) | PreviewNavStatus::Success(_) | PreviewNavStatus::LoadFailed(_))
}

/// `resolveResponsiveBrowserViewportSize`: the freeform size "Show device toolbar" starts
/// at, filling the pane inside the toolbar and resize rails.
fn responsive_viewport(width: f32, height: f32, zoom: f64) -> PreviewViewportSetting {
    let zoom = if zoom.is_finite() && zoom > 0. { zoom as f32 } else { 1. };
    let area = ((width - RESIZE_RAIL * 2.).max(1.) / zoom, (height - DEVICE_TOOLBAR_HEIGHT - RESIZE_RAIL).max(1.) / zoom);
    let clamp = |v: f32| v.round().clamp(VIEWPORT_MIN, VIEWPORT_MAX);
    let (mut w, mut h) = (clamp(area.0), clamp(area.1));
    if w * h > VIEWPORT_MAX_AREA {
        w = (VIEWPORT_MAX_AREA / h).floor().max(VIEWPORT_MIN);
        h = h.min(VIEWPORT_MAX);
    }
    PreviewViewportSetting::Freeform(t3code_contracts::PreviewViewportSettingFreeform { width: w as u64, height: h as u64 })
}

/// What the panel tells the right panel.
#[derive(Clone, Debug)]
pub enum PreviewViewEvent {
    /// The tab's title or address changed.
    TabChanged,
    /// "Float preview over chat" opened the mini player: close the panel.
    Floated,
}

/// The loading bar's run: when the current load started, and when the last one ended.
#[derive(Default)]
struct LoadingBar {
    generation: u64,
    started: Option<Instant>,
    /// The fill level and the run it ends, for the finishing animation.
    ended: Option<(u64, f32)>,
}

impl LoadingBar {
    fn level(started: Instant) -> f32 {
        let t = (started.elapsed().as_secs_f32() / LOADING.duration.as_secs_f32()).min(1.);
        0.04 + 0.86 * LOADING.easing.ease(t)
    }

    /// Follows `loading` and returns the bar to draw this frame.
    fn follow(&mut self, loading: bool) {
        match (loading, self.started) {
            (true, None) => {
                self.generation += 1;
                self.started = Some(Instant::now());
                self.ended = None;
            }
            (false, Some(started)) => {
                self.ended = Some((self.generation, Self::level(started)));
                self.started = None;
            }
            _ => {}
        }
    }
}

/// The browser panel for one tab of a thread.
pub struct PreviewView {
    source: Rc<dyn PreviewSource>,
    thread_id: ThreadId,
    environment: EnvironmentId,
    /// History is keyed by the thread's project.
    project_key: String,
    history_path: Option<PathBuf>,
    history: BrowserHistory,
    /// The right panel surface's tab; `None` shows the thread's newest tab.
    requested: Option<String>,
    sessions: Vec<PreviewSessionSnapshot>,
    active: Option<String>,
    surfaces: Vec<(String, Entity<ServerBrowserSurface>)>,
    address: Entity<InputState>,
    editing: bool,
    servers: Vec<DiscoveredLocalServer>,
    /// The session's `preview:operate` grant; fixtures have it.
    can_operate: bool,
    show_details: bool,
    visible: bool,
    loading_bar: LoadingBar,
    /// The pane's size in window pixels, for "Show device toolbar".
    pane_size: Rc<std::cell::Cell<(f32, f32)>>,
    last_tab_label: (SharedString, String),
    /// Download links by download id, minted once each.
    download_urls: std::collections::HashMap<String, Option<String>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PreviewViewEvent> for PreviewView {}

impl Focusable for PreviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PreviewView {
    /// Lists the thread's tabs and follows preview events and local servers.
    pub fn new(
        source: Rc<dyn PreviewSource>,
        thread_id: ThreadId,
        project_key: String,
        history_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search or enter URL")));
        let input_events = cx.subscribe_in(&address, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let value = this.address.read(cx).value().trim().to_owned();
                if !value.is_empty() {
                    this.navigate(value, window, cx);
                }
            }
            InputEvent::Focus => {
                this.editing = true;
                cx.notify();
            }
            InputEvent::Blur => {
                this.editing = false;
                cx.notify();
            }
            _ => {}
        });
        let players = cx.observe_global::<PreviewMiniPlayers>(|_, cx| cx.notify());
        let list = source.list(&thread_id);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = list.await {
                this.update(cx, |this, cx| {
                    this.sessions = result.sessions;
                    this.pick_active();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        let mut events = source.events();
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this.update(cx, |this, cx| this.on_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut servers = source.discovered_servers(Default::default());
        let thread = thread_id.clone();
        cx.spawn(async move |this, cx| {
            while let Some(list) = servers.next().await {
                let mine: Vec<DiscoveredLocalServer> = list
                    .servers
                    .into_iter()
                    .filter(|s| {
                        s.terminal.as_ref().is_some_and(|t| t.thread_id == thread) || s.agent.as_ref().is_some_and(|a| a.thread_id == thread)
                    })
                    .collect();
                if this
                    .update(cx, |this, cx| {
                        this.servers = mine;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let history = history_path.as_deref().map(BrowserHistory::read).unwrap_or_default();
        Self {
            source,
            thread_id,
            environment: EnvironmentId::new("fixture"),
            project_key,
            history_path,
            history,
            requested: None,
            sessions: Vec::new(),
            active: None,
            surfaces: Vec::new(),
            address,
            editing: false,
            servers: Vec::new(),
            can_operate: true,
            show_details: false,
            visible: true,
            loading_bar: LoadingBar::default(),
            pane_size: Rc::default(),
            last_tab_label: (SharedString::default(), String::new()),
            download_urls: Default::default(),
            focus: cx.focus_handle(),
            _subscriptions: vec![input_events, players],
        }
    }

    /// Shows one tab, as a right panel `preview` surface with a `resourceId` does.
    pub fn for_tab(mut self, tab: Option<String>) -> Self {
        self.active = tab.clone();
        self.requested = tab;
        self
    }

    /// Names the environment, for the mini player, and asks the server whether this
    /// session may operate previews (`useEnvironmentScope(…, AuthPreviewOperateScope)`).
    pub fn with_environment(mut self, environment: EnvironmentId, cx: &mut Context<Self>) -> Self {
        if let Some(check) = crate::components::settings::connections_settings_access::check_session(&environment, cx) {
            cx.spawn(async move |this, cx| {
                let Ok(session) = check.await else { return };
                let can = crate::components::settings::connections_settings_access::grants(&session, t3code_contracts::AuthEnvironmentScope::PreviewOperate);
                this.update(cx, |this, cx| {
                    this.can_operate = can;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        self.environment = environment;
        self
    }

    /// Whether the panel shows this tab; a hidden tab's stream disconnects (`visible`).
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            for (_, surface) in &self.surfaces {
                surface.update(cx, |surface, cx| surface.set_visible(visible, cx));
            }
        }
    }

    /// Sets the `preview:operate` grant directly, for fixtures and checks.
    pub fn set_can_operate(&mut self, can: bool, cx: &mut Context<Self>) {
        self.can_operate = can;
        cx.notify();
    }

    fn pick_active(&mut self) {
        if let Some(requested) = &self.requested {
            self.active = Some(requested.clone());
        } else if self.active.as_ref().is_none_or(|a| !self.sessions.iter().any(|s| &s.tab_id == a)) {
            self.active = self.sessions.last().map(|s| s.tab_id.clone());
        }
    }

    fn active_session(&self) -> Option<&PreviewSessionSnapshot> {
        let active = self.active.as_ref()?;
        self.sessions.iter().find(|s| &s.tab_id == active)
    }

    fn mini_key(&self) -> MiniPlayerKey {
        MiniPlayerKey { environment: self.environment.clone(), thread: self.thread_id.clone() }
    }

    fn surface(&mut self, tab: &str, cx: &mut Context<Self>) -> Entity<ServerBrowserSurface> {
        if let Some((_, surface)) = self.surfaces.iter().find(|(id, _)| id == tab) {
            return surface.clone();
        }
        let (source, thread, tab_id) = (self.source.clone(), self.thread_id.to_string(), tab.to_owned());
        let visible = self.visible;
        let surface = cx.new(|cx| {
            let mut surface = ServerBrowserSurface::new(source, thread, tab_id, cx);
            surface.set_visible(visible, cx);
            surface
        });
        cx.subscribe(&surface, |this, surface, event: &SurfaceEvent, cx| match event {
            SurfaceEvent::Popup(tab) => {
                // `showPreviewPopup(…, "panel")`: the page's new tab opens in the panel.
                this.active = Some(tab.clone());
                if this.requested.is_some() {
                    this.requested = Some(tab.clone());
                }
                this.refresh_sessions(cx);
                cx.emit(PreviewViewEvent::TabChanged);
            }
            SurfaceEvent::Gone => {
                let tab = surface.read(cx).tab_id().to_owned();
                this.remove_tab(&tab, cx);
            }
            SurfaceEvent::Control(_) | SurfaceEvent::Settled => cx.notify(),
        })
        .detach();
        self.surfaces.push((tab.to_owned(), surface.clone()));
        surface
    }

    fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        let list = self.source.list(&self.thread_id);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = list.await {
                this.update(cx, |this, cx| {
                    this.sessions = result.sessions;
                    this.pick_active();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The tab's title for the right panel's tab strip.
    pub fn tab_title(&self) -> SharedString {
        self.active_session().map(nav_title).unwrap_or_else(|| t!("Browser"))
    }

    /// The tab's address, for its favicon; `None` while idle.
    pub fn tab_url(&self) -> Option<&str> {
        self.active_session().map(nav_url).filter(|u| !u.is_empty())
    }

    /// The active tab's title as the tab strip shows it.
    pub fn active_title(&self) -> String {
        self.tab_title().to_string()
    }

    /// The tab this view shows, if it has one.
    pub fn active_tab(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The active tab's surface, if one is open.
    pub fn active_surface(&self) -> Option<Entity<ServerBrowserSurface>> {
        let active = self.active.as_ref()?;
        self.surfaces.iter().find(|(id, _)| id == active).map(|(_, s)| s.clone())
    }

    fn remove_tab(&mut self, tab: &str, cx: &mut Context<Self>) {
        self.sessions.retain(|s| s.tab_id != tab);
        self.surfaces.retain(|(id, _)| id != tab);
        if self.active.as_deref() == Some(tab) {
            self.active = if self.requested.is_some() { None } else { self.sessions.last().map(|s| s.tab_id.clone()) };
        }
        cx.emit(PreviewViewEvent::TabChanged);
        cx.notify();
    }

    fn on_event(&mut self, event: PreviewEvent, cx: &mut Context<Self>) {
        let snapshot = match event {
            PreviewEvent::Opened(e) => e.snapshot,
            PreviewEvent::Navigated(e) => e.snapshot,
            PreviewEvent::Resized(e) => e.snapshot,
            PreviewEvent::Closed(e) if e.thread_id == self.thread_id.as_str() => return self.remove_tab(&e.tab_id, cx),
            // `failed` turns the tab's own snapshot into `LoadFailed` (`previewStateStore`).
            PreviewEvent::Failed(e) if e.thread_id == self.thread_id.as_str() => {
                let Some(existing) = self.sessions.iter().find(|s| s.tab_id == e.tab_id) else { return };
                let mut snapshot = existing.clone();
                snapshot.nav_status = PreviewNavStatus::LoadFailed(t3code_contracts::PreviewNavStatusLoadFailed {
                    url: e.url,
                    title: e.title,
                    code: e.code,
                    description: e.description,
                    download: e.download.map(|d| t3code_contracts::PreviewNavStatusLoadFailedDownload { id: d.id, file_name: d.file_name }),
                });
                snapshot.updated_at = e.created_at;
                snapshot
            }
            _ => return,
        };
        self.apply_snapshot(snapshot, cx);
    }

    fn apply_snapshot(&mut self, snapshot: PreviewSessionSnapshot, cx: &mut Context<Self>) {
        if snapshot.thread_id != self.thread_id.as_str() {
            return;
        }
        let incognito = snapshot.profile_id.as_deref() == Some(INCOGNITO_PROFILE);
        if let PreviewNavStatus::Success(status) = &snapshot.nav_status
            && !incognito
        {
            self.history.record_visit(&self.project_key, &status.url, crate::timestamp_format::now_ms(), None);
            if !status.title.is_empty() {
                self.history.set_title(&self.project_key, &status.url, &status.title, None);
            }
            if let Some(path) = &self.history_path {
                self.history.save(path);
            }
        }
        match self.sessions.iter_mut().find(|s| s.tab_id == snapshot.tab_id) {
            Some(existing) => *existing = snapshot,
            None => {
                if self.active.is_none() && self.requested.is_none() {
                    self.active = Some(snapshot.tab_id.clone());
                }
                self.sessions.push(snapshot);
            }
        }
        cx.notify();
    }

    /// Whether this viewer drives the active tab's page.
    fn has_control(&self, cx: &App) -> bool {
        self.active_surface().is_some_and(|s| s.read(cx).control().is_some_and(|c| c.controller == Controller::You))
    }

    /// Navigates the active tab, or opens one when there is none.
    pub fn navigate(&mut self, raw: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = normalize_history_url(&raw) else {
            Toaster::push(ToastData::new(ToastKind::Error, t!("Could not open this address")).description(t!("Enter an http or https URL.")), cx);
            return;
        };
        window.blur(cx);
        if let Some(surface) = self.active_surface() {
            // `serverInputDisabled`: without control the address bar does nothing.
            surface.update(cx, |surface, _| surface.send(PreviewStreamInput::Navigate { url: url.clone() }));
            return;
        }
        if self.active_session().is_some() {
            return;
        }
        let settings = ClientSettingsStore::get(cx).clone();
        let open = self.source.open(PreviewOpenInput {
            thread_id: self.thread_id.clone(),
            url: Some(url),
            viewport: Some(settings.browser_default_viewport.clone()),
            profile_id: Some(default_profile_id(&settings)),
            runtime: Some(PreviewRuntime::Server),
            color_scheme: Some(settings.browser_default_appearance),
            zoom_factor: Some(settings.browser_default_zoom_factor),
            reveal: None,
        });
        cx.spawn(async move |this, cx| match open.await {
            Ok(snapshot) => {
                this.update(cx, |this, cx| {
                    this.active = Some(snapshot.tab_id.clone());
                    if this.requested.is_some() {
                        this.requested = Some(snapshot.tab_id.clone());
                    }
                    this.apply_snapshot(snapshot, cx);
                    cx.emit(PreviewViewEvent::TabChanged);
                })
                .ok();
            }
            Err(error) => {
                cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to open browser")).description(error.0), cx));
            }
        })
        .detach();
    }

    fn send(&mut self, input: PreviewStreamInput, cx: &mut Context<Self>) {
        if let Some(surface) = self.active_surface() {
            surface.update(cx, |surface, _| surface.send(input));
        }
    }

    /// `adjustServerTab`: one `preview.adjust`, its snapshot applied, a toast on failure.
    fn adjust(&mut self, change: impl FnOnce(&mut PreviewAdjustInput), cx: &mut Context<Self>) {
        let Some(session) = self.active_session() else { return };
        let mut input = PreviewAdjustInput {
            thread_id: self.thread_id.clone(),
            tab_id: session.tab_id.clone(),
            color_scheme: None,
            zoom_factor: None,
            hard_reload: None,
            clear: None,
        };
        change(&mut input);
        let adjust = self.source.adjust(input);
        cx.spawn(async move |this, cx| match adjust.await {
            Ok(snapshot) => {
                this.update(cx, |this, cx| this.apply_snapshot(snapshot, cx)).ok();
            }
            Err(error) => {
                cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to change the browser tab")).description(error.0), cx));
            }
        })
        .detach();
    }

    /// `stepServerZoom`: one step along `PREVIEW_ZOOM_LEVELS`, or back to 100%.
    fn zoom(&mut self, direction: i8, cx: &mut Context<Self>) {
        let Some(session) = self.active_session() else { return };
        let current = session.zoom_factor.unwrap_or(1.0);
        let index = ZOOM_LEVELS.iter().position(|z| (*z - current).abs() < 1e-9).map_or(7, |i| i as i32);
        let next = if direction == 0 { 1.0 } else { ZOOM_LEVELS[(index + direction as i32).clamp(0, ZOOM_LEVELS.len() as i32 - 1) as usize] };
        if (next - current).abs() > 1e-9 {
            self.adjust(|input| input.zoom_factor = Some(next), cx);
        }
    }

    /// `handleToggleDeviceToolbar`: back to fill, or the default (or a pane-filling) freeform size.
    fn toggle_device_toolbar(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.active_session() else { return };
        let viewport = match session.viewport {
            Some(PreviewViewportSetting::Fill(_)) | None => {
                let default = ClientSettingsStore::get(cx).browser_default_viewport.clone();
                if matches!(default, PreviewViewportSetting::Fill(_)) {
                    let (width, height) = self.pane_size.get();
                    responsive_viewport(width, height, session.zoom_factor.unwrap_or(1.))
                } else {
                    default
                }
            }
            _ => PreviewViewportSetting::Fill(t3code_contracts::PreviewViewportSettingFill {}),
        };
        let resize = self.source.resize(PreviewResizeInput { thread_id: self.thread_id.clone(), tab_id: session.tab_id.clone(), viewport });
        cx.spawn(async move |this, cx| match resize.await {
            Ok(snapshot) => {
                this.update(cx, |this, cx| this.apply_snapshot(snapshot, cx)).ok();
            }
            Err(error) => {
                cx.update(|cx| {
                    Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to resize browser viewport")).description(error.0), cx)
                });
            }
        })
        .detach();
    }

    /// `handleFind`: Chromium's find runs only in tabs the desktop app draws, so a streamed
    /// tab says so.
    fn find(&mut self, cx: &mut Context<Self>) {
        Toaster::push(ToastData::new(ToastKind::Info, t!("Find on page isn't available for this page")), cx);
    }

    /// `handlePictureInPicture`: floats the tab over the chat and closes the panel, or
    /// closes the floating preview that already shows it.
    fn toggle_float(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active.clone() else { return };
        let key = self.mini_key();
        if PreviewMiniPlayers::tab(&key, cx) == Some(tab.as_str()) {
            PreviewMiniPlayers::close(&key, cx);
            return;
        }
        PreviewMiniPlayers::open(key, tab, cx);
        cx.emit(PreviewViewEvent::Floated);
    }

    fn close_tab(&mut self, tab: String, cx: &mut Context<Self>) {
        let close = self.source.close(PreviewCloseInput { thread_id: self.thread_id.clone(), tab_id: Some(tab.clone()) });
        self.remove_tab(&tab, cx);
        cx.spawn(async move |_, _| {
            _ = close.await;
        })
        .detach();
    }

    /// Closes the shown tab on the server (the right panel closing its surface).
    pub fn close_active(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active.clone() {
            self.close_tab(tab, cx);
        }
    }

    fn render_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let session = self.active_session().cloned();
        let url = session.as_ref().map(|s| nav_url(s).to_owned()).unwrap_or_default();
        let surface = self.active_surface();
        let settled = surface.as_ref().is_some_and(|s| s.read(cx).settled());
        let failed = session.as_ref().is_some_and(|s| matches!(s.nav_status, PreviewNavStatus::LoadFailed(_)));
        let idle = session.as_ref().is_none_or(is_idle);
        let pending = session.is_some() && !idle && !failed && !settled;
        let loading = matches!(session.as_ref().map(|s| &s.nav_status), Some(PreviewNavStatus::Loading(_))) || pending;
        let input_disabled = session.is_some() && !self.has_control(cx);
        let can_back = session.as_ref().is_some_and(|s| s.can_go_back) && !input_disabled;
        let can_forward = session.as_ref().is_some_and(|s| s.can_go_forward) && !input_disabled;
        let refresh_disabled = idle || input_disabled;
        if !self.editing && self.address.read(cx).value() != url.as_str() {
            self.address.update(cx, |state, cx| state.set_value(url.clone(), window, cx));
        }
        self.loading_bar.follow(loading);
        let key = self.mini_key();
        let floating = self.active.as_deref().is_some_and(|tab| PreviewMiniPlayers::tab(&key, cx) == Some(tab));
        let nav_button = |id: &'static str, icon: IconName, label: SharedString, disabled: bool| {
            div().id(SharedString::from(format!("{id}-tip"))).tooltip(tooltip(label)).child(
                Button::new(id).variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(icon).disabled(disabled),
            )
        };
        let stop_label = t!("Stop", context = "stop loading the page");
        let address = div()
            .id("preview-address")
            .group("preview-address")
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h(theme.sp(7.))
            .rounded(theme.radii.control)
            .border_1()
            .border_color(if self.editing { c.ring() } else { gpui_kit::transparent_black() })
            .when(self.editing, |el| el.bg(c.background()))
            .when(!self.editing && !input_disabled, |el| el.hover(|s| s.bg(with_alpha(c.muted, 0.4))))
            .when(input_disabled, |el| el.opacity(crate::components::ui::DISABLED_OPACITY))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl(theme.sp(2.5) - px(1.))
                    .pr(if url.is_empty() || self.editing { theme.sp(2.5) - px(1.) } else { theme.sp(2.) })
                    .child(Input::new(&self.address).appearance(false).disabled(input_disabled).ui_text(&theme, TextSize::Sm, weight::NORMAL)),
            )
            .when(!url.is_empty() && !self.editing, |el| {
                let open_url = url.clone();
                el.child(
                    div()
                        .id("preview-open-external-tip")
                        .flex_none()
                        .pr(theme.sp(1.5) - px(1.))
                        .invisible()
                        .group_hover("preview-address", |s| s.visible())
                        .tooltip(tooltip(t!("Open in system browser")))
                        .child(
                            Button::new("preview-open-external")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconXs)
                                .icon(IconName::ExternalLink)
                                .on_click(move |_, _, cx| cx.open_url(&open_url)),
                        ),
                )
            });
        let weak = cx.weak_entity();
        let call = |f: fn(&mut PreviewView, &mut Context<PreviewView>)| -> Rc<dyn Fn(&mut App)> {
            let weak = weak.clone();
            Rc::new(move |cx: &mut App| {
                weak.update(cx, |this, cx| f(this, cx)).ok();
            })
        };
        let more = session.as_ref().map(|session| {
            let settings = ClientSettingsStore::get(cx).clone();
            let (zoom_weak, scheme_weak) = (weak.clone(), weak.clone());
            let actions = PreviewMoreMenuActions {
                hard_reload: call(|this, cx| this.adjust(|i| i.hard_reload = Some(true), cx)),
                set_color_scheme: Rc::new(move |scheme, cx| {
                    scheme_weak.update(cx, |this, cx| this.adjust(|i| i.color_scheme = Some(scheme), cx)).ok();
                }),
                zoom: Rc::new(move |direction, cx| {
                    zoom_weak.update(cx, |this, cx| this.zoom(direction, cx)).ok();
                }),
                clear_cookies: call(|this, cx| this.adjust(|i| i.clear = Some(PreviewAdjustInputClear::Cookies), cx)),
                clear_cache: call(|this, cx| this.adjust(|i| i.clear = Some(PreviewAdjustInputClear::Cache), cx)),
                toggle_device_toolbar: call(|this, cx| this.toggle_device_toolbar(cx)),
            };
            let state = PreviewMoreMenuState {
                enabled: true,
                zoom_factor: session.zoom_factor.unwrap_or(1.0),
                color_scheme: session.color_scheme.unwrap_or(PreviewAppearancePreference::System),
                device_toolbar_visible: !matches!(session.viewport, None | Some(PreviewViewportSetting::Fill(_))),
                profile_name: Some(profile_name(&settings, session.profile_id.as_deref().unwrap_or(DEFAULT_PROFILE))),
            };
            PreviewMoreMenu::new("preview-more-menu", state, actions)
        });
        let bar = self.render_loading_bar(&theme, cx.reduce_motion());
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.))
                    .h(theme.sp(10.))
                    .px(theme.sp(2.))
                    .border_b_1()
                    .border_color(with_alpha(c.border, 0.6))
                    .bg(c.background())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(0.5))
                            .child(nav_button("preview-back", IconName::ArrowLeft, t!("Back", context = "browser navigation"), !can_back).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if can_back {
                                        this.send(PreviewStreamInput::History { delta: -1 }, cx)
                                    }
                                }),
                            ))
                            .child(nav_button("preview-forward", IconName::ArrowRight, t!("Forward", context = "browser navigation"), !can_forward).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if can_forward {
                                        this.send(PreviewStreamInput::History { delta: 1 }, cx)
                                    }
                                }),
                            ))
                            .child(nav_button("preview-reload", IconName::RefreshCw, if loading { stop_label } else { t!("Refresh") }, refresh_disabled).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if refresh_disabled {
                                        return;
                                    }
                                    this.send(if loading { PreviewStreamInput::StopLoading } else { PreviewStreamInput::Reload { ignore_cache: None } }, cx)
                                }),
                            )),
                    )
                    .child(address)
                    .when(session.is_some(), |el| {
                        el.child(
                            div()
                                .id("preview-float-tip")
                                .tooltip(tooltip(if floating { t!("Close floating preview") } else { t!("Float preview over chat") }))
                                .child(
                                    Button::new("preview-float")
                                        .variant(if floating { ButtonVariant::Secondary } else { ButtonVariant::Ghost })
                                        .size(ButtonSize::IconXs)
                                        .icon(IconName::PictureInPicture2)
                                        .pressed(floating)
                                        .disabled(failed)
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_float(cx))),
                                ),
                        )
                    })
                    .children(more),
            )
            .child(bar)
    }

    /// The 2px progress line under the address bar, finite in both directions.
    fn render_loading_bar(&self, theme: &crate::AppTheme, reduce_motion: bool) -> impl IntoElement {
        let line = |level: f32| {
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .h(theme.sp(0.5))
                .w(relative(level))
                .rounded_r(theme.radii.full)
                .bg(theme.colors.primary())
                .shadow(vec![gpui_kit::BoxShadow {
                    color: theme.colors.ring(),
                    inset: false,
                    offset: gpui_kit::point(px(0.), px(0.)),
                    blur_radius: px(6.),
                    spread_radius: px(1.),
                }])
        };
        if reduce_motion {
            return div().children(self.loading_bar.started.map(|_| line(0.9))).into_any_element();
        }
        if self.loading_bar.started.is_some() {
            return LOADING.run(("preview-loading", self.loading_bar.generation as usize), line(0.04), |el, t| el.w(relative(0.04 + 0.86 * t)));
        }
        match self.loading_bar.ended {
            Some((generation, from)) => LOADED.run(("preview-loaded", generation as usize), line(from), move |el, t| {
                let elapsed = t * 370.;
                let width = from + (1. - from) * (elapsed / 150.).min(1.);
                let opacity = 1. - ((elapsed - 220.) / 150.).clamp(0., 1.);
                el.w(relative(width)).opacity(opacity)
            }),
            None => div().into_any_element(),
        }
    }

    fn render_empty(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let recent: Vec<_> = self.history.recent(&self.project_key, 6).to_vec();
        let row = |id: String, label: String, detail: String| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .h(theme.sp(8.))
                .px(theme.sp(2.))
                .rounded(theme.radii.md)
                .cursor_pointer()
                .hover(|s| s.bg(with_alpha(c.muted, 0.6)))
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .child(Icon::new(IconName::Globe).size(IconSize::Sm).color(c.muted_foreground))
                .child(div().flex_1().min_w_0().truncate().text_color(c.foreground()).child(label))
                .child(div().flex_none().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(detail))
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme.sp(4.))
            .p(theme.sp(6.))
            .bg(c.background())
            .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("No preview yet")))
            .when(!recent.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_w(theme.rem(24.))
                        .gap(theme.sp(1.))
                        .child(div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(c.muted_foreground).child(t!("Recently used")))
                        .children(recent.into_iter().enumerate().map(|(index, entry)| {
                            let url = entry.url.clone();
                            row(format!("recent-{index}"), entry.title.clone().unwrap_or_else(|| entry.url.clone()), String::new())
                                .on_click(cx.listener(move |this, _, window, cx| this.navigate(url.clone(), window, cx)))
                        })),
                )
            })
            .when(!self.servers.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_w(theme.rem(24.))
                        .gap(theme.sp(1.))
                        .child(div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(c.muted_foreground).child(t!("Local servers")))
                        .children(self.servers.clone().into_iter().map(|server| {
                            let url = server.url.clone();
                            row(format!("server-{}", server.port), server.url.clone(), server.process_name.clone().unwrap_or_default())
                                .on_click(cx.listener(move |this, _, window, cx| this.navigate(url.clone(), window, cx)))
                        }))
                        .child(
                            div()
                                .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(t!("Select a live local server to open it in this browser tab.")),
                        ),
                )
            })
    }

    /// The link "Download" saves from; asked of the server the first time it is shown.
    fn download_url(&mut self, tab: &str, id: &str, cx: &mut Context<Self>) -> Option<String> {
        if let Some(url) = self.download_urls.get(id) {
            return url.clone();
        }
        self.download_urls.insert(id.to_owned(), None);
        let (request, id) = (self.source.download_url(&self.thread_id, tab, id), id.to_owned());
        cx.spawn(async move |this, cx| {
            if let Ok(url) = request.await {
                this.update(cx, |this, cx| {
                    this.download_urls.insert(id, Some(url));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        None
    }

    /// `PreviewPanel` without `preview:operate`.
    fn render_no_access(&self, cx: &App) -> impl IntoElement {
        let theme = cx.app_theme();
        div().flex().flex_col().flex_1().min_h_0().items_center().justify_center().gap(theme.sp(3.)).p(theme.sp(8.)).child(
            div()
                .max_w(theme.rem(24.))
                .text_center()
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(theme.colors.muted_foreground)
                .child(t!("Pair this client again with preview access to control browser previews.")),
        )
    }
}

/// `browserDefaults.profileId`: the default profile unless it is missing or incognito.
fn default_profile_id(settings: &crate::hooks::use_settings::ClientSettings) -> String {
    let id = settings.browser_default_profile_id.as_str();
    let known = id == DEFAULT_PROFILE
        || settings.browser_profiles.iter().any(|p| p.id == id && p.kind != t3code_contracts::BrowserProfileKind::Incognito);
    if known && id != INCOGNITO_PROFILE { id.to_owned() } else { DEFAULT_PROFILE.to_owned() }
}

/// `previewProfileName`: the profile's name, "Removed profile" when it is gone.
fn profile_name(settings: &crate::hooks::use_settings::ClientSettings, id: &str) -> SharedString {
    match id {
        DEFAULT_PROFILE => "Default".into(),
        INCOGNITO_PROFILE => "Incognito".into(),
        _ => settings.browser_profiles.iter().find(|p| p.id == id).map_or_else(|| t!("Removed profile"), |p| p.name.clone().into()),
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let label = (self.tab_title(), self.tab_url().unwrap_or_default().to_owned());
        if label != self.last_tab_label {
            self.last_tab_label = label;
            cx.emit(PreviewViewEvent::TabChanged);
        }
        let root = div()
            .id("preview-view")
            .track_focus(&self.focus)
            .key_context("PreviewView")
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .bg(theme.colors.background())
            .text_color(theme.colors.foreground());
        if !self.can_operate {
            return root.child(self.render_no_access(cx)).into_any_element();
        }
        let chrome = self.render_chrome(window, cx).into_any_element();
        let session = self.active_session().cloned();
        let idle = session.as_ref().is_none_or(is_idle);
        let failed = session.as_ref().and_then(|s| match &s.nav_status {
            PreviewNavStatus::LoadFailed(f) => Some(f.clone()),
            _ => None,
        });
        let surface = session.as_ref().map(|s| {
            let surface = self.surface(&s.tab_id.clone(), cx);
            div().absolute().inset_0().when(idle || failed.is_some(), |el| el.invisible()).child(surface)
        });
        let empty = idle.then(|| self.render_empty(cx).into_any_element());
        let failed_tab = session.as_ref().map(|s| s.tab_id.clone()).unwrap_or_default();
        let failure = failed.map(|failed| {
            let content = match failed.download {
                Some(download) => {
                    let download_url = self.download_url(&failed_tab, &download.id, cx);
                    PreviewFileNotShown { url: failed.url.clone(), file_name: download.file_name, download_url }.into_any_element()
                }
                None => {
                    let weak = cx.weak_entity();
                    let reload = weak.clone();
                    PreviewUnreachable::new(failed.url.clone(), failed.code, failed.description.clone(), self.show_details)
                        .on_toggle_details(move |_, cx| {
                            weak.update(cx, |this, cx| {
                                this.show_details = !this.show_details;
                                cx.notify();
                            })
                            .ok();
                        })
                        .on_reload(move |_, cx| {
                            reload.update(cx, |this, cx| this.send(PreviewStreamInput::Reload { ignore_cache: None }, cx)).ok();
                        })
                        .into_any_element()
                }
            };
            div().absolute().inset_0().bg(theme.colors.background()).child(content)
        });
        let pane_size = self.pane_size.clone();
        root.on_action(cx.listener(|this, _: &RefreshPreview, _, cx| this.send(PreviewStreamInput::Reload { ignore_cache: None }, cx)))
            .on_action(cx.listener(|this, _: &ZoomInPreview, _, cx| this.zoom(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomOutPreview, _, cx| this.zoom(-1, cx)))
            .on_action(cx.listener(|this, _: &ResetPreviewZoom, _, cx| this.zoom(0, cx)))
            .on_action(cx.listener(|this, _: &FindInPreview, _, cx| this.find(cx)))
            .on_action(cx.listener(|this, _: &FocusPreviewUrl, window, cx| this.address.update(cx, |state, cx| state.focus(window, cx))))
            .child(chrome)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        gpui_kit::canvas(move |bounds, _, _| pane_size.set((f32::from(bounds.size.width), f32::from(bounds.size.height))), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .children(surface)
                    .children(empty)
                    .children(failure),
            )
            .into_any_element()
    }
}
