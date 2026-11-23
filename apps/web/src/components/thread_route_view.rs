//! The chat for a thread route. Ports `ThreadRouteView.tsx`: one chat view
//! and one composer stay mounted across thread switches (the composer keeps a
//! draft per thread key), and switching only swaps the data they show. The
//! composer's events go to the fixture store until the sync layer replaces it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;

use crate::components::chat::composer::{ChatComposer, ComposerContextStrip, ComposerEvent, ComposerState};
use crate::components::chat_view::{ChatView, ChatViewEvent};
use crate::composer_draft_store::{ComposerDraftKey, ComposerDraftStore, InMemoryDraftStore};
use crate::fixtures::{FixtureEvent, FixtureStore, ThreadView};
use crate::router::ThreadTarget;
use crate::scenes::composer::{FixtureMenuSource, fixture_state};
use crate::i18n::t;

/// What the route view asks the app to do.
#[derive(Clone, Debug)]
pub enum ThreadRouteEvent {
    /// A draft got its first message; the route should become the thread's.
    Promoted(ThreadTarget),
}

/// The chat view for whichever thread the route names.
pub struct ThreadRouteView {
    store: Entity<FixtureStore>,
    chat: Entity<ChatView>,
    composer: Entity<ChatComposer>,
    target: Option<ThreadTarget>,
    /// The composer settings that do not depend on the thread.
    base: ComposerState,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadRouteEvent> for ThreadRouteView {}

impl ThreadRouteView {
    pub fn new(store: Entity<FixtureStore>, cx: &mut Context<Self>) -> Self {
        let base = fixture_state("none");
        let drafts: Rc<RefCell<dyn ComposerDraftStore>> = Rc::new(RefCell::new(InMemoryDraftStore::default()));
        let composer = {
            let state = base.clone();
            cx.new(|cx| ChatComposer::new(state, Rc::new(FixtureMenuSource), drafts, cx))
        };
        let chat = cx.new(ChatView::new);
        chat.update(cx, |chat, cx| chat.set_composer(composer.clone().into(), cx));
        let subscriptions = vec![
            cx.subscribe(&composer, Self::on_composer_event),
            cx.subscribe(&chat, Self::on_chat_event),
            cx.subscribe(&store, Self::on_store_event),
        ];
        Self { store, chat, composer, target: None, base, _subscriptions: subscriptions }
    }

    pub fn chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    pub fn composer(&self) -> &Entity<ChatComposer> {
        &self.composer
    }

    pub fn target(&self) -> Option<&ThreadTarget> {
        self.target.as_ref()
    }

    /// Shows `target`. Returns false when the store has no such thread.
    pub fn show(&mut self, target: ThreadTarget, cx: &mut Context<Self>) -> bool {
        if self.target.as_ref() == Some(&target) {
            return true;
        }
        let Some(view) = self.store.update(cx, |store, _| store.thread_view(&target)) else { return false };
        self.target = Some(target);
        self.chat.update(cx, |chat, cx| {
            let hero = view.is_draft && view.timeline.entries.is_empty();
            chat.set_thread(view.key.clone(), view.project.clone(), view.title.clone(), view.timeline.clone(), cx);
            chat.set_hero(hero, cx);
        });
        self.sync_composer(&view, cx);
        cx.notify();
        true
    }

    /// Re-reads the shown thread after the store changed it.
    fn refresh(&mut self, anchor: bool, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        let Some(view) = self.store.update(cx, |store, _| store.thread_view(&target)) else { return };
        let timeline = self.chat.read(cx).timeline().clone();
        timeline.update(cx, |timeline, cx| {
            timeline.update_input(view.timeline.clone(), cx);
            if anchor {
                timeline.anchor_latest_user_message(cx);
            }
        });
        let hero = view.is_draft && view.timeline.entries.is_empty();
        self.chat.update(cx, |chat, cx| chat.set_hero(hero, cx));
        self.sync_composer(&view, cx);
    }

