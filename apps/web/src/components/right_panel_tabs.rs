//! The right panel: its tab strip in the titlebar row and the active
//! surface's body. Ports `RightPanelTabs.tsx`. Tabs activate on click, close
//! on the X or a middle click, reorder by drag, and have a context menu
//! (Close, Close others, Close to the right, Close all). The `+` menu and the
//! empty panel's launcher open new surfaces. Surface bodies other areas have
//! not built yet draw a labelled empty state.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::app_sidebar_layout::AppSidebarLayout;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::empty::Empty;
use crate::components::ui::menu::{ContextMenu, MenuItem, MenuPopup, dropdown_menu};
use crate::components::ui::tooltip::tooltip;
use crate::right_panel_store::{RightPanelKind, RightPanelStore, RightPanelSurface, SingletonKind};
use crate::i18n::t;

/// What the panel asks the app to do beyond editing the store.
#[derive(Clone, Debug)]
pub enum RightPanelEvent {
    /// The store changed; persist it.
    Changed,
    /// "Terminal" was picked: open the thread's terminal drawer.
    OpenTerminal,
}

/// A tab being dragged to a new position.
#[derive(Clone)]
struct DraggedTab {
    id: SharedString,
    title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        div()
            .h(px(24.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(theme.colors.accent_surface)
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(theme.colors.text)
            .child(self.title.clone())
    }
}

/// The right panel for the thread on screen.
pub struct RightPanel {
    store: Entity<RightPanelStore>,
    /// The scoped thread key whose panel shows.
    thread: Option<SharedString>,
    /// Read for the titlebar row's height and the insets around the fixed toggles.
    layout: WeakEntity<AppSidebarLayout>,
    topbar_height: Pixels,
    _observe: Subscription,
}

impl EventEmitter<RightPanelEvent> for RightPanel {}

/// The tab's label (`surfaceTitle`).
pub fn surface_title(surface: &RightPanelSurface) -> SharedString {
    match surface {
        RightPanelSurface::Diff { .. } => t!("Diff"),
        RightPanelSurface::Files { .. } => t!("Files"),
        RightPanelSurface::File { relative_path, .. } => {
            relative_path.rsplit(['/', '\\']).next().unwrap_or(relative_path).to_string().into()
        }
        RightPanelSurface::Terminal { active_terminal_id, .. } => {
            crate::components::thread_terminal_drawer::terminal_label(active_terminal_id)
        }
        RightPanelSurface::PullRequest { number, .. } => format!("#{number}").into(),
        RightPanelSurface::PullRequests { .. } => t!("Linked items"),
        RightPanelSurface::Issue { key, .. } => {
            if key.chars().all(|c| c.is_ascii_digit()) { format!("#{key}").into() } else { key.clone().into() }
        }
        RightPanelSurface::Device { title, .. } => title.clone().map(Into::into).unwrap_or_else(|| t!("Device")),
        RightPanelSurface::SideChat { .. } => t!("Side chat"),
        RightPanelSurface::Preview { .. } => t!("Browser"),
    }
}

/// The tab's 12px glyph (`SurfaceIcon`).
fn surface_icon(kind: RightPanelKind) -> IconName {
    match kind {
        RightPanelKind::Diff => IconName::FileDiff,
        RightPanelKind::Files => IconName::Files,
        RightPanelKind::File => IconName::File,
        RightPanelKind::Preview => IconName::Globe,
        RightPanelKind::Device => IconName::Smartphone,
        RightPanelKind::Terminal => IconName::SquareTerminal,
        RightPanelKind::PullRequest => IconName::GitPullRequestArrow,
        RightPanelKind::PullRequests => IconName::Link,
        RightPanelKind::Issue => IconName::CircleDot,
        RightPanelKind::SideChat => IconName::MessagesSquare,
    }
}

/// One launcher entry: a kind the `+` menu and the empty panel offer.
#[derive(Clone, Copy)]
enum Launch {
    Surface(SingletonKind),
    Terminal,
}

impl Launch {
    const ALL: [Launch; 6] = [
        Launch::Surface(SingletonKind::Preview),
        Launch::Terminal,
        Launch::Surface(SingletonKind::Files),
        Launch::Surface(SingletonKind::Diff),
        Launch::Surface(SingletonKind::PullRequests),
        Launch::Surface(SingletonKind::Device),
    ];

