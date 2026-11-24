//! The terminal drawer under the chat (or the terminal tab of the right panel):
//! split panes for the active group, a terminal list when there is more than
//! one, split/new/close controls, and the drag handle. Ports
//! `ThreadTerminalDrawer.tsx`. Drawer state lives in [`TerminalUiStateStore`].

use crate::components::icons::IconName;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::tooltip::tooltip;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;
use crate::keybindings::{ShortcutContext, WhenKey, shortcut_label_for_command};
use crate::shortcuts::commands::{CloseTerminal, NewTerminal, SplitTerminal, SplitTerminalVertical};
use crate::shortcuts::keymap::ShortcutState;
use crate::terminal::{TerminalBackend, TerminalSurface, TerminalSurfaceEvent};
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand};
use crate::terminal_ui_state_store::{
    DEFAULT_THREAD_TERMINAL_HEIGHT, MAX_TERMINALS_PER_GROUP, SplitDirection, TerminalUiStateStore, ThreadKey,
    ThreadTerminalGroup, ThreadTerminalUiState,
};
use crate::theme_palette::ActiveAppTheme as _;

/// Smallest drawer height in px.
pub const MIN_DRAWER_HEIGHT: f32 = 180.;
/// The drawer may take at most this share of the window height.
const MAX_DRAWER_HEIGHT_RATIO: f32 = 0.75;
const RESIZE_HANDLE_HEIGHT: Pixels = px(6.);
const SIDEBAR_WIDTH: Pixels = px(144.);

/// Where a terminal session runs, as the server's `terminal.open` input needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub thread: ThreadKey,
    pub terminal_id: SharedString,
    pub cwd: SharedString,
    pub worktree_path: Option<SharedString>,
}

/// Creates the backend for a terminal: the RPC session in the app, a recording in scenes.
pub type BackendFactory = Rc<dyn Fn(&TerminalLaunch, &mut App) -> Box<dyn TerminalBackend>>;

/// Whether the drawer sits under the chat or fills a right-panel tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawerMode {
    Drawer,
    Panel,
}

/// What the drawer tells its host.
#[derive(Clone, Debug)]
pub enum ThreadTerminalDrawerEvent {
    /// A link in one of the terminals was activated.
    LinkActivated(TerminalSurfaceEvent),
    /// "Add to chat" on a terminal selection (`TerminalContextSelection`).
    AddTerminalContext { terminal_id: SharedString, terminal_label: SharedString, text: SharedString, line_start: usize, line_end: usize },
}

impl EventEmitter<ThreadTerminalDrawerEvent> for ThreadTerminalDrawer {}

struct HeightDrag {
    start_y: Pixels,
    start_height: f32,
    height: f32,
}

/// One thread's terminals.
pub struct ThreadTerminalDrawer {
    thread: ThreadKey,
    cwd: SharedString,
    worktree_path: Option<SharedString>,
    mode: DrawerMode,
    factory: BackendFactory,
    surfaces: HashMap<String, Entity<TerminalSurface>>,
    labels: HashMap<String, SharedString>,
    drag: Option<HeightDrag>,
    visible: bool,
    can_operate: bool,
    can_add_to_chat: bool,
    /// The terminal whose close waits on the confirmation dialog.
    pending_close: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// "Terminal 3" for `term-3`, otherwise the id itself.
pub fn terminal_label(terminal_id: &str) -> SharedString {
    let number = terminal_id
        .strip_prefix("terminal-")
        .or_else(|| terminal_id.strip_prefix("term-"))
        .map(|rest| rest.split('-').next().unwrap_or(rest))
        .filter(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()));
    match number {
        Some(number) => format!("Terminal {number}").into(),
        None => terminal_id.to_string().into(),
    }
}

/// The lowest free `term-N` id.
pub fn next_terminal_id(existing: &[String]) -> String {
    let used: Vec<SharedString> = existing.iter().map(|id| terminal_label(id)).collect();
    let mut index = 1;
    while used.iter().any(|label| *label == format!("Terminal {index}")) {
        index += 1;
    }
    format!("term-{index}")
}

impl ThreadTerminalDrawer {
    pub fn new(thread: ThreadKey, cwd: SharedString, mode: DrawerMode, factory: BackendFactory, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe_global::<TerminalUiStateStore>(|_, cx| cx.notify());
        Self {
            thread,
            cwd,
            worktree_path: None,
            mode,
            factory,
            surfaces: HashMap::new(),
            labels: HashMap::new(),
            drag: None,
            visible: true,
            can_operate: true,
            can_add_to_chat: false,
            pending_close: None,
            focus: cx.focus_handle(),
            _subscriptions: vec![observe],
        }
    }

