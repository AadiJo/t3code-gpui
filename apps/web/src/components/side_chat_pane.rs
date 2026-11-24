//! A side chat tab in the right panel. Ports `SideChatPane.tsx`: a 2.25rem header with the side
//! chat's title, "Add to chat", Delete and Close, over the side chat's own chat (`ChatView`
//! embedded: no header, its own timeline and composer). Its stream runs beside the main pane's
//! thread through [`AppData::side_chat_view`]; a side chat deleted elsewhere says so.

use gpui_kit::*;

use crate::components::chat::composer::{ChatComposer, ComposerEvent, ComposerState};
use crate::components::chat::composer_command_menu::{ComposerCommandItem, ComposerMenuSource, SlashMenuContext};
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::chat_view::{ChatView, ChatViewEvent};
use crate::components::icons::IconName;
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::tooltip;
use crate::appearance_fonts::{TextSize, weight};
use crate::composer_editor_mentions::format_context_reference;
use crate::composer_logic::ComposerTrigger;
use crate::i18n::t;
use crate::router::ThreadTarget;
use crate::scenes::composer::fixture_state;
use crate::state::app_data::{ActionOutcome, DataEvent, SharedAppData, SideChatLookup};
use crate::ActiveAppTheme as _;
use crate::hooks::use_settings::ClientSettingsStore;

/// What the pane asks the right panel to do.
#[derive(Clone, Debug)]
pub enum SideChatPaneEvent {
    /// Close this tab (the X, or the side chat was deleted).
    Close,
    /// Add this reference to the main thread's composer ("Add to chat").
    AddToChat(String),
}

/// The composer's `@`, `$` and `/` source, set once the view arrives.
#[derive(Clone, Default)]
struct PaneMenu(std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<dyn ComposerMenuSource>>>>);

impl ComposerMenuSource for PaneMenu {
    fn items(&self, trigger: &ComposerTrigger) -> (Vec<ComposerCommandItem>, bool) {
        let source = self.0.borrow().clone();
        source.map(|s| s.items(trigger)).unwrap_or_default()
    }

    fn slash_items(&self, trigger: &ComposerTrigger, context: &SlashMenuContext) -> Option<(Vec<ComposerCommandItem>, bool)> {
        let source = self.0.borrow().clone();
        source.and_then(|s| s.slash_items(trigger, context))
    }

    fn empty_text(&self, trigger: &ComposerTrigger) -> Option<SharedString> {
        let source = self.0.borrow().clone();
        source.and_then(|s| s.empty_text(trigger))
    }
}

/// One side chat's tab body.
pub struct SideChatPane {
    data: SharedAppData,
    key: ThreadKey,
    chat: Entity<ChatView>,
    composer: Entity<ChatComposer>,
    base: ComposerState,
    menu: PaneMenu,
    /// The side chat's view arrived and the chat shows it.
    shown: bool,
    confirming_delete: bool,
    deleting: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SideChatPaneEvent> for SideChatPane {}

impl SideChatPane {
    pub fn new(data: SharedAppData, key: ThreadKey, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let base = fixture_state("none");
        let drafts = data.draft_store(cx);
        let menu = PaneMenu::default();
        let composer = {
            let (state, menu) = (base.clone(), menu.clone());
            cx.new(|cx| ChatComposer::new(state, std::rc::Rc::new(menu), drafts, cx))
        };
        let chat = cx.new(ChatView::new);
        let focus = composer.focus_handle(cx);
        chat.update(cx, |chat, cx| {
            chat.set_embedded(true, cx);
            chat.set_composer(composer.clone().into(), focus, cx);
        });
        let subscriptions = vec![
            cx.subscribe(&composer, Self::on_composer_event),
            cx.subscribe_in(&chat, window, Self::on_chat_event),
            cx.subscribe(&data.events(), Self::on_data_event),
        ];
        let mut this = Self { data, key, chat, composer, base, menu, shown: false, confirming_delete: false, deleting: false, _subscriptions: subscriptions };
        this.show(cx);
        this
    }

    pub fn key(&self) -> &ThreadKey {
        &self.key
    }

    pub fn chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    pub fn composer(&self) -> &Entity<ChatComposer> {
        &self.composer
    }

    fn target(&self) -> ThreadTarget {
        ThreadTarget::Server(self.key.clone())
    }