    fn label(self) -> SharedString {
        match self {
            Launch::Surface(SingletonKind::Preview) => t!("Browser"),
            Launch::Terminal => t!("Terminal"),
            Launch::Surface(SingletonKind::Files) => t!("Files"),
            Launch::Surface(SingletonKind::Diff) => t!("Diff"),
            Launch::Surface(SingletonKind::PullRequests) => t!("Linked items"),
            Launch::Surface(SingletonKind::Device) => t!("Device"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Launch::Surface(kind) => surface_icon(kind.kind()),
            Launch::Terminal => IconName::SquareTerminal,
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            Launch::Surface(SingletonKind::Preview) => "B",
            Launch::Terminal => "T",
            Launch::Surface(SingletonKind::Files) => "F",
            Launch::Surface(SingletonKind::Diff) => "D",
            Launch::Surface(SingletonKind::PullRequests) => "L",
            Launch::Surface(SingletonKind::Device) => "M",
        }
    }
}

impl RightPanel {
    pub fn new(store: Entity<RightPanelStore>, layout: WeakEntity<AppSidebarLayout>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self { store, thread: None, layout, topbar_height: px(40.), _observe: observe }
    }

    pub fn set_thread(&mut self, thread: Option<SharedString>, cx: &mut Context<Self>) {
        if self.thread != thread {
            self.thread = thread;
            cx.notify();
        }
    }

    fn edit(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut RightPanelStore, &str)) {
        let Some(thread) = self.thread.clone() else { return };
        self.store.update(cx, |store, cx| {
            edit(store, &thread);
            cx.notify();
        });
        cx.emit(RightPanelEvent::Changed);
    }

    fn launch(&mut self, launch: Launch, cx: &mut Context<Self>) {
        match launch {
            Launch::Surface(kind) => self.edit(cx, |store, thread| store.open(thread, kind)),
            Launch::Terminal => cx.emit(RightPanelEvent::OpenTerminal),
        }
    }

    fn launch_menu(weak: WeakEntity<Self>) -> MenuPopup {
        Launch::ALL.into_iter().fold(MenuPopup::new(), |menu, launch| {
            let weak = weak.clone();
            menu.item(MenuItem::new(launch.label()).icon(launch.icon()).on_select(move |_, cx| {
                weak.update(cx, |this, cx| this.launch(launch, cx)).ok();
            }))
        })
    }