    fn sync_composer(&mut self, view: &ThreadView, cx: &mut Context<Self>) {
        let input = &view.timeline;
        let placeholder = if input.pending_question.is_some() {
            Some(t!("Type your own answer, or leave this blank to use the selected option"))
        } else if !input.pending_approval_ids.is_empty() {
            Some(t!("Send a message to decline and redirect the agent"))
        } else {
            None
        };
        let state = ComposerState {
            draft_key: ComposerDraftKey(view.key.clone()),
            is_draft_thread: view.is_draft,
            running: view.running,
            can_interrupt: view.running,
            context_strip: view.is_draft.then(|| ComposerContextStrip {
                environment: t!("This machine"),
                workspace: view.project.clone(),
                branch: "main".into(),
            }),
            placeholder,
            ..self.base.clone()
        };
        self.composer.update(cx, |composer, cx| composer.set_state(state, cx));
    }

    fn on_composer_event(&mut self, _: Entity<ChatComposer>, event: &ComposerEvent, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        match event {
            ComposerEvent::Submit { prompt, dispatch, .. } => {
                let (prompt, dispatch) = (prompt.clone(), *dispatch);
                let sent_to = self.store.update(cx, |store, cx| store.submit(&target, prompt, dispatch, cx));
                if let (ThreadTarget::Draft(_), Some(key)) = (&target, sent_to) {
                    // The draft is now a server thread: show it under its own key.
                    let promoted = ThreadTarget::Server(key);
                    self.target = None;
                    self.show(promoted.clone(), cx);
                    cx.emit(ThreadRouteEvent::Promoted(promoted));
                }
                self.refresh(true, cx);
            }
            ComposerEvent::Stop => {
                if let ThreadTarget::Server(key) = &target {
                    self.store.update(cx, |store, cx| store.stop(key, cx));
                }
            }
            ComposerEvent::PageScroll { .. }
            | ComposerEvent::ClearSession
            | ComposerEvent::SideChat { .. }
            | ComposerEvent::AttachRequested
            | ComposerEvent::AttachmentsAdded(_)
            | ComposerEvent::AttachmentRemoved(_)
            | ComposerEvent::ModelChanged(_)
            | ComposerEvent::RuntimeModeChanged(_)
            | ComposerEvent::FavoriteToggled { .. }
            | ComposerEvent::VoiceRequested
            | ComposerEvent::ContextStripClicked(_) => {}
        }
    }

    fn on_chat_event(&mut self, _: Entity<ChatView>, event: &ChatViewEvent, cx: &mut Context<Self>) {
        let ChatViewEvent::AnswerQuestion(index) = event;
        if let Some(ThreadTarget::Server(key)) = self.target.clone() {
            self.store.update(cx, |store, cx| store.answer_questions(&key, &[*index], cx));
        }
    }

    fn on_store_event(&mut self, _: Entity<FixtureStore>, event: &FixtureEvent, cx: &mut Context<Self>) {
        match event {
            FixtureEvent::ThreadChanged(target) if self.target.as_ref() == Some(target) => self.refresh(false, cx),
            FixtureEvent::Delta { thread, message, text } => {
                if self.target.as_ref() == Some(&ThreadTarget::Server(thread.clone())) {
                    let timeline = self.chat.read(cx).timeline().clone();
                    timeline.update(cx, |timeline, cx| timeline.update_message_text(message, text.clone(), true, cx));
                }
            }
            FixtureEvent::DraftPromoted { draft, thread } => {
                if self.target.as_ref() == Some(&ThreadTarget::Draft(draft.clone())) {
                    let target = ThreadTarget::Server(thread.clone());
                    self.target = Some(target.clone());
                    cx.emit(ThreadRouteEvent::Promoted(target));
                }
            }
            FixtureEvent::ThreadChanged(_) | FixtureEvent::SnapshotChanged => {}
        }
    }
}

impl Render for ThreadRouteView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.chat.clone()
    }
}