    pub fn thread(&self) -> &ThreadKey {
        &self.thread
    }

    /// Hidden drawers keep their sessions; they just do not paint.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        cx.notify();
    }

    /// Without the operate grant the controls are disabled and terminals are read-only.
    pub fn set_can_operate(&mut self, can_operate: bool, cx: &mut Context<Self>) {
        self.can_operate = can_operate;
        for surface in self.surfaces.values() {
            surface.update(cx, |surface, _| surface.set_read_only(!can_operate));
        }
        cx.notify();
    }

    /// Whether the terminals' context menus offer "Add to chat"; the host then handles `AddTerminalContext`.
    pub fn set_can_add_to_chat(&mut self, can_add: bool, cx: &mut Context<Self>) {
        self.can_add_to_chat = can_add;
        for surface in self.surfaces.values() {
            surface.update(cx, |surface, _| surface.set_can_add_to_chat(can_add));
        }
    }

    /// Labels from server metadata (running process names), keyed by terminal id.
    pub fn set_labels(&mut self, labels: HashMap<String, SharedString>, cx: &mut Context<Self>) {
        self.labels = labels;
        cx.notify();
    }

    fn state(&self, cx: &App) -> ThreadTerminalUiState {
        cx.global::<TerminalUiStateStore>().state(&self.thread)
    }

    fn update_store(&self, cx: &mut App, update: impl FnOnce(&mut TerminalUiStateStore, &ThreadKey)) {
        let thread = self.thread.clone();
        cx.update_global::<TerminalUiStateStore, _>(|store, _| update(store, &thread));
    }

    fn label(&self, terminal_id: &str) -> SharedString {
        self.labels.get(terminal_id).cloned().unwrap_or_else(|| terminal_label(terminal_id))
    }

    /// The surface for a terminal, created (and its session started) on first use.
    pub fn surface(&mut self, terminal_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalSurface> {
        if let Some(surface) = self.surfaces.get(terminal_id) {
            return surface.clone();
        }
        let launch = TerminalLaunch {
            thread: self.thread.clone(),
            terminal_id: terminal_id.to_string().into(),
            cwd: self.cwd.clone(),
            worktree_path: self.worktree_path.clone(),
        };
        let backend = (self.factory)(&launch, cx);
        let (can_operate, can_add_to_chat) = (self.can_operate, self.can_add_to_chat);
        let surface = cx.new(|cx| {
            let mut surface = TerminalSurface::new(backend, window, cx);
            surface.set_read_only(!can_operate);
            surface.set_can_add_to_chat(can_add_to_chat);
            surface
        });
        let id = terminal_id.to_string();
        cx.subscribe(&surface, move |this, _, event: &TerminalSurfaceEvent, cx| match event {
            TerminalSurfaceEvent::SessionEnded(_) => {
                if this.can_operate {
                    this.close_terminal(&id, cx);
                }
            }
            TerminalSurfaceEvent::LinkActivated { .. } => cx.emit(ThreadTerminalDrawerEvent::LinkActivated(event.clone())),
            TerminalSurfaceEvent::AddToChat { text, line_start, line_end } => cx.emit(ThreadTerminalDrawerEvent::AddTerminalContext {
                terminal_id: id.clone().into(),
                terminal_label: this.label(&id),
                text: text.clone(),
                line_start: *line_start,
                line_end: *line_end,
            }),
            _ => {}
        })
        .detach();
        self.surfaces.insert(terminal_id.to_string(), surface.clone());
        surface
    }

    fn focus_terminal(&mut self, terminal_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let surface = self.surface(terminal_id, window, cx);
        let focus = surface.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
    }

    fn all_ids(&self, cx: &App) -> Vec<String> {
        let mut ids = self.state(cx).terminal_ids;
        ids.extend(self.surfaces.keys().cloned());
        ids
    }

    /// Adds a terminal: in a new group, or split beside or below the active one.
    pub fn launch(&mut self, split: Option<SplitDirection>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_operate {
            return;
        }
        let state = self.state(cx);
        if split.is_some() && state.active_group().is_some_and(|group| group.terminal_ids.len() >= MAX_TERMINALS_PER_GROUP) {
            return;
        }
        let id = next_terminal_id(&self.all_ids(cx));
        self.update_store(cx, |store, thread| match split {
            Some(direction) => store.split_terminal(thread, &id, direction),
            None => store.new_terminal(thread, &id),
        });
        self.focus_terminal(&id, window, cx);
        cx.notify();
    }

    /// Closes a terminal's session and removes it from the drawer.
    pub fn close_terminal(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        if let Some(surface) = self.surfaces.remove(terminal_id) {
            surface.update(cx, |surface, _| surface.close_session());
        }
        self.update_store(cx, |store, thread| store.close_terminal(thread, terminal_id));
        cx.notify();
    }

    /// Closes a terminal the user asked to close, behind the `confirmTerminalClose` dialog when that setting is on.
    pub fn request_close(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        if !self.can_operate {
            return;
        }
        if ClientSettingsStore::get(cx).confirm_terminal_close {
            self.pending_close = Some(terminal_id.to_string());
            cx.notify();
        } else {
            self.close_terminal(terminal_id, cx);
        }
    }

    fn resolve_close(&mut self, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.pending_close.take() else { return };
        if confirmed && self.can_operate {
            self.close_terminal(&id, cx);
        }
        let active = self.state(cx).active_terminal_id;
        if self.visible && !active.is_empty() {
            self.focus_terminal(&active, window, cx);
        }
        cx.notify();
    }

    /// `Close terminal "{label}"?` with Cancel and a destructive Confirm (`ConfirmDialogHost`).
    fn render_close_confirm(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let id = self.pending_close.as_ref()?;
        let label = self.label(id);
        let cancel = cx.listener(|this, _: &ClickEvent, window, cx| this.resolve_close(false, window, cx));
        let confirm = cx.listener(|this, _: &ClickEvent, window, cx| this.resolve_close(true, window, cx));
        let this = cx.weak_entity();
        let dialog = DialogSurface::new(t!("Close terminal \"{label}\"?", label = label))
            .description(t!("This stops the running process and clears its history."))
            .footer(Button::new("terminal-close-cancel").variant(ButtonVariant::Outline).size(ButtonSize::Default).label(t!("Cancel")).on_click(cancel))
            .footer(Button::new("terminal-close-confirm").variant(ButtonVariant::Destructive).size(ButtonSize::Default).label(t!("Confirm")).on_click(confirm));
        Some(Modal::dialog("terminal-close-confirm-dialog", dialog, move |window, cx| {
            this.update(cx, |this, cx| this.resolve_close(false, window, cx)).ok();
        }))
    }

    /// A terminal command's shortcut label as the drawer shows it (terminal focused and open).
    fn shortcut_label(command: StaticCommand, cx: &App) -> Option<SharedString> {
        let state = cx.try_global::<ShortcutState>()?;
        let context = ShortcutContext::default().with(WhenKey::TerminalFocus, true).with(WhenKey::TerminalOpen, true);
        shortcut_label_for_command(&state.config, &KeybindingCommand::Static(command), context, state.platform)
    }

    fn activate(&mut self, terminal_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.update_store(cx, |store, thread| store.set_active_terminal(thread, terminal_id));
        if self.visible {
            self.focus_terminal(terminal_id, window, cx);
        }
    }

    fn max_height(&self, window: &Window) -> f32 {
        (f32::from(window.viewport_size().height) * MAX_DRAWER_HEIGHT_RATIO).floor().max(MIN_DRAWER_HEIGHT)
    }

    fn clamp_height(height: f32, max: f32) -> f32 {
        let height = if height.is_finite() { height } else { DEFAULT_THREAD_TERMINAL_HEIGHT };
        height.round().clamp(MIN_DRAWER_HEIGHT, max)
    }

    // Rendering.

    fn action_button(&self, id: &'static str, icon: &'static str, label: SharedString, enabled: bool, compact: bool, cx: &mut Context<Self>, on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> Stateful<Div> {
        let colors = &cx.app_theme().colors;
        let mut button = div()
            .id(id)
            .tooltip(tooltip(label))
            .flex()
            .items_center()
            .justify_center()
            .text_color(colors.text.opacity(0.9))
            .child(svg().path(icon).size(px(13.)).text_color(colors.text.opacity(0.9)));
        button = if compact { button.h_full().px(px(4.)) } else { button.p(px(4.)) };
        if enabled {
            let hover = if compact { colors.accent_surface.opacity(0.7) } else { colors.accent_surface };
            button
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        } else {
            button.opacity(0.64)
        }
    }

    /// The new-terminal label: "New Terminal (⌘N)" or "New Terminal".
    fn new_terminal_label(cx: &App) -> SharedString {
        match Self::shortcut_label(StaticCommand::TerminalNew, cx) {
            Some(shortcut) => t!("New Terminal ({shortcut})", shortcut = shortcut),
            None => t!("New Terminal"),
        }
    }

    fn action_buttons(&self, split_enabled: bool, compact: bool, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let operate = self.can_operate;
        let max = MAX_TERMINALS_PER_GROUP;
        let split_label = match (split_enabled, Self::shortcut_label(StaticCommand::TerminalSplit, cx)) {
            (false, _) => t!("Split Terminal Horizontally (max {max} per group)", max = max),
            (true, Some(shortcut)) => t!("Split Terminal Horizontally ({shortcut})", shortcut = shortcut),
            (true, None) => t!("Split Terminal Horizontally"),
        };
        let split_vertical_label = match (split_enabled, Self::shortcut_label(StaticCommand::TerminalSplitVertical, cx)) {
            (false, _) => t!("Split Terminal Vertically (max {max} per group)", max = max),
            (true, Some(shortcut)) => t!("Split Terminal Vertically ({shortcut})", shortcut = shortcut),
            (true, None) => t!("Split Terminal Vertically"),
        };
        let close_label = match Self::shortcut_label(StaticCommand::TerminalClose, cx) {
            Some(shortcut) => t!("Close Terminal ({shortcut})", shortcut = shortcut),
            None => t!("Close Terminal"),
        };
        vec![
            self.action_button("terminal-split", IconName::SquareSplitHorizontal.path(), split_label, operate && split_enabled, compact, cx, |this, window, cx| {
                this.launch(Some(SplitDirection::Horizontal), window, cx)
            }),
            self.action_button("terminal-split-vertical", IconName::SquareSplitVertical.path(), split_vertical_label, operate && split_enabled, compact, cx, |this, window, cx| {
                this.launch(Some(SplitDirection::Vertical), window, cx)
            }),
            self.action_button("terminal-new", IconName::Plus.path(), Self::new_terminal_label(cx), operate, compact, cx, |this, window, cx| this.launch(None, window, cx)),
            self.action_button("terminal-close", IconName::Trash2.path(), close_label, operate, compact, cx, |this, _, cx| {
                let active = this.state(cx).active_terminal_id;
                this.request_close(&active, cx);
            }),
        ]
    }

    fn render_panes(&mut self, group: &ThreadTerminalGroup, active: &str, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let vertical = group.split_direction == SplitDirection::Vertical;
        let mut panes = div().flex().size_full().min_w_0().overflow_hidden();
        panes = if vertical { panes.flex_col() } else { panes.flex_row() };
        for (index, terminal_id) in group.terminal_ids.iter().enumerate() {
            let surface = self.surface(terminal_id, window, cx);
            let border = if terminal_id == active { colors.border } else { colors.border.opacity(0.7) };
            let id = terminal_id.clone();
            let mut pane = div()
                .id(SharedString::from(format!("terminal-pane-{terminal_id}")))
                .flex_1()
                .min_w_0()
                .min_h_0()
                .border_color(border)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    if this.state(cx).active_terminal_id != id {
                        this.update_store(cx, |store, thread| store.set_active_terminal(thread, &id));
                    }
                }))
                // Cached: a chatty terminal repaints itself without re-rendering the drawer.
                .child(AnyView::from(surface).cached(StyleRefinement::default().size_full()));
            if index > 0 {
                pane = if vertical { pane.border_t_1() } else { pane.border_l_1() };
            }
            panes = panes.child(pane);
        }
        panes
    }

    fn render_sidebar(&self, state: &ThreadTerminalUiState, split_enabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let show_headers = state.terminal_groups.len() > 1 || state.terminal_groups.iter().any(|group| group.terminal_ids.len() > 1);
        let mut header = div().flex().h_full().items_stretch();
        for (index, button) in self.action_buttons(split_enabled, true, cx).into_iter().enumerate() {
            header = header.child(if index > 0 { button.border_l_1().border_color(colors.border.opacity(0.7)) } else { button });
        }
        let mut list = div().id("terminal-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().p(px(4.));
        for group in &state.terminal_groups {
            list = list.child(self.render_group(group, &state.active_terminal_id, show_headers, cx));
        }
        div()
            .flex()
            .flex_col()
            .w(SIDEBAR_WIDTH)
            .min_w(SIDEBAR_WIDTH)
            .border_1()
            .border_color(colors.border.opacity(0.7))
            .bg(colors.muted.opacity(0.1))
            .child(
                div()
                    .flex()
                    .h(px(22.))
                    .justify_end()
                    .border_b_1()
                    .border_color(colors.border.opacity(0.7))
                    .child(header),
            )
            .child(list)
    }

    fn render_group(&self, group: &ThreadTerminalGroup, active: &str, show_header: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let group_active = group.terminal_ids.iter().any(|id| id == active);
        let count = group.terminal_ids.len();
        let (label, icon) = match (count > 1, group.split_direction) {
            (false, _) => (t!("Single", context = "terminal group with one terminal"), IconName::Square.path()),
            (true, SplitDirection::Vertical) => (t!("Stacked", context = "terminals split vertically"), IconName::SquareSplitVertical.path()),
            (true, SplitDirection::Horizontal) => (t!("Side by side", context = "terminals split horizontally"), IconName::SquareSplitHorizontal.path()),
        };
        let mut column = div().pb(px(2.)).flex().flex_col();
        if show_header {
            let target = if group_active { active.to_string() } else { group.terminal_ids[0].clone() };
            let text = if group_active { colors.text } else { colors.muted_foreground };
            column = column.child(
                div()
                    .id(SharedString::from(format!("terminal-group-{}", group.id)))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(22.))
                    .w_full()
                    .px(px(6.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(text)
                    .cursor_pointer()
                    .when(group_active, |row| row.bg(colors.accent_surface.opacity(0.5)))
                    .when(!group_active, |row| row.hover(|style| style.bg(colors.accent_surface.opacity(0.4)).text_color(colors.text)))
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(&target, window, cx)))
                    .child(svg().path(icon).size(px(12.)).flex_none().text_color(text))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().text_size(px(10.)).text_color(colors.muted_foreground.opacity(0.7)).child(count.to_string())),
            );
        }
        let mut rows = div().flex().flex_col().gap(px(2.));
        let close_shortcut = Self::shortcut_label(StaticCommand::TerminalClose, cx);
        for terminal_id in &group.terminal_ids {
            let is_active = terminal_id == active;
            let terminal = self.label(terminal_id);
            let close_label = match (&close_shortcut, is_active) {
                (Some(shortcut), true) => t!("Close {terminal} ({shortcut})", terminal = terminal, shortcut = shortcut),
                _ => t!("Close {terminal}", terminal = terminal),
            };
            let text = if is_active { colors.text } else { colors.muted_foreground };
            let row_id = SharedString::from(format!("terminal-row-{terminal_id}"));
            let close_id = terminal_id.clone();
            let activate_id = terminal_id.clone();
            let icon = div()
                .id(SharedString::from(format!("terminal-row-close-{terminal_id}")))
                .size(px(12.))
                .flex_none()
                .relative()
                .child(svg().path(IconName::SquareTerminal.path()).size(px(12.)).text_color(text).group_hover(row_id.clone(), |style| style.invisible()))
                .when(self.can_operate, |icon| {
                    icon.child(
                        svg()
                            .path(IconName::X.path())
                            .absolute()
                            .top_0()
                            .left_0()
                            .size(px(12.))
                            .text_color(text)
                            .invisible()
                            .group_hover(row_id.clone(), |style| style.visible()),
                    )
                    .cursor_pointer()
                    .tooltip(tooltip(close_label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.request_close(&close_id, cx);
                    }))
                });
            rows = rows.child(
                div()
                    .id(row_id.clone())
                    .group(row_id)
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .h(px(24.))
                    .w_full()
                    .pl(px(6.))
                    .pr(px(8.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(text)
                    .cursor_pointer()
                    .when(is_active, |row| row.bg(colors.accent_surface))
                    .when(!is_active, |row| row.hover(|style| style.bg(colors.accent_surface.opacity(0.6)).text_color(colors.text)))
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(&activate_id, window, cx)))
                    .child(icon)
                    .child(div().flex_1().min_w_0().truncate().child(self.label(terminal_id))),
            );
        }
        column.child(rows)
    }

    fn render_resize_handle(&self, height: f32, max: f32, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal-drawer-resize")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(RESIZE_HANDLE_HEIGHT)
            .cursor_row_resize()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.drag = Some(HeightDrag { start_y: event.position.y, start_height: height, height });
            }))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let Some(drag) = this.drag.as_mut() else { return };
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let next = Self::clamp_height(drag.start_height + f32::from(drag.start_y - event.position.y), max);
                if next != drag.height {
                    drag.height = next;
                    cx.notify();
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::end_drag))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_drag))
    }

    /// The store hears the height once, when the drag ends.
    fn end_drag(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if drag.height != drag.start_height {
            self.update_store(cx, |store, thread| store.set_terminal_height(thread, drag.height));
        }
        for surface in self.surfaces.values() {
            surface.update(cx, |surface, _| surface.flush_resize());
        }
        cx.notify();
    }
}

