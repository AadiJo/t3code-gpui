//! The browser right-panel tab in streamed mode. Ports the streamed parts of
//! `PreviewView.tsx`, `PreviewChromeRow.tsx` and `PreviewEmptyState.tsx`:
//! the thread's server tabs, an address bar with back, forward, reload and
//! history, zoom, the live surface, and "Open in system browser" for what
//! streaming cannot do (webview tabs, annotations, recording, devtools).
//! Mount one per thread; the right-panel tab system owns where it sits.

use std::path::PathBuf;
use std::rc::Rc;

use futures::StreamExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
};
use t3code_client_runtime::preview::server_browser_stream::PreviewStreamInput;
use t3code_contracts::{
    DiscoveredLocalServer, PreviewAdjustInput, PreviewCloseInput, PreviewEvent, PreviewNavStatus, PreviewOpenInput, PreviewRuntime,
    PreviewSessionSnapshot, ThreadId,
};

use super::preview_source::PreviewSource;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::browser::server_browser_surface::{ServerBrowserSurface, SurfaceEvent};
use crate::browser_history_store::{BrowserHistory, normalize_history_url};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::shortcuts::commands::{FocusPreviewUrl, RefreshPreview, ResetPreviewZoom, ZoomInPreview, ZoomOutPreview};
use crate::theme_color::with_alpha;

/// Zoom steps the toolbar and shortcuts move through.
const ZOOM_STEPS: [f64; 9] = [0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0];

/// What the address bar shows for a tab.
fn nav_url(snapshot: &PreviewSessionSnapshot) -> &str {
    match &snapshot.nav_status {
        PreviewNavStatus::Loading(s) => &s.url,
        PreviewNavStatus::Success(s) => &s.url,
        PreviewNavStatus::LoadFailed(s) => &s.url,
        _ => "",
    }
}

fn nav_title(snapshot: &PreviewSessionSnapshot) -> String {
    let title = match &snapshot.nav_status {
        PreviewNavStatus::Loading(s) => s.title.as_str(),
        PreviewNavStatus::Success(s) => s.title.as_str(),
        PreviewNavStatus::LoadFailed(s) => s.title.as_str(),
        _ => "",
    };
    if !title.trim().is_empty() {
        return title.to_owned();
    }
    url::Url::parse(nav_url(snapshot)).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| t!("New tab").to_string())
}