    /// Opens the side chat's stream and shows it, once it is in the shell.
    fn show(&mut self, cx: &mut Context<Self>) {
        if self.shown || !matches!(self.data.side_chat(&self.key, cx), SideChatLookup::Found(_)) {
            return;
        }
        let Some(view) = self.data.side_chat_view(&self.key, cx) else { return };
        self.shown = true;
        *self.menu.0.borrow_mut() = Some(self.data.menu_source(&view, &self.composer, cx));
        let timeline = self.chat.read(cx).timeline().clone();
        timeline.update(cx, |timeline, cx| {
            timeline.set_now_millis(t3code_client_runtime::state::models::now_ms());
            timeline.set_top_inset(px(8.), cx);
        });
        self.chat.update(cx, |chat, cx| {
            chat.set_thread(view.key.clone(), view.project.clone(), view.title.clone(), view.timeline.clone(), cx);
            chat.set_questions(view.questions.clone(), cx);
        });
        self.sync_composer(cx);
        cx.notify();
    }

    fn refresh(&mut self, anchor: bool, cx: &mut Context<Self>) {
        if !self.shown {
            return self.show(cx);
        }
        let Some(view) = self.data.side_chat_view(&self.key, cx) else { return };
        let timeline = self.chat.read(cx).timeline().clone();
        timeline.update(cx, |timeline, cx| {
            // Running timers ("Working for 8s") read wall-clock milliseconds.
            timeline.set_now_millis(t3code_client_runtime::state::models::now_ms());
            timeline.update_input(view.timeline.clone(), cx);
            if anchor {
                timeline.anchor_latest_user_message(cx);
            }
        });
        self.chat.update(cx, |chat, cx| chat.set_questions(view.questions.clone(), cx));
        self.sync_composer(cx);
    }

    fn sync_composer(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.data.side_chat_view(&self.key, cx) else { return };
        let placeholder = if view.timeline.pending_question.is_some() {
            Some(t!("Type your own answer, or leave this blank to use the selected option"))
        } else if !view.timeline.pending_approval_ids.is_empty() {
            Some(t!("Send a message to decline and redirect the agent"))
        } else {
            None
        };
        let state = self.data.composer_state(&view, &self.base, cx);
        let state = ComposerState { context_strip: None, placeholder, ..state };
        self.composer.update(cx, |composer, cx| composer.set_state(state, cx));
    }

    fn on_composer_event(&mut self, composer: Entity<ChatComposer>, event: &ComposerEvent, cx: &mut Context<Self>) {
        self.data.composer_event(&self.target(), event, &composer.downgrade(), cx);
        match event {
            ComposerEvent::Submit { .. } => self.refresh(true, cx),
            ComposerEvent::ModelChanged(_) | ComposerEvent::RuntimeModeChanged(_) | ComposerEvent::ClearSession | ComposerEvent::BannerAction(_) => {
                self.refresh(false, cx)
            }
            _ => {}
        }
    }

    fn on_chat_event(&mut self, _: &Entity<ChatView>, event: &ChatViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ChatViewEvent::AnswerQuestions(answers) => self.data.answer_questions(&self.key, answers, cx),
            ChatViewEvent::InsertReference(text) => {
                let editor = self.composer.read(cx).editor().clone();
                editor.update(cx, |editor, cx| editor.insert(&format!("{text} "), cx));
                window.focus(&self.composer.focus_handle(cx), cx);
            }
            // Side chat runs keep no checkpoints to diff, and a fork would promote the side chat.
            ChatViewEvent::Timeline(TimelineAction::OpenDiff { .. } | TimelineAction::Fork(_)) => {}
            ChatViewEvent::Timeline(action) => match self.data.timeline_action(&self.key, action, cx) {
                ActionOutcome::LoadComposer(text) => {
                    let editor = self.composer.read(cx).editor().clone();
                    editor.update(cx, |editor, cx| {
                        editor.set_text(text.to_string(), cx);
                        editor.set_cursor(text.len(), cx);
                    });
                }
                ActionOutcome::Done | ActionOutcome::OpenThread(_) | ActionOutcome::ConfirmRevert { .. } => {}
            },
            ChatViewEvent::CloseDetails | ChatViewEvent::ThreadCommand(..) | ChatViewEvent::DetailsModeChanged(_) => {}
        }
    }