    fn render_tab(&self, index: usize, surface: &RightPanelSurface, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let id: SharedString = surface.id().to_string().into();
        let title = surface_title(surface);
        let group: SharedString = format!("tab-{id}").into();
        let close = {
            let id = id.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                let id = id.clone();
                this.edit(cx, |store, thread| store.close_surface(thread, &id));
                cx.stop_propagation();
            })
        };
        let close_button = div()
            .id(("tab-close", index))
            .size(px(16.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .hover(|s| s.bg(c.muted))
            .child(
                div()
                    .group_hover(group.clone(), |s| s.invisible())
                    .child(Icon::new(surface_icon(surface.kind())).size(IconSize::Xs)),
            )
            .child(
                div()
                    .absolute()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .child(Icon::new(IconName::X).size(IconSize::Xs)),
            )
            .on_click(close)
            .tooltip(tooltip(t!("Close {title}", title = title.clone())));
        let (fg, bg) = if active { (c.text, Some(c.accent_surface)) } else { (c.muted_foreground, None) };
        let hover_bg = c.accent_surface.opacity(0.6);
        let hover_fg = c.text;
        let tab = div()
            .id(("tab", index))
            .group(group)
            .occlude()
            .h(px(24.))
            .max_w(px(144.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(px(6.))
            .pr(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(fg)
            .when_some(bg, |d, bg| d.bg(bg))
            .when(!active, |d| d.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
            .child(close_button)
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(title.clone()))
            .on_click({
                let id = id.clone();
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let id = id.clone();
                    this.edit(cx, |store, thread| store.activate_surface(thread, &id));
                })
            })
            .on_mouse_up(MouseButton::Middle, {
                let id = id.clone();
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    let id = id.clone();
                    this.edit(cx, |store, thread| store.close_surface(thread, &id));
                })
            })
            .on_drag(DraggedTab { id: id.clone(), title: title.clone() }, |tab, _, _, cx| cx.new(|_| tab.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hover_bg))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                let dragged = dragged.id.clone();
                this.edit(cx, |store, thread| store.move_surface(thread, &dragged, index));
            }));
        let weak = cx.weak_entity();
        ContextMenu::new(("tab-menu", index), tab, move |_, _| {
            let action = |label: SharedString, f: fn(&mut RightPanelStore, &str, &str)| {
                let (weak, id) = (weak.clone(), id.clone());
                MenuItem::new(label).on_select(move |_, cx| {
                    let id = id.clone();
                    weak.update(cx, |this, cx| this.edit(cx, |store, thread| f(store, thread, &id))).ok();
                })
            };
            MenuPopup::new()
                .item(action(t!("Close"), |s, t, id| s.close_surface(t, id)))
                .item(action(t!("Close others"), |s, t, id| s.close_other_surfaces(t, id)))
                .item(action(t!("Close to the right"), |s, t, id| s.close_surfaces_to_right(t, id)))
                .separator()
                .item(action(t!("Close all"), |s, t, _| {
                    for surface in s.state(t).surfaces {
                        s.close_surface(t, surface.id());
                    }
                }))
        })
        .into_any_element()
    }

    fn render_launcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let hover = c.accent_surface.opacity(0.6);
        let rows = Launch::ALL.into_iter().enumerate().map(|(index, launch)| {
            div()
                .id(("launch", index))
                .h(px(32.))
                .w_full()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .rounded(theme.radii.control)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.text)
                .child(Icon::new(launch.icon()).size(IconSize::Md).color(c.muted_foreground))
                .child(div().flex_1().child(launch.label()))
                .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(launch.shortcut()))
                .on_click(cx.listener(move |this, _, _, cx| this.launch(launch, cx)))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .px(px(24.))
            .pb(self.topbar_height)
            .child(
                div()
                    .w_full()
                    .max_w(px(320.))
                    .py(px(24.))
                    .child(
                        div()
                            .mb(px(12.))
                            .text_center()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(c.text)
                            .child(t!("Open a surface")),
                    )
                    .child(div().flex().flex_col().gap(px(2.)).children(rows)),
            )
    }
}

impl Render for RightPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (topbar, leading, trailing) = self
            .layout
            .read_with(cx, |layout, _| {
                (layout.platform().topbar_height(), layout.right_panel_leading_inset(), layout.right_panel_trailing_inset(window))
            })
            .unwrap_or((px(40.), px(0.), px(112.)));
        self.topbar_height = topbar;
        let state = self.thread.as_ref().map(|t| self.store.read(cx).state(t)).unwrap_or_default();
        let active_id = state.active_surface_id.clone();
        let tabs: Vec<_> = state
            .surfaces
            .iter()
            .enumerate()
            .map(|(index, surface)| self.render_tab(index, surface, active_id.as_deref() == Some(surface.id()), cx))
            .collect();
        let weak = cx.weak_entity();
        let add = dropdown_menu(
            "right-panel-add",
            Button::new("right-panel-add-trigger")
                .icon(IconName::Plus)
                .size(ButtonSize::IconXs)
                .variant(ButtonVariant::GhostMuted),
            move |_, _| Self::launch_menu(weak.clone()),
        );
        let tab_row = div()
            .id("right-panel-tabbar")
            .flex_none()
            .h(topbar)
            .flex()
            .items_center()
            .gap_1()
            .pl(px(8.) + leading)
            .pr(trailing)
            .child(
                div()
                    .id("right-panel-tab-list")
                    .min_w_0()
                    .flex_1()
                    .h_full()
                    .overflow_x_scroll()
                    .child(div().h_full().flex().items_center().gap_1().children(tabs).child(add)),
            );
        let body = match state.active() {
            Some(surface) => {
                let theme = cx.app_theme();
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        // Surface bodies are built by other areas; until then the tab names itself.
                        Empty::new(surface_title(surface)).icon(surface_icon(surface.kind())),
                    )
                    .text_color(theme.colors.text)
                    .into_any_element()
            }
            None => self.render_launcher(cx).into_any_element(),
        };
        div().size_full().flex().flex_col().child(tab_row).child(body)
    }
}
