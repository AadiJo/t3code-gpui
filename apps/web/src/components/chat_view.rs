//! The chat view: header, timeline and the composer overlay. Ports the
//! layout of `ChatView.tsx` and `ChatCanvas.tsx`. The composer overlays the
//! timeline; its measured height becomes the timeline's bottom inset, so the
//! last row always ends above it.

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    base::TextSelectionLayer, canvas, div, linear_color_stop, linear_gradient, px,
};

use super::chat::{
    chat_header::{PanelState, PanelToggle, chat_header, panel_toggles},
    chat_icons::{ChatIcon, icon},
    chat_style::{ChatColors, TEXT_SM, TEXT_XS, alpha},
    messages_timeline::{MessagesTimeline, SM_BREAKPOINT, TIMELINE_GUTTER, TIMELINE_GUTTER_NARROW, TIMELINE_TOP_INSET},
    pending_user_input_panel::pending_user_input_panel,
    timeline_entries::TimelineInput,
};

/// The composer lane is 2rem wider than the message lane (fork, `index.css:132-133`).
pub const COMPOSER_MAX_WIDTH: Pixels = px(768.);
/// Composer overlay `pt-1.5 sm:pt-2`; spacer under it `h-[1.25rem]` on desktop.
const COMPOSER_TOP_PAD: Pixels = px(8.);
const COMPOSER_BOTTOM_SPACER: Pixels = px(20.);
/// Placeholder composer: `rounded-2xl`, about the resting height of the real one.
const COMPOSER_PLACEHOLDER_HEIGHT: Pixels = px(118.);
/// Scroll-to-end button: `h-6 px-[7px] rounded-full`, 4px above the composer.
const SCROLL_BUTTON_HEIGHT: Pixels = px(24.);
const SCROLL_BUTTON_GAP: Pixels = px(4.);
/// The top fade masks the first 24px under the header, outside the 6px scrollbar lane.
const SCROLLBAR_LANE: Pixels = px(6.);

/// The chat view of one thread.
pub struct ChatView {
    timeline: Entity<MessagesTimeline>,
    project: SharedString,
    title: SharedString,
    panels: PanelState,
    /// The composer view, mounted by the composer module. A placeholder of the
    /// same shape draws until then.
    composer: Option<AnyView>,
}