/// The browser panel for one thread.
pub struct PreviewView {
    source: Rc<dyn PreviewSource>,
    thread_id: ThreadId,
    /// History is keyed by the thread's project.
    project_key: String,
    history_path: Option<PathBuf>,
    history: BrowserHistory,
    sessions: Vec<PreviewSessionSnapshot>,
    active: Option<String>,
    surfaces: Vec<(String, Entity<ServerBrowserSurface>)>,
    address: Entity<InputState>,
    editing: bool,
    servers: Vec<DiscoveredLocalServer>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

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
        let subscription = cx.subscribe_in(&address, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let value = this.address.read(cx).value().trim().to_owned();
                if !value.is_empty() {
                    this.navigate(value, window, cx);
                }
            }
            InputEvent::Focus => this.editing = true,
            InputEvent::Blur => {
                this.editing = false;
                cx.notify();
            }
            _ => {}
        });
        let list = source.list(&thread_id);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = list.await {
                this.update(cx, |this, cx| {
                    this.sessions = result.sessions;
                    if this.active.is_none() {
                        this.active = this.sessions.first().map(|s| s.tab_id.clone());
                    }
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
                        s.terminal.as_ref().is_some_and(|t| t.thread_id == thread)
                            || s.agent.as_ref().is_some_and(|a| a.thread_id == thread)
                    })
                    .collect();
                if this.update(cx, |this, cx| {
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
            project_key,
            history_path,
            history,
            sessions: Vec::new(),
            active: None,
            surfaces: Vec::new(),
            address,
            editing: false,
            servers: Vec::new(),
            focus: cx.focus_handle(),
            _subscriptions: vec![subscription],
        }
    }

    fn active_session(&self) -> Option<&PreviewSessionSnapshot> {
        let active = self.active.as_ref()?;
        self.sessions.iter().find(|s| &s.tab_id == active)
    }

    fn surface(&mut self, tab: &str, cx: &mut Context<Self>) -> Entity<ServerBrowserSurface> {
        if let Some((_, surface)) = self.surfaces.iter().find(|(id, _)| id == tab) {
            return surface.clone();
        }
        let (source, thread, tab_id) = (self.source.clone(), self.thread_id.to_string(), tab.to_owned());
        let surface = cx.new(|cx| ServerBrowserSurface::new(source, thread, tab_id, cx));
        cx.subscribe(&surface, |this, surface, event: &SurfaceEvent, cx| match event {
            SurfaceEvent::Popup(tab) => {
                this.active = Some(tab.clone());
                let list = this.source.list(&this.thread_id);
                cx.spawn(async move |this, cx| {
                    if let Ok(result) = list.await {
                        this.update(cx, |this, cx| {
                            this.sessions = result.sessions;
                            cx.notify();
                        })
                        .ok();
                    }
                })
                .detach();
            }
            SurfaceEvent::Gone => {
                let tab = surface.read(cx).tab_id().to_owned();
                this.remove_tab(&tab, cx);
            }
            SurfaceEvent::Control(_) => cx.notify(),
        })
        .detach();
        self.surfaces.push((tab.to_owned(), surface.clone()));
        surface
    }

    /// The active tab's title as the tab strip shows it.
    pub fn active_title(&self) -> String {
        self.active_session().map(nav_title).unwrap_or_default()
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
            self.active = self.sessions.last().map(|s| s.tab_id.clone());
        }
        cx.notify();
    }

    fn on_event(&mut self, event: PreviewEvent, cx: &mut Context<Self>) {
        let snapshot = match event {
            PreviewEvent::Opened(e) => e.snapshot,
            PreviewEvent::Navigated(e) => e.snapshot,
            PreviewEvent::Resized(e) => e.snapshot,
            PreviewEvent::Closed(e) if e.thread_id == self.thread_id.as_str() => return self.remove_tab(&e.tab_id, cx),
            _ => return,
        };
        if snapshot.thread_id != self.thread_id.as_str() {
            return;
        }
        if let PreviewNavStatus::Success(status) = &snapshot.nav_status {
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
                self.active.get_or_insert(snapshot.tab_id.clone());
                self.sessions.push(snapshot);
            }
        }
        cx.notify();
    }

    /// Navigates the active tab, or opens one when there is none.
    pub fn navigate(&mut self, raw: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = normalize_history_url(&raw) else {
            Toaster::push(
                ToastData::new(ToastKind::Error, t!("Could not open this address")).description(t!("Enter an http or https URL.")),
                cx,
            );
            return;
        };
        window.blur(cx);
        if let Some(surface) = self.active_surface()
            && surface.update(cx, |surface, _| surface.send(PreviewStreamInput::Navigate { url: url.clone() }))
        {
            return;
        }
        if self.active.is_some() {
            return;
        }
        let open = self.source.open(PreviewOpenInput {
            thread_id: self.thread_id.clone(),
            url: Some(url),
            viewport: None,
            profile_id: None,
            runtime: Some(PreviewRuntime::Server),
            color_scheme: None,
            zoom_factor: None,
            reveal: None,
        });
        cx.spawn(async move |this, cx| match open.await {
            Ok(snapshot) => {
                this.update(cx, |this, cx| {
                    this.active = Some(snapshot.tab_id.clone());
                    if !this.sessions.iter().any(|s| s.tab_id == snapshot.tab_id) {
                        this.sessions.push(snapshot);
                    }
                    cx.notify();
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

    fn zoom(&mut self, direction: i8, cx: &mut Context<Self>) {
        let Some(session) = self.active_session() else { return };
        let current = session.zoom_factor.unwrap_or(1.0);
        let next = match direction {
            0 => 1.0,
            d if d > 0 => ZOOM_STEPS.iter().copied().find(|z| *z > current + 1e-6).unwrap_or(current),
            _ => ZOOM_STEPS.iter().rev().copied().find(|z| *z < current - 1e-6).unwrap_or(current),
        };
        let input = PreviewAdjustInput {
            thread_id: self.thread_id.clone(),
            tab_id: session.tab_id.clone(),
            color_scheme: None,
            zoom_factor: Some(next),
            hard_reload: None,
            clear: None,
        };
        let adjust = self.source.adjust(input);
        cx.spawn(async move |this, cx| {
            if let Ok(snapshot) = adjust.await {
                this.update(cx, |this, cx| this.on_event(PreviewEvent::Resized(resized(snapshot)), cx)).ok();
            }
        })
        .detach();
    }

    fn close_tab(&mut self, tab: String, cx: &mut Context<Self>) {
        let close = self.source.close(PreviewCloseInput { thread_id: self.thread_id.clone(), tab_id: Some(tab.clone()) });
        self.remove_tab(&tab, cx);
        cx.spawn(async move |_, _| {
            _ = close.await;
        })
        .detach();
    }

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let tabs = self.sessions.iter().map(|session| {
            let active = self.active.as_ref() == Some(&session.tab_id);
            let (select, close) = (session.tab_id.clone(), session.tab_id.clone());
            div()
                .id(SharedString::from(format!("preview-tab-{}", session.tab_id)))
                .flex()
                .items_center()
                .gap(theme.sp(1.5))
                .h(theme.sp(7.))
                .max_w(theme.sp(48.))
                .pl(theme.sp(2.))
                .pr(theme.sp(1.))
                .rounded(theme.radii.md)
                .cursor_pointer()
                .ui_text(&theme, TextSize::Xs, weight::MEDIUM)
                .text_color(if active { c.foreground() } else { c.muted_foreground })
                .when(active, |el| el.bg(c.muted))
                .when(!active, |el| el.hover(|s| s.bg(with_alpha(c.muted, 0.6))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active = Some(select.clone());
                    cx.notify();
                }))
                // Streamed tabs carry no favicon, so every tab shows the globe the web falls back to.
                .child(Icon::new(IconName::Globe).size(IconSize::Xs).color(c.muted_foreground))
                .child(div().flex_1().min_w_0().truncate().child(nav_title(session)))
                .child(
                    Button::new(SharedString::from(format!("preview-tab-close-{}", session.tab_id)))
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconMicro)
                        .icon(IconName::X)
                        .on_click(cx.listener(move |this, _, _, cx| this.close_tab(close.clone(), cx))),
                )
        });
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.sp(1.))
            .h(theme.sp(9.))
            .px(theme.sp(2.))
            .border_b_1()
            .border_color(with_alpha(c.border, 0.6))
            .children(tabs)
            .child(Button::new("preview-new-tab").variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(IconName::Plus).on_click(cx.listener(
                |this, _, window, cx| {
                    this.active = None;
                    this.address.update(cx, |state, cx| {
                        state.set_value("", window, cx);
                        state.focus(window, cx);
                    });
                    cx.notify();
                },
            )))
    }

    fn render_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let session = self.active_session().cloned();
        let url = session.as_ref().map(|s| nav_url(s).to_owned()).unwrap_or_default();
        let loading = matches!(session.as_ref().map(|s| &s.nav_status), Some(PreviewNavStatus::Loading(_)));
        let can_operate = self.active_surface().is_some_and(|s| s.read(cx).control().is_some_and(|c| c.can_operate));
        if !self.editing && self.address.read(cx).value() != url.as_str() {
            self.address.update(cx, |state, cx| state.set_value(url.clone(), window, cx));
        }
        let open_url = url.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.sp(1.))
            .h(theme.sp(10.))
            .px(theme.sp(2.))
            .border_b_1()
            .border_color(with_alpha(c.border, 0.6))
            .bg(c.background())
            .child(
                Button::new("preview-back")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::ArrowLeft)
                    .disabled(!session.as_ref().is_some_and(|s| s.can_go_back) || !can_operate)
                    .on_click(cx.listener(|this, _, _, cx| this.send(PreviewStreamInput::History { delta: -1 }, cx))),
            )
            .child(
                Button::new("preview-forward")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::ArrowRight)
                    .disabled(!session.as_ref().is_some_and(|s| s.can_go_forward) || !can_operate)
                    .on_click(cx.listener(|this, _, _, cx| this.send(PreviewStreamInput::History { delta: 1 }, cx))),
            )
            .child(
                Button::new("preview-reload")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(if loading { IconName::X } else { IconName::RefreshCw })
                    .disabled(session.is_none() || !can_operate)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(if loading { PreviewStreamInput::StopLoading } else { PreviewStreamInput::Reload { ignore_cache: None } }, cx)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .h(theme.sp(7.))
                    .px(theme.sp(2.))
                    .rounded(theme.radii.md)
                    .hover(|s| s.bg(with_alpha(c.muted, 0.6)))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.address).appearance(false).ui_text(&theme, TextSize::Sm, weight::NORMAL)))
                    .when(!url.is_empty(), |el| {
                        el.child(
                            Button::new("preview-open-external")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconXs)
                                .icon(IconName::ExternalLink)
                                .on_click(move |_, _, cx| cx.open_url(&open_url)),
                        )
                    }),
            )
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
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(theme.sp(4.))
            .p(theme.sp(6.))
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
}

