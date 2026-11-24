//! The chat view: header, timeline and the composer overlay. Ports the
//! layout of `ChatView.tsx` and `ChatCanvas.tsx`. The composer overlays the
//! timeline; its measured height becomes the timeline's bottom inset, so the
//! last row always ends above it. One chat view stays mounted while the user
//! moves between threads; [`ChatView::set_thread`] swaps what it shows.
//! A new thread with no messages shows the draft hero: the composer centred
//! under "What should we work on?".

use crate::components::ui::GlassExt as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{rems, 
    AnyView, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, canvas, div, linear_color_stop, linear_gradient,
    prelude::FluentBuilder as _, px,
};

use crate::components::command_palette_drag::PaletteThreadDrag;
use crate::components::files::file_tree::FileReferenceDrag;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::composer_editor_mentions::format_context_reference;
use crate::hooks::use_settings::ActiveClientSettings as _;
use crate::i18n::t;
use crate::components::sidebar::context_menu::{self, ContextMenu, MenuCommand, MenuTarget};
use crate::components::sidebar::model::{SettledOverride, SidebarThread, ThreadKey, TimestampMs};
use crate::components::sidebar::style::ShellColors;
use crate::ActiveAppTheme as _;
use crate::shortcuts::commands::FindInThread;
use crate::thread_find_store::ThreadFindStore;

use super::chat::thread_details_panel::ThreadDetailsMode;
use super::chat::{
    chat_header::{ThreadActionsTrigger, chat_header},
    chat_style::{ChatColors, TEXT_SM, TEXT_XS, alpha},
    messages_timeline::{FindUpdated, MessagesTimeline, SM_BREAKPOINT, TIMELINE_GUTTER, TIMELINE_GUTTER_NARROW, TIMELINE_TOP_INSET, TimelineAction},
    pending_user_input_panel::pending_user_input_panel,
    thread_find_bar::{ThreadFindBar, ThreadFindBarEvent},
    timeline_entries::{PendingQuestion, TimelineInput},
};

/// The composer lane is 2rem wider than the message lane (fork, `index.css:132-133`).
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

/// Draft hero headline: `text-3xl` (30/36px) on desktop, 16px above the composer.
const HERO_HEADLINE_SIZE: Pixels = px(30.);
const HERO_HEADLINE_LINE: Pixels = px(36.);
/// Below this height the hero composer stays pinned to the bottom (`34rem`).
const HERO_MIN_HEIGHT: Pixels = px(544.);
/// A picked option moves to the next question after this (`ComposerPendingUserInputPanel`).
const QUESTION_ADVANCE_DELAY: Duration = Duration::from_millis(200);
/// The docked card's width (`THREAD_DETAILS_CARD_WIDTH`) and its inset from the canvas edges.
const DETAILS_CARD_WIDTH: Pixels = px(280.);
const DETAILS_CARD_GAP: Pixels = px(12.);
/// The details popover's `alignOffset` from the pane's right edge
/// (`THREAD_DETAILS_CARD_GAP - 8`); the popup's 0.5rem padding adds to it.
const DETAILS_POPOVER_ALIGN_OFFSET: Pixels = px(4.);

/// What the chat view asks the app to do.
#[derive(Clone, Debug)]
pub enum ChatViewEvent {
    /// A row of the header's "Thread actions" menu (other than "Find in thread",
    /// which the view handles): run it as the sidebar runs its context menu.
    ThreadCommand(ThreadKey, MenuCommand),
    /// Every question of the pending request has an answer: option indices in order.
    AnswerQuestions(Vec<usize>),
    /// A timeline button was pressed.
    Timeline(TimelineAction),
    /// A file or thread was dropped on the composer: insert this reference.
    InsertReference(String),
    /// A click outside the thread details popover.
    CloseDetails,
    /// The canvas's width moved the details card between docked and popover.
    DetailsModeChanged(ThreadDetailsMode),
}

