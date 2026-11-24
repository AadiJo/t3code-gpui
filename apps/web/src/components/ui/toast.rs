//! Toasts. Ports `toast.tsx`: glass cards in the top-right corner, stacked
//! with a 12px peek and each further card 10% smaller, spreading out with a
//! 12px gap while hovered. A corner orb dismisses; non-loading toasts expire
//! after five seconds.

use crate::components::ui::GlassExt as _;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, Hsla, IntoElement, Render, RenderOnce, SharedString, Window, anchored,
    deferred, div, point, prelude::*, px, relative,
};

use super::TextStyleExt as _;
use super::motion::Transition;
use super::button::{Button, ButtonSize, ButtonVariant};
use super::spinner::{Spinner, SpinnerSize};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

/// The toast's type, which picks its leading icon and colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastKind {
    #[default]
    Plain,
    Error,
    Info,
    Success,
    Warning,
    Loading,
}

impl ToastKind {
    pub const ALL: [ToastKind; 6] = [Self::Plain, Self::Error, Self::Info, Self::Success, Self::Warning, Self::Loading];
}

type Action = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// One notification.
#[derive(Clone)]
pub struct ToastData {
    pub kind: ToastKind,
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub action: Option<(SharedString, Action)>,
    /// A leading icon in place of the kind's (`data.leadingIcon`).
    pub icon: Option<(IconName, Hsla)>,
    /// How long it stays; `None` keeps it until dismissed (`timeout: 0`).
    pub timeout: Option<Duration>,
}

impl ToastData {
    pub fn new(kind: ToastKind, title: impl Into<SharedString>) -> Self {
        let timeout = (kind != ToastKind::Loading).then_some(TIMEOUT);
        Self { kind, title: title.into(), description: None, action: None, icon: None, timeout }
    }

    /// Keeps the toast until it is dismissed (`timeout: 0`).
    pub fn persistent(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// Dismisses the toast after `timeout` instead of the default five seconds.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Replaces the kind's leading icon.
    pub fn icon(mut self, icon: IconName, color: Hsla) -> Self {
        self.icon = Some((icon, color));
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A trailing `xs` button.
    pub fn action(mut self, label: impl Into<SharedString>, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Some((label.into(), Rc::new(action)));
        self
    }
}

fn kind_icon(theme: &AppTheme, kind: ToastKind) -> Option<(IconName, Hsla)> {
    let c = &theme.colors;
    let f = &theme.fixed;
    match kind {
        ToastKind::Error => Some((IconName::CircleAlert, c.destructive())),
        ToastKind::Info => Some((IconName::Info, f.info)),
        ToastKind::Success => Some((IconName::CircleCheck, f.success)),
        ToastKind::Warning => Some((IconName::TriangleAlert, c.warning)),
        ToastKind::Plain | ToastKind::Loading => None,
    }
}

/// The toast card, also drawn inline by the gallery.
#[derive(IntoElement)]
pub struct ToastSurface {
    data: ToastData,
    on_close: Option<Action>,
}

impl ToastSurface {
    pub fn new(data: ToastData) -> Self {
        Self { data, on_close: None }
    }

    pub fn on_close(mut self, close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(close));
        self
    }
}

/// `dropdown-glass` with `shadow-xl shadow-black/25`.
fn toast_card(theme: &AppTheme) -> gpui_kit::Div {
    div()
        .rounded(theme.radii.lg)
        .border_1()
        .border_color(theme.derived.dropdown_glass_border)
        .glass(theme.derived.glass_dropdown)
        .shadow(theme.shadows.xl25.clone())
}

impl RenderOnce for ToastSurface {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let line = px(TextSize::Sm.line_height(theme.fonts.interface));
        let icon = match self.data.kind {
            ToastKind::Loading => Some(Spinner::new(SpinnerSize::Md).color(with_alpha(c.foreground(), 0.8)).into_any_element()),
            kind => self.data.icon.or_else(|| kind_icon(theme, kind)).map(|(name, color)| Icon::new(name).size(IconSize::Md).color(color).into_any_element()),
        };
        let has_action = self.data.action.is_some();
        let close = self.on_close.clone();
        toast_card(theme)
            .relative()
            .w_full()
            .text_color(theme.foregrounds.popover)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(1.5))
                    .pl(sp(3.5))
                    .py(sp(3.))
                    .pr(if has_action { sp(6.) } else { sp(3.5) })
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(sp(0.5))
                            .child(
                                div()
                                    .flex()
                                    .gap(sp(2.))
                                    .min_w_0()
                                    .when(!has_action, |r| r.pr(sp(6.)))
                                    .children(icon.map(|icon| div().flex().items_center().justify_center().w(px(16.)).h(line).flex_none().child(icon)))
                                    .child(div().min_w_0().font_weight(weight::MEDIUM).child(self.data.title)),
                            )
                            .children(self.data.description.map(|d| div().min_w_0().text_color(c.muted_foreground).child(d))),
                    )
                    .children(self.data.action.map(|(label, action)| {
                        div().flex().items_center().gap(sp(1.5)).flex_none().child(
                            Button::new("toast-action")
                                .label(label)
                                .size(ButtonSize::Xs)
                                .variant(ButtonVariant::Default)
                                .on_click(move |_, window, cx| action(window, cx)),
                        )
                    })),
            )
            .child(
                div()
                    .id("toast-close")
                    .absolute()
                    .top(px(-6.))
                    .right(px(-6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(sp(6.))
                    .rounded(theme.radii.full)
                    .border_1()
                    .border_color(with_alpha(c.border, 0.6))
                    .bg(with_alpha(c.popover(), 0.92))
                    .shadow(theme.shadows.sm5.clone())
                    .text_color(c.muted_foreground)
                    .child(Icon::new(IconName::X).size(IconSize::Xs).stroke(2.25))
                    .when_some(close, |orb, close| {
                        let hover_fg = c.foreground();
                        let hover_bg = c.popover();
                        orb.cursor_pointer()
                            .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                            .on_click(move |_, window, cx| close(window, cx))
                    }),
            )
    }
}