fn resized(snapshot: PreviewSessionSnapshot) -> t3code_contracts::PreviewEventResized {
    t3code_contracts::PreviewEventResized {
        thread_id: snapshot.thread_id.clone(),
        tab_id: snapshot.tab_id.clone(),
        created_at: snapshot.updated_at.clone(),
        server_epoch: String::new(),
        revision: 1,
        snapshot,
        request: None,
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.render_tabs(cx).into_any_element();
        let chrome = self.render_chrome(window, cx).into_any_element();
        let body = match self.active.clone() {
            Some(tab) => self.surface(&tab, cx).into_any_element(),
            None => self.render_empty(cx).into_any_element(),
        };
        let theme = cx.app_theme();
        div()
            .id("preview-view")
            .track_focus(&self.focus)
            .key_context("PreviewView")
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .bg(theme.colors.background())
            .text_color(theme.colors.foreground())
            .on_action(cx.listener(|this, _: &RefreshPreview, _, cx| this.send(PreviewStreamInput::Reload { ignore_cache: None }, cx)))
            .on_action(cx.listener(|this, _: &ZoomInPreview, _, cx| this.zoom(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomOutPreview, _, cx| this.zoom(-1, cx)))
            .on_action(cx.listener(|this, _: &ResetPreviewZoom, _, cx| this.zoom(0, cx)))
            .on_action(cx.listener(|this, _: &FocusPreviewUrl, window, cx| this.address.update(cx, |state, cx| state.focus(window, cx))))
            .child(tabs)
            .child(chrome)
            .child(div().flex().flex_col().flex_1().min_h_0().child(body))
    }
}