    fn on_data_event(&mut self, _: Entity<crate::state::app_data::DataEvents>, event: &DataEvent, cx: &mut Context<Self>) {
        match event {
            DataEvent::ThreadChanged(ThreadTarget::Server(key)) if key == &self.key => self.refresh(false, cx),
            DataEvent::Text { thread, changes } if thread == &self.key => {
                let timeline = self.chat.read(cx).timeline().clone();
                timeline.update(cx, |timeline, cx| {
                    for (message, text, streaming) in changes.iter() {
                        timeline.update_message_text(message, text.clone(), *streaming, cx);
                    }
                });
            }
            DataEvent::SnapshotChanged => {
                self.show(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    /// "Add to chat": a thread chip for the main thread's next message, once the side chat has
    /// a message (closing an unused one deletes it, which would leave the link unreadable).
    fn add_to_chat(&mut self, cx: &mut Context<Self>) {
        let SideChatLookup::Found(facts) = self.data.side_chat(&self.key, cx) else { return };
        if !facts.has_user_message {
            return;
        }
        let id = format!("{}:{}", self.key.environment_id.0, self.key.thread_id.0);
        cx.emit(SideChatPaneEvent::AddToChat(format_context_reference("thread", &id, &facts.title)));
    }

    fn request_delete(&mut self, cx: &mut Context<Self>) {
        if self.deleting || !matches!(self.data.side_chat(&self.key, cx), SideChatLookup::Found(_)) {
            return;
        }
        if ClientSettingsStore::get(cx).confirm_thread_delete {
            self.confirming_delete = true;
            return cx.notify();
        }
        self.delete(cx);
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        self.confirming_delete = false;
        self.deleting = true;
        cx.notify();
        let task = self.data.delete_thread(&self.key, false, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.deleting = false;
                match result {
                    Ok(()) => cx.emit(SideChatPaneEvent::Close),
                    Err(error) => {
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not delete the side chat")).description(error), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_delete_confirm(&self, title: SharedString, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.confirming_delete {
            return None;
        }
        let dialog = DialogSurface::new(t!("Delete side chat \"{title}\"?", title = title.as_ref()))
            .description(t!("This permanently clears its conversation history."))
            .footer(Button::new("side-chat-delete-cancel").variant(ButtonVariant::Outline).size(ButtonSize::Default).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                this.confirming_delete = false;
                cx.notify();
            })))
            .footer(Button::new("side-chat-delete-confirm").variant(ButtonVariant::Destructive).size(ButtonSize::Default).label(t!("Delete", context = "verb, deletes a thread")).on_click(cx.listener(|this, _, _, cx| this.delete(cx))));
        let this = cx.weak_entity();
        Some(
            deferred(Modal::dialog("side-chat-delete-dialog", dialog, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.confirming_delete = false;
                    cx.notify();
                })
                .ok();
            }))
            .with_priority(3)
            .into_any_element(),
        )
    }
}

impl Render for SideChatPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let lookup = self.data.side_chat(&self.key, cx);
        let facts = match &lookup {
            SideChatLookup::Found(facts) => Some(facts.clone()),
            _ => None,
        };
        let title = facts.as_ref().map(|f| f.title.clone()).unwrap_or_else(|| t!("Side chat"));
        let can_add = facts.as_ref().is_some_and(|f| f.has_user_message);
        let add = div()
            .id("side-chat-add")
            .tooltip(tooltip(if can_add {
                t!("Adds this side chat to the main thread's next message.")
            } else {
                t!("Available after the first message.")
            }))
            .child(
                Button::new("side-chat-add-button")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Xs)
                    .label(t!("Add to chat"))
                    .disabled(!can_add)
                    .on_click(cx.listener(|this, _, _, cx| this.add_to_chat(cx))),
            );
        let header = div()
            .flex()
            .flex_none()
            .h(theme.rem(2.25))
            .items_center()
            .gap(theme.rem(0.25))
            .pr(theme.rem(0.25))
            .pl(theme.rem(0.75))
            .border_b_1()
            .border_color(theme.colors.border)
            .child(div().flex_1().min_w_0().truncate().ui_text(&theme, TextSize::Xs, weight::MEDIUM).child(title.clone()))
            .child(add)
            .child(
                div().id("side-chat-delete-tip").tooltip(tooltip(t!("Delete side chat"))).child(
                    Button::new("side-chat-delete")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconXs)
                        .icon(IconName::Trash2)
                        .disabled(facts.is_none() || self.deleting)
                        .on_click(cx.listener(|this, _, _, cx| this.request_delete(cx))),
                ),
            )
            .child(
                div().id("side-chat-close-tip").tooltip(tooltip(t!("Close side chat"))).child(
                    Button::new("side-chat-close")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconXs)
                        .icon(IconName::X)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SideChatPaneEvent::Close))),
                ),
            );
        let body = match lookup {
            SideChatLookup::Found(_) if self.shown => div().flex_1().min_h_0().child(self.chat.clone()).into_any_element(),
            SideChatLookup::Deleted => div()
                .p(theme.rem(0.75))
                .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                .text_color(theme.colors.muted_foreground)
                .child(t!("This side chat was deleted."))
                .into_any_element(),
            _ => div().into_any_element(),
        };
        div()
            .id("side-chat-pane")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(body)
            .children(self.render_delete_confirm(title, cx))
    }
}