/// How many toasts show at once (Base UI's default limit).
const VISIBLE_LIMIT: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(5);

/// The app's toast stack. Install with [`Toaster::init`], push with
/// [`Toaster::push`], and render [`Toaster::view`] once at the window root.
pub struct Toaster {
    toasts: Vec<(u64, ToastData)>,
    /// Toasts playing their exit transition, removed when it ends.
    closing: HashSet<u64>,
    next_id: u64,
    expanded: bool,
}

struct GlobalToaster(Entity<Toaster>);

impl Global for GlobalToaster {}

impl Toaster {
    /// Creates the app-wide toaster.
    pub fn init(cx: &mut App) {
        let toaster = cx.new(|_| Toaster { toasts: Vec::new(), closing: HashSet::new(), next_id: 0, expanded: false });
        cx.set_global(GlobalToaster(toaster));
    }

    /// The toaster view, to render as the last child of a window's root.
    pub fn view(cx: &App) -> Entity<Toaster> {
        cx.global::<GlobalToaster>().0.clone()
    }

    /// Shows a toast. It expires after its timeout (five seconds unless set;
    /// never for a loading or persistent toast).
    pub fn push(data: ToastData, cx: &mut App) -> u64 {
        let toaster = Self::view(cx);
        toaster.update(cx, |toaster, cx| {
            let id = toaster.next_id;
            toaster.next_id += 1;
            let timeout = data.timeout;
            toaster.toasts.insert(0, (id, data));
            if let Some(timeout) = timeout {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(timeout).await;
                    this.update(cx, |toaster, cx| toaster.remove(id, cx)).ok();
                })
                .detach();
            }
            cx.notify();
            id
        })
    }

    /// Removes a toast by id.
    /// The titles of the toasts on screen, newest first.
    pub fn titles(cx: &App) -> Vec<SharedString> {
        Self::view(cx).read(cx).toasts.iter().map(|(_, toast)| toast.title.clone()).collect()
    }

    pub fn dismiss(id: u64, cx: &mut App) {
        Self::view(cx).update(cx, |toaster, cx| toaster.remove(id, cx));
    }

    /// Plays the exit transition, then drops the toast.
    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if cx.reduce_motion() {
            self.toasts.retain(|(toast, _)| *toast != id);
        } else if self.toasts.iter().any(|(toast, _)| *toast == id) && self.closing.insert(id) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Transition::TOAST.duration).await;
                this.update(cx, |toaster, cx| {
                    toaster.closing.remove(&id);
                    toaster.toasts.retain(|(toast, _)| *toast != id);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }
}

impl Render for Toaster {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let inset = theme.sp(8.);
        let viewport = window.viewport_size();
        let width = (viewport.width - inset * 2.).min(px(360.));
        let top = inset + theme.spacing.workspace_topbar_height;
        let expanded = self.expanded;
        let peek = theme.sp(3.);
        let visible = self.toasts.iter().take(VISIBLE_LIMIT).enumerate();
        let cards = visible.map(|(index, (id, data))| {
            let id = *id;
            let card = ToastSurface::new(data.clone()).on_close(move |_, cx| Toaster::dismiss(id, cx));
            if index == 0 || expanded {
                let card = div().relative().w_full().child(card);
                return if self.closing.contains(&id) {
                    // `data-ending-style`: fades while sliding out past the right inset.
                    let travel = width + inset;
                    Transition::TOAST.run(("toast-exit", id), card, move |c, t| c.opacity(1. - t).left(travel * t))
                } else {
                    // `data-starting-style`: from one toast height (plus the inset) above, transparent.
                    let card = div().relative().child(card);
                    Transition::TOAST.run(("toast-enter", id), card, move |c, t| {
                        c.opacity(t).top(relative(t - 1.)).mt(-inset * (1. - t))
                    })
                };
            }
            // A card further back: the frontmost's size, scaled from the top and peeking below it.
            let shrink = 0.1 * index as f32;
            div()
                .absolute()
                .top(peek * index as f32)
                .bottom(-(peek * index as f32))
                .left_0()
                .right_0()
                .child(
                    toast_card(theme)
                        .absolute()
                        .top(relative(shrink))
                        .bottom_0()
                        .left(relative(shrink / 2.))
                        .right(relative(shrink / 2.)),
                )
                .into_any_element()
        });
        let mut cards: Vec<_> = cards.collect();
        if !expanded {
            // Paint the cards further back first.
            cards.reverse();
        }
        let stack = div()
            .id("toaster")
            .absolute()
            .top(top)
            .right(inset)
            .w(width)
            .flex()
            .flex_col()
            .when(expanded, |s| s.gap(peek))
            .on_hover(cx.listener(|toaster, hovered: &bool, _, cx| {
                toaster.expanded = *hovered;
                cx.notify();
            }))
            .children(cards);
        // Toasts (z-100) sit above dialogs and sheets (z-50).
        deferred(
            anchored()
                .position(point(px(0.), px(0.)))
                .child(div().relative().w(viewport.width).h(viewport.height).child(stack)),
        )
        .with_priority(4)
    }
}