/// What the header's "Thread actions" menu needs to know about the shown thread.
#[derive(Clone, Debug)]
pub struct HeaderThreadMenu {
    pub key: ThreadKey,
    pub branch: Option<SharedString>,
    pub is_pinned: bool,
    pub is_settled: bool,
    pub is_snoozed: bool,
    pub is_running: bool,
    pub auto_settle_enabled: bool,
    /// The snooze submenu, resolved when the row was read.
    pub snooze_presets: Vec<crate::state::sidebar_actions::ResolvedSnoozePreset>,
}

impl HeaderThreadMenu {
    /// The facts for a sidebar row, as the row's own context menu reads them.
    pub fn from_row(
        row: &SidebarThread,
        now: TimestampMs,
        auto_settle_enabled: bool,
        snooze_presets: Option<&[t3code_contracts::SnoozePresetSetting]>,
    ) -> Self {
        use crate::components::sidebar_logic::{ThreadStatus, effectively_snoozed, resolve_thread_status};
        Self {
            key: row.key.clone(),
            branch: row.branch.clone(),
            is_pinned: row.pinned_at.is_some(),
            is_settled: row.settled_override == Some(SettledOverride::Settled),
            is_snoozed: effectively_snoozed(row, now),
            is_running: matches!(resolve_thread_status(row), ThreadStatus::Working | ThreadStatus::Preparing),
            auto_settle_enabled,
            snooze_presets: crate::state::sidebar_actions::resolve_snooze_presets(
                snooze_presets,
                &jiff::Timestamp::from_millisecond(now.0)
                    .map(|t| t.to_zoned_datetime(jiff::tz::TimeZone::system()))
                    .unwrap_or_else(|_| jiff::ZonedDateTime::now()),
            ),
        }
    }
}

/// Where the user is in a multi-question request.
#[derive(Default)]
struct QuestionProgress {
    index: usize,
    answers: Vec<Option<usize>>,
}