impl Focusable for ThreadTerminalDrawer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ThreadTerminalDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let state = self.state(cx);
        let max = self.max_height(window);
        let height = self.drag.as_ref().map_or_else(|| Self::clamp_height(state.terminal_height, max), |drag| drag.height);
        let is_drawer = self.mode == DrawerMode::Drawer;

        let mut root = div()
            .id("thread-terminal-drawer")
            .key_context("ThreadTerminalDrawer")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .overflow_hidden()
            .bg(colors.canvas)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| this.launch(None, window, cx)))
            .on_action(cx.listener(|this, _: &SplitTerminal, window, cx| this.launch(Some(SplitDirection::Horizontal), window, cx)))
            .on_action(cx.listener(|this, _: &SplitTerminalVertical, window, cx| this.launch(Some(SplitDirection::Vertical), window, cx)))
            .on_action(cx.listener(|this, _: &CloseTerminal, _, cx| {
                let active = this.state(cx).active_terminal_id;
                this.request_close(&active, cx);
            }))
            .children(self.render_close_confirm(cx));
        root = if is_drawer {
            root.flex_none().h(px(height)).border_t_1().border_color(colors.border.opacity(0.8))
        } else {
            root.size_full()
        };
        if !self.visible {
            return root.h(px(0.)).border_0();
        }
        if is_drawer {
            root = root.child(self.render_resize_handle(height, max, cx));
        }

        let Some(group) = state.active_group().cloned() else {
            let operate = self.can_operate;
            return root.child(
                div()
                    .flex()
                    .flex_1()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(24.))
                    .text_size(px(14.))
                    .text_color(colors.muted_foreground)
                    .child(t!("No terminal sessions for this thread yet."))
                    .child(
                        div()
                            .id("terminal-empty-new")
                            .h(px(24.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(colors.input)
                            .text_size(px(12.))
                            .text_color(colors.text)
                            .when(operate, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(colors.accent_surface))
                                    .on_click(cx.listener(|this, _, window, cx| this.launch(None, window, cx)))
                            })
                            .when(!operate, |button| button.opacity(0.64))
                            .child(Self::new_terminal_label(cx)),
                    ),
            );
        };

        let split_enabled = group.terminal_ids.len() < MAX_TERMINALS_PER_GROUP;
        let has_sidebar = state.terminal_ids.len() > 1;
        // Focus given to the drawer (opening it, workspace focus) belongs to its active terminal.
        if self.focus.is_focused(window) && self.pending_close.is_none() {
            self.focus_terminal(&state.active_terminal_id, window, cx);
        }
        let panes = self.render_panes(&group, &state.active_terminal_id, window, cx);
        let mut content = div().flex().size_full().min_h_0().bg(colors.terminal_background);
        if has_sidebar {
            content = content.gap(px(6.));
        }
        content = content.child(div().flex_1().min_w_0().child(panes));
        if has_sidebar {
            content = content.child(self.render_sidebar(&state, split_enabled, cx));
        }
        root = root.child(div().flex_1().min_h_0().w_full().child(content));

        if !has_sidebar {
            let mut group_buttons = div()
                .flex()
                .items_center()
                .overflow_hidden()
                .rounded(px(6.))
                .border_1()
                .border_color(colors.border.opacity(0.8))
                .bg(colors.canvas)
                .shadow_xs();
            for (index, button) in self.action_buttons(split_enabled, false, cx).into_iter().enumerate() {
                if index > 0 {
                    group_buttons = group_buttons.child(div().h(px(16.)).w(px(1.)).bg(colors.border.opacity(0.8)));
                }
                group_buttons = group_buttons.child(button);
            }
            root = root.child(div().absolute().top(px(8.)).right(px(8.)).child(group_buttons));
        }
        root
    }
}