impl ChatView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let timeline = cx.new(MessagesTimeline::new);
        cx.observe(&timeline, |_, _, cx| cx.notify()).detach();
        Self {
            timeline,
            project: SharedString::default(),
            title: SharedString::default(),
            panels: PanelState::default(),
            composer: None,
        }
    }

    /// Shows a thread: its breadcrumb and timeline.
    pub fn set_thread(
        &mut self,
        key: SharedString,
        project: SharedString,
        title: SharedString,
        input: TimelineInput,
        cx: &mut Context<Self>,
    ) {
        self.project = project;
        self.title = title;
        self.timeline.update(cx, |timeline, cx| timeline.set_thread(key, input, cx));
        cx.notify();
    }

    /// Mounts the composer in the overlay slot.
    pub fn set_composer(&mut self, composer: AnyView, cx: &mut Context<Self>) {
        self.composer = Some(composer);
        cx.notify();
    }

    pub fn timeline(&self) -> &Entity<MessagesTimeline> {
        &self.timeline
    }

    pub fn panels(&self) -> PanelState {
        self.panels
    }

    fn toggle_panel(&mut self, toggle: PanelToggle, cx: &mut Context<Self>) {
        match toggle {
            PanelToggle::ThreadDetails => self.panels.thread_details ^= true,
            PanelToggle::Terminal => self.panels.terminal ^= true,
            PanelToggle::RightPanel => self.panels.right_panel ^= true,
        }
        cx.notify();
    }

    /// The composer's stand-in: the glass card shape with its placeholder text.
    fn composer_placeholder(colors: &ChatColors) -> impl IntoElement {
        let outline = super::chat::timeline_row_parts::mix(colors.theme.canvas, colors.theme.input, 0.3);
        div()
            .w_full()
            .h(COMPOSER_PLACEHOLDER_HEIGHT)
            .rounded(px(16.))
            .border_1()
            .border_color(outline)
            .bg(colors.theme.surface_raised)
            .px_4()
            .pt_4()
            .text_size(TEXT_SM.size)
            .line_height(px(22.75))
            .text_color(colors.theme.placeholder)
            .child("Ask anything. @ files, $ skills, / commands")
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ChatColors::get(cx);
        let gutter = if window.viewport_size().width >= SM_BREAKPOINT { TIMELINE_GUTTER } else { TIMELINE_GUTTER_NARROW };
        let timeline = self.timeline.clone();
        let (show_scroll_button, pending_question) = {
            let timeline = self.timeline.read(cx);
            (timeline.scroll_button_visible(), timeline.input().pending_question.clone())
        };
        let measure_timeline = timeline.clone();
        let weak = cx.weak_entity();

        let composer_overlay = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .pt(COMPOSER_TOP_PAD)
            .px(gutter)
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .max_w(COMPOSER_MAX_WIDTH)
                    .flex()
                    .flex_col()
                    .children(pending_question.as_ref().map(|question| {
                        div()
                            .mx(px(16.))
                            .rounded_t(px(12.))
                            .border_1()
                            .border_b_0()
                            .border_color(alpha(colors.theme.border, 0.8))
                            .bg(colors.theme.surface_raised)
                            .pb_2()
                            .child(pending_user_input_panel(question, Some(0), &colors))
                    }))
                    .child(match &self.composer {
                        Some(composer) => composer.clone().into_any_element(),
                        None => Self::composer_placeholder(&colors).into_any_element(),
                    })
                    .child(div().h(COMPOSER_BOTTOM_SPACER)),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        measure_timeline.update(cx, |timeline, cx| timeline.set_composer_inset(bounds.size.height, cx));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        let scroll_button = show_scroll_button.then(|| {
            div().absolute().left_0().right_0().bottom_0().flex().justify_center().child(
                div()
                    .id("scroll-to-end")
                    .mb(self.timeline.read(cx).composer_bottom() + SCROLL_BUTTON_GAP)
                    .h(SCROLL_BUTTON_HEIGHT)
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .border_1()
                    .border_color(alpha(colors.theme.border, 0.6))
                    .bg(alpha(colors.theme.canvas, 0.8))
                    .shadow_sm()
                    .cursor_pointer()
                    .text_size(TEXT_XS.size)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.theme.text)
                    .child(icon(ChatIcon::ChevronDown, px(14.), colors.theme.text))
                    .child("Scroll to end")
                    .on_click(move |_, _, cx| timeline.update(cx, |timeline, cx| timeline.scroll_to_end(cx))),
            )
        });

        let top_fade = div()
            .absolute()
            .top_0()
            .left_0()
            .right(SCROLLBAR_LANE)
            .h(TIMELINE_TOP_INSET)
            .bg(linear_gradient(
                180.,
                linear_color_stop(colors.theme.canvas, 0.),
                linear_color_stop(alpha(colors.theme.canvas, 0.), 1.),
            ));

        div()
            .relative()
            .flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .bg(colors.theme.canvas)
            .text_color(colors.theme.text)
            .font_family(super::chat::chat_style::SANS_FAMILY)
            .child(TextSelectionLayer)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(chat_header(&self.project, &self.title, &colors))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .child(div().flex().flex_col().flex_1().min_h_0().child(self.timeline.clone()))
                            .child(top_fade)
                            .children(scroll_button)
                            .child(composer_overlay),
                    ),
            )
            .child(panel_toggles(self.panels, &colors, move |toggle, _, cx| {
                weak.update(cx, |this, cx| this.toggle_panel(toggle, cx)).ok();
            }))
    }
}

/// Creates a chat view entity.
pub fn chat_view(cx: &mut App) -> Entity<ChatView> {
    cx.new(ChatView::new)
}