/// The chat view of one thread.
pub struct ChatView {
    timeline: Entity<MessagesTimeline>,
    project: SharedString,
    title: SharedString,
    /// The window's topbar height and the header's left inset (sidebar collapsed).
    header_height: Pixels,
    header_inset: Pixels,
    /// A new thread with no messages yet.
    hero: bool,
    /// The composer view, mounted by the composer module. A placeholder of the
    /// same shape draws until then.
    composer: Option<AnyView>,
    composer_focus: Option<FocusHandle>,
    /// The branch and worktree strip under the composer.
    toolbar: Option<AnyView>,
    /// The shown thread's facts for the header menu; `None` for drafts.
    thread_menu: Option<HeaderThreadMenu>,
    /// The open "Thread actions" menu.
    header_menu: Option<ContextMenu>,
    /// The thread details card, while open.
    details: Option<AnyView>,
    /// Where the card shows at this canvas width (`resolveThreadDetailsCardLayout`).
    details_mode: ThreadDetailsMode,
    /// The banner pinned over the top of the timeline (`ChatBannerLane`), while one shows.
    banner: Option<AnyView>,
    /// The breadcrumb's project icon (override and favicon); the monogram without one.
    project_icon: Option<crate::components::project_favicon::ProjectIconSource>,
    thread_key: SharedString,
    find_bar: Option<(Entity<ThreadFindBar>, Subscription)>,
    /// Every question of the pending request.
    questions: Arc<[PendingQuestion]>,
    question: QuestionProgress,
    advance: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ChatView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let timeline = cx.new(MessagesTimeline::new);
        cx.observe(&timeline, |_, _, cx| cx.notify()).detach();
        cx.observe_global::<crate::hooks::use_settings::ClientSettingsStore>(|_, cx| cx.notify()).detach();
        let actions = cx.subscribe(&timeline, |_, _, action: &TimelineAction, cx| cx.emit(ChatViewEvent::Timeline(action.clone())));
        let counted = cx.subscribe(&timeline, |this, timeline, _: &FindUpdated, cx| {
            let (count, counting) = (timeline.read(cx).find_count(), timeline.read(cx).find_counting());
            if let Some((bar, _)) = &this.find_bar {
                bar.update(cx, |bar, cx| bar.set_count(count.0, count.1, counting, cx));
            }
        });
        Self {
            timeline,
            project: SharedString::default(),
            title: SharedString::default(),
            header_height: px(52.),
            header_inset: px(0.),
            hero: false,
            composer: None,
            composer_focus: None,
            toolbar: None,
            thread_menu: None,
            header_menu: None,
            details: None,
            details_mode: ThreadDetailsMode::Popover,
            banner: None,
            project_icon: None,
            thread_key: SharedString::default(),
            find_bar: None,
            questions: Arc::from([]),
            question: QuestionProgress::default(),
            advance: None,
            _subscriptions: vec![actions, counted],
        }
    }

    /// Shows a thread: its breadcrumb and timeline. `draft` marks a new
    /// thread, which shows the hero while it has no messages.
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
        self.hero = false;
        self.thread_key = key.clone();
        self.timeline.update(cx, |timeline, cx| timeline.set_thread(key, input, cx));
        // Find belongs to the thread it was opened in.
        if !ThreadFindStore::is_open_in(cx, &self.thread_key) {
            self.find_bar = None;
        }
        cx.notify();
    }

    /// The questions of the pending request; a new request starts at its first question.
    pub fn set_questions(&mut self, questions: Arc<[PendingQuestion]>, cx: &mut Context<Self>) {
        let same = questions.first().map(|q| &q.request_id) == self.questions.first().map(|q| &q.request_id);
        if !same || questions.len() != self.question.answers.len() {
            self.question = QuestionProgress { index: 0, answers: vec![None; questions.len()] };
            self.advance = None;
        }
        self.questions = questions;
        cx.notify();
    }

    /// Picks an option of the shown question (a click or a digit key); a
    /// single-select question moves on after a short pause.
    pub fn pick_option(&mut self, option: usize, cx: &mut Context<Self>) {
        let index = self.question.index;
        let Some(question) = self.questions.get(index) else { return };
        if option >= question.options.len() {
            return;
        }
        self.question.answers[index] = Some(option);
        cx.notify();
        self.advance = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUESTION_ADVANCE_DELAY).await;
            this.update(cx, |this, cx| this.advance_question(cx)).ok();
        }));
    }

    /// Next question, or submits the answers on the last one (`onAdvance`).
    pub fn advance_question(&mut self, cx: &mut Context<Self>) {
        self.advance = None;
        let index = self.question.index;
        if self.question.answers.get(index).copied().flatten().is_none() {
            return;
        }
        if index + 1 < self.questions.len() {
            self.question.index += 1;
        } else if let Some(answers) = self.question.answers.iter().copied().collect::<Option<Vec<_>>>() {
            cx.emit(ChatViewEvent::AnswerQuestions(answers));
        }
        cx.notify();
    }

    pub fn previous_question(&mut self, cx: &mut Context<Self>) {
        self.advance = None;
        self.question.index = self.question.index.saturating_sub(1);
        cx.notify();
    }

    /// The shown question's position, for checks.
    pub fn question_index(&self) -> usize {
        self.question.index
    }

    /// Opens find in this thread, or refocuses it (`chat.find`).
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_key.is_empty() || self.hero {
            return;
        }
        ThreadFindStore::open(cx, self.thread_key.clone());
        self.timeline.update(cx, |timeline, cx| timeline.prepare_find(cx));
        let bar = match &self.find_bar {
            Some((bar, _)) => bar.clone(),
            None => {
                let bar = cx.new(|cx| ThreadFindBar::new(window, cx));
                let subscription = cx.subscribe_in(&bar, window, Self::on_find_event);
                self.find_bar = Some((bar.clone(), subscription));
                bar
            }
        };
        bar.update(cx, |bar, cx| bar.focus(window, cx));
        cx.notify();
    }

    pub fn find_bar(&self) -> Option<&Entity<ThreadFindBar>> {
        self.find_bar.as_ref().map(|(bar, _)| bar)
    }

    fn on_find_event(&mut self, bar: &Entity<ThreadFindBar>, event: &ThreadFindBarEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = match event {
            ThreadFindBarEvent::QueryChanged(query) => {
                ThreadFindStore::set_query(cx, query.clone());
                let query = query.clone();
                self.timeline.update(cx, |timeline, cx| timeline.set_find_query(&query, cx))
            }
            ThreadFindBarEvent::Step(delta) => {
                let delta = *delta;
                self.timeline.update(cx, |timeline, cx| timeline.step_find(delta, cx))
            }
            ThreadFindBarEvent::Close => return self.close_find(window, cx),
        };
        let counting = self.timeline.read(cx).find_counting();
        bar.update(cx, |bar, cx| bar.set_count(count.0, count.1, counting, cx));
    }

    /// Closes find and puts focus back in the composer.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ThreadFindStore::close(cx);
        self.find_bar = None;
        self.timeline.update(cx, |timeline, cx| timeline.clear_find(cx));
        if let Some(focus) = &self.composer_focus {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    /// Sets what the header's "Thread actions" menu offers (`None` hides it, as on a draft).
    pub fn set_thread_menu(&mut self, menu: Option<HeaderThreadMenu>, cx: &mut Context<Self>) {
        if menu.is_none() {
            self.header_menu = None;
        }
        self.thread_menu = menu;
        cx.notify();
    }

    /// Opens or closes the header's "Thread actions" menu (`buildThreadActionMenuItems` with find).
    fn toggle_header_menu(&mut self, cx: &mut Context<Self>) {
        if self.header_menu.take().is_some() {
            cx.notify();
            return;
        }
        let Some(facts) = self.thread_menu.as_ref() else { return };
        let items = context_menu::thread_menu_items(&context_menu::ThreadMenuState {
            branch: facts.branch.as_deref(),
            is_pinned: facts.is_pinned,
            is_settled: facts.is_settled,
            is_snoozed: facts.is_snoozed,
            is_running: facts.is_running,
            project_label: &self.project,
            project_filter_active: false,
            show_project_filter: false,
            can_find: true,
            auto_settle_enabled: facts.auto_settle_enabled,
            snooze_presets: &facts.snooze_presets,
        });
        self.header_menu = Some(ContextMenu {
            target: MenuTarget::Thread(facts.key.clone()),
            position: gpui_kit::Point::default(),
            items,
            highlighted: None,
            submenu: None,
        });
        cx.notify();
    }

    fn run_header_menu_item(&mut self, ix: usize, child: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.header_menu.take() else { return };
        match (context_menu::picked_command(&menu, ix, child), menu.target) {
            (Some(MenuCommand::FindInThread), _) => self.open_find(window, cx),
            (Some(command), MenuTarget::Thread(key)) => cx.emit(ChatViewEvent::ThreadCommand(key, command)),
            _ => {}
        }
        cx.notify();
    }

    /// The open header menu, below the title (`DropdownMenu` align start, 4px offset).
    fn render_header_menu(&mut self, _window: &Window, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let menu = self.header_menu.clone()?;
        let colors = ShellColors::from_theme(cx.app_theme());
        let body = context_menu::menu_body(
            &menu,
            &colors,
            |this: &mut Self| this.header_menu.as_mut(),
            |this, ix, child, window, cx| this.run_header_menu_item(ix, child, window, cx),
            cx,
        )
        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.header_menu = None;
            cx.notify();
        }));
        Some(gpui_kit::deferred(div().absolute().top_full().left_0().pt(rems(0.25)).child(body)).with_priority(2).into_any_element())
    }

    /// Mounts the branch toolbar under the composer.
    pub fn set_toolbar(&mut self, toolbar: Option<AnyView>, cx: &mut Context<Self>) {
        self.toolbar = toolbar;
        cx.notify();
    }

    /// Where the details card shows now.
    pub fn details_mode(&self) -> ThreadDetailsMode {
        self.details_mode
    }

    /// `resolveThreadDetailsCardLayout`: the card docks when the canvas keeps the chat lane's
    /// minimum width (`min-w-[40rem]` plus `sm:ps-12`) beside the 280px card, its 12px gap
    /// and the 32px clearance, and has 160px of height under the header.
    fn measure_canvas(&mut self, size: gpui_kit::Size<Pixels>, cx: &mut Context<Self>) {
        let theme = crate::ActiveAppTheme::app_theme(&**cx);
        let fits_width = size.width - DETAILS_CARD_WIDTH - DETAILS_CARD_GAP - px(32.) - theme.rem(3.) >= theme.rem(40.);
        let fits_height = size.height - self.header_height - DETAILS_CARD_GAP * 2. >= px(160.);
        let mode = if fits_width && fits_height { ThreadDetailsMode::Inline } else { ThreadDetailsMode::Popover };
        if mode != self.details_mode {
            self.details_mode = mode;
            cx.emit(ChatViewEvent::DetailsModeChanged(mode));
            cx.notify();
        }
    }

    /// Shows the thread details card (docked or as a popover, by [`Self::details_mode`]), or hides it.
    pub fn set_details(&mut self, details: Option<AnyView>, cx: &mut Context<Self>) {
        self.details = details;
        cx.notify();
    }

    /// The breadcrumb's project icon source, as drawn.
    pub fn project_icon(&self) -> Option<&crate::components::project_favicon::ProjectIconSource> {
        self.project_icon.as_ref()
    }

    /// The breadcrumb's project icon source.
    pub fn set_project_icon(&mut self, icon: Option<crate::components::project_favicon::ProjectIconSource>, cx: &mut Context<Self>) {
        if self.project_icon != icon {
            self.project_icon = icon;
            cx.notify();
        }
    }

    /// Shows a banner over the top of the timeline, or none. While one shows the timeline's top
    /// fade is off (`topFadeEnabled={!hasTimelineTopBanner}`).
    pub fn set_banner(&mut self, banner: Option<AnyView>, cx: &mut Context<Self>) {
        if self.banner.as_ref().map(|b| b.entity_id()) != banner.as_ref().map(|b| b.entity_id()) {
            self.banner = banner;
            cx.notify();
        }
    }

    /// Digit keys pick a question option when focus is not in a text field.
    /// The app root forwards keys from outside the chat here too, as the web
    /// listens on the whole document.
    pub fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if self.header_menu.is_some() && keystroke.key == "escape" {
            self.header_menu = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.questions.is_empty() || keystroke.modifiers.modified() || self.find_bar.is_some() {
            return;
        }
        if self.composer_focus.as_ref().is_some_and(|focus| focus.contains_focused(window, cx)) {
            return;
        }
        if let Ok(digit @ 1..=9) = keystroke.key.parse::<usize>() {
            self.pick_option(digit - 1, cx);
            cx.stop_propagation();
        }
    }

    /// Shows the draft hero (new thread, no messages) or the timeline.
    pub fn set_hero(&mut self, hero: bool, cx: &mut Context<Self>) {
        if self.hero != hero {
            self.hero = hero;
            cx.notify();
        }
    }

    /// The topbar height and the header's left inset.
    pub fn set_header_metrics(&mut self, height: Pixels, inset: Pixels, cx: &mut Context<Self>) {
        if (self.header_height, self.header_inset) != (height, inset) {
            (self.header_height, self.header_inset) = (height, inset);
            cx.notify();
        }
    }

    /// Mounts the composer in the overlay slot.
    pub fn set_composer(&mut self, composer: AnyView, focus: FocusHandle, cx: &mut Context<Self>) {
        self.composer = Some(composer);
        self.composer_focus = Some(focus);
        cx.notify();
    }

    pub fn timeline(&self) -> &Entity<MessagesTimeline> {
        &self.timeline
    }

    /// The draft hero's headline (`DraftHeroHeadline.tsx`).
    fn hero_headline(colors: &ChatColors) -> impl IntoElement {
        div()
            .pb_4()
            .w_full()
            .text_center()
            .text_size(colors.r(HERO_HEADLINE_SIZE))
            .line_height(colors.r(HERO_HEADLINE_LINE))
            .font_weight(FontWeight::NORMAL)
            .text_color(colors.theme.text)
            .child(t!("What should we work on?"))
    }

    /// The composer's stand-in: the glass card shape with its placeholder text.
    fn composer_placeholder(colors: &ChatColors) -> impl IntoElement {
        let outline = super::chat::timeline_row_parts::mix(colors.theme.canvas, colors.theme.input, 0.3);
        div()
            .w_full()
            .h(colors.r(COMPOSER_PLACEHOLDER_HEIGHT))
            .rounded(rems(1.))
            .border_1()
            .border_color(outline)
            .glass(colors.composer_glass)
            .px_4()
            .pt_4()
            .text_size(TEXT_SM.size)
            .line_height(rems(1.421875))
            .text_color(colors.theme.placeholder)
            .child(t!("Ask anything. @ files, $ skills, / commands"))
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ChatColors::get(cx);
        let lane = crate::components::chat::chat_style::ChatLane::get(cx);
        let gutter = colors.r(if window.viewport_size().width >= SM_BREAKPOINT { TIMELINE_GUTTER } else { TIMELINE_GUTTER_NARROW });
        let timeline = self.timeline.clone();
        let (show_scroll_button, pending_question) = {
            let timeline = self.timeline.read(cx);
            (timeline.scroll_button_visible(), timeline.input().pending_question.clone())
        };
        let measure_timeline = timeline.clone();
        let weak = cx.weak_entity();
        let on_pick: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new(move |index, _, cx| {
            weak.update(cx, |this, cx| this.pick_option(index, cx)).ok();
        });
        // The shown question of a multi-question request, with its pick.
        let question_index = self.question.index;
        let pending_question = pending_question.map(|first| self.questions.get(question_index).cloned().unwrap_or(first));
        let selected = self.question.answers.get(question_index).copied().flatten();
        let question_nav = (pending_question.is_some() && self.questions.len() > 1).then(|| {
            let last = question_index + 1 >= self.questions.len();
            let label = if !last { t!("Next question") } else { t!("Submit answers") };
            div()
                .flex()
                .justify_end()
                .gap_1()
                .px_3()
                .pb_1()
                .when(question_index > 0, |d| {
                    d.child(
                        Button::new("question-previous")
                            .icon(IconName::ArrowLeft)
                            .size(ButtonSize::IconXs)
                            .variant(ButtonVariant::GhostMuted)
                            .on_click(cx.listener(|this, _, _, cx| this.previous_question(cx))),
                    )
                })
                .child(
                    Button::new("question-next")
                        .label(label)
                        .size(ButtonSize::Xs)
                        .variant(ButtonVariant::Ghost)
                        .disabled(selected.is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.advance_question(cx))),
                )
        });
        let hero = self.hero && window.viewport_size().height >= colors.r(HERO_MIN_HEIGHT);

        let composer_overlay = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .when(hero, |d| d.top_0().flex().items_center())
            .pt(colors.r(COMPOSER_TOP_PAD))
            .px(gutter)
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .when_some(lane.composer, |d, w| d.max_w(w))
                    .flex()
                    .flex_col()
                    .when(self.hero, |d| d.child(Self::hero_headline(&colors)))
                    .children(pending_question.as_ref().map(|question| {
                        div()
                            .mx(rems(1.))
                            .rounded_t(rems(0.75))
                            .border_1()
                            .border_b_0()
                            .border_color(alpha(colors.theme.border, 0.8))
                            .glass(colors.composer_glass)
                            .pb_2()
                            .child(pending_user_input_panel(question, selected, on_pick.clone(), &colors))
                            .children(question_nav)
                    }))
                    .child(
                        div()
                            .id("composer-drop")
                            .test_support()
                            .w_full()
                            .on_drop(cx.listener(|_, drag: &FileReferenceDrag, _, cx| {
                                cx.emit(ChatViewEvent::InsertReference(drag.mention()))
                            }))
                            .on_drop(cx.listener(|_, drag: &PaletteThreadDrag, _, cx| {
                                let id = format!("{}:{}", drag.thread.environment_id.0, drag.thread.thread_id.0);
                                cx.emit(ChatViewEvent::InsertReference(format_context_reference("thread", &id, &drag.title)))
                            }))
                            .child(match &self.composer {
                                Some(composer) => composer.clone().into_any_element(),
                                None => Self::composer_placeholder(&colors).into_any_element(),
                            })
                            // `shouldShowComposerContextStrip`: a new thread always shows the
                            // strip; a started one only with "persist context strip" on.
                            .when(self.hero || cx.client_settings().persist_composer_context_strip, |d| {
                                d.children(self.toolbar.clone())
                            }),
                    )
                    .child(div().h(colors.r(COMPOSER_BOTTOM_SPACER))),
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
                    .h(colors.r(SCROLL_BUTTON_HEIGHT))
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
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::Sm).color(colors.theme.text))
                    .child(t!("Scroll to end"))
                    .on_click(move |_, _, cx| timeline.update(cx, |timeline, cx| timeline.scroll_to_end(cx))),
            )
        });

        let top_fade = div()
            .absolute()
            .top_0()
            .left_0()
            .right(SCROLLBAR_LANE)
            .h(colors.r(TIMELINE_TOP_INSET))
            .bg(linear_gradient(
                180.,
                linear_color_stop(colors.theme.canvas, 0.),
                linear_color_stop(alpha(colors.theme.canvas, 0.), 1.),
            ));

        let header_height = self.header_height;
        let details_padding = crate::ActiveAppTheme::app_theme(&**cx).rem(0.5);
        let inline = self.details_mode == ThreadDetailsMode::Inline;
        let details = self.details.clone().map(|details| {
            if inline {
                return div()
                    .id("thread-details-inline")
                    .absolute()
                    .top(header_height + DETAILS_CARD_GAP)
                    .right(DETAILS_CARD_GAP)
                    .bottom(DETAILS_CARD_GAP)
                    .w(DETAILS_CARD_WIDTH)
                    .child(details);
            }
            div()
                .id("thread-details-popover")
                .occlude()
                .absolute()
                .top(header_height)
                .right(DETAILS_POPOVER_ALIGN_OFFSET)
                .p(details_padding)
                .on_mouse_down_out(cx.listener(move |_, event: &gpui_kit::MouseDownEvent, _, cx| {
                    // The titlebar toggle closes it itself.
                    if event.position.y > header_height {
                        cx.emit(ChatViewEvent::CloseDetails);
                    }
                }))
                .child(details)
        });
        let thread_actions = self.thread_menu.is_some().then(|| ThreadActionsTrigger {
            open: self.header_menu.is_some(),
            on_click: Box::new(cx.listener(|this, _, _, cx| this.toggle_header_menu(cx))),
            menu: self.render_header_menu(window, cx),
        });
        let find_bar = self.find_bar.as_ref().map(|(bar, _)| div().absolute().top(header_height).left_0().right_0().child(bar.clone()));
        div()
            .id("chat-view")
            .key_context("ChatView")
            .on_action(cx.listener(|this, _: &FindInThread, window, cx| this.open_find(window, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .bg(colors.theme.canvas)
            .text_color(colors.theme.text)
            .font_family(super::chat::chat_style::SANS_FAMILY)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(chat_header(&self.project, self.project_icon.as_ref(), &self.title, self.header_height, self.header_inset, &colors, thread_actions))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .when(!self.hero, |d| {
                                d.child(div().flex().flex_col().flex_1().min_h_0().child(self.timeline.clone()))
                            })
                            .when(!self.hero, |d| d.when(self.banner.is_none(), |d| d.child(top_fade)).children(scroll_button))
                            .child(composer_overlay)
                            .children(self.banner.clone().map(|banner| div().absolute().top_0().left_0().right_0().child(banner))),
                    ),
            )
            .children(find_bar)
            .children(details)
            .child({
                let this = cx.entity();
                gpui_kit::canvas(
                    |bounds, _, _| bounds.size,
                    move |_, size, _, cx| this.update(cx, |this, cx| this.measure_canvas(size, cx)),
                )
                .absolute()
                .size_full()
            })
    }
}

impl EventEmitter<ChatViewEvent> for ChatView {}

/// Creates a chat view entity.
pub fn chat_view(cx: &mut App) -> Entity<ChatView> {
    cx.new(ChatView::new)
}
