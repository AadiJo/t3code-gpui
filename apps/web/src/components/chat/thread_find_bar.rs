//! The find bar over the timeline (Mod+F). Ports `ThreadFindBar.tsx`: the
//! query field, the `n/total` counter (red when nothing matches), previous,
//! next and close. Enter goes to the next match, Shift+Enter to the
//! previous, Escape closes. The web bar has no case or whole-word options.

use gpui_kit::{
    component::input::{Input, InputEvent, InputState},
    *,
};

use super::thread_find::format_count;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::popover::glass_surface;
use crate::i18n::t;

actions!(thread_find, [CloseFind]);

/// Binds Escape inside the bar. Shift+Enter arrives as the field's own
/// `PressEnter { shift }`.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CloseFind, Some("ThreadFindBar"))]);
}

/// What the bar asks the chat view to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadFindBarEvent {
    QueryChanged(SharedString),
    /// +1 next, -1 previous.
    Step(isize),
    Close,
}

/// The find bar.
pub struct ThreadFindBar {
    input: Entity<InputState>,
    active: usize,
    total: usize,
    last_query: SharedString,
    _subscription: Subscription,
}

impl EventEmitter<ThreadFindBarEvent> for ThreadFindBar {}

impl ThreadFindBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Find in thread")));
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let query = input.read(cx).value().clone();
                if query != this.last_query {
                    this.last_query = query.clone();
                    cx.emit(ThreadFindBarEvent::QueryChanged(query));
                }
            }
            InputEvent::PressEnter { shift, .. } => cx.emit(ThreadFindBarEvent::Step(if *shift { -1 } else { 1 })),
            _ => {}
        });
        Self { input, active: 0, total: 0, last_query: SharedString::default(), _subscription: subscription }
    }

    /// Focuses the field and selects its query, as each Mod+F does.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
    }

    pub fn set_query(&self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_value(query.to_string(), window, cx));
    }

    pub fn query(&self, cx: &App) -> SharedString {
        self.input.read(cx).value().clone()
    }

    pub fn set_count(&mut self, active: usize, total: usize, cx: &mut Context<Self>) {
        if (self.active, self.total) != (active, total) {
            (self.active, self.total) = (active, total);
            cx.notify();
        }
    }

    pub fn label(&self, cx: &App) -> String {
        if self.query(cx).trim().is_empty() { String::new() } else { format_count(self.active, self.total) }
    }
}

impl Render for ThreadFindBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let has_query = !self.query(cx).trim().is_empty();
        let no_results = has_query && self.total == 0;
        let nav_disabled = self.total == 0;
        let weak = cx.weak_entity();
        let button = |id: &'static str, icon: IconName, event: ThreadFindBarEvent, disabled: bool| {
            let weak = weak.clone();
            Button::new(id)
                .icon(icon)
                .size(ButtonSize::IconXs)
                .variant(ButtonVariant::Ghost)
                .disabled(disabled)
                .on_click(move |_, _, cx| {
                    weak.update(cx, |_, cx| cx.emit(event.clone())).ok();
                })
        };
        div()
            .id("thread-find-bar")
            .key_context("ThreadFindBar")
            .on_action(cx.listener(|_, _: &CloseFind, _, cx| cx.emit(ThreadFindBarEvent::Close)))
            .occlude()
            // `top-3 right-3 w-[min(var(--thread-details-panel-width),calc(100%-1.5rem))]`
            .absolute()
            .top(px(12.))
            .left(px(12.))
            .right(px(12.))
            .flex()
            .justify_end()
            .child(
                glass_surface(theme)
                    .w(theme.spacing.thread_details_panel_width)
                    .max_w_full()
                    .h(px(36.))
                    .rounded(theme.radii.lg)
                    .pl(px(10.))
                    .pr(px(4.))
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.input).appearance(false)))
                    .child(
                        div()
                            .min_w(px(40.))
                            .text_center()
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(if no_results { c.destructive() } else { c.muted_foreground })
                            .child(self.label(cx)),
                    )
                    .child(button("find-previous", IconName::ChevronUp, ThreadFindBarEvent::Step(-1), nav_disabled))
                    .child(button("find-next", IconName::ChevronDown, ThreadFindBarEvent::Step(1), nav_disabled))
                    .child(button("find-close", IconName::X, ThreadFindBarEvent::Close, false)),
            )
    }
}
