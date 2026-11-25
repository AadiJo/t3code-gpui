//! Dictation in the composer (`ComposerSpeechButton.tsx`, `VoiceInputPill.tsx`,
//! `VoiceInputErrorPill.tsx`, `useDictationShortcut.ts`, the speech parts of
//! `ChatComposer.tsx`): the mic button while idle, the recording capsule with its elapsed time
//! and waveform, the error capsule, the live transcript above the footer, and the
//! `composer.dictation` press (tap to toggle; in Auto, a hold of 300 ms or more finishes on
//! release). The recording itself is the app's one [`VoiceInput`] session.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use t3code_client_runtime::voice_input::{VoiceErrorAction, VoiceInputErrorCode, VoiceInputPhase, VoiceInputState};
use t3code_contracts::VoiceShortcutMode;
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand};

use super::composer::{ChatComposer, ComposerPending};
use super::composer_theme::ComposerColors;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::components::ui::tooltip::tooltip;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;
use crate::keybindings::shortcut_label_for_command;
use crate::shortcuts::keymap::ShortcutState;
use crate::speech::voice_input_session::{VoiceInput, VoiceRecordingContext, VoiceTarget, WAVEFORM_BARS, voice_text};

/// `HOLD_THRESHOLD_MS`.
const HOLD_THRESHOLD: Duration = Duration::from_millis(300);
/// `WAVEFORM_REST_SCALE`.
const WAVEFORM_REST: f32 = 0.2;

/// A `composer.dictation` press that may finish on release.
#[derive(Clone, Copy, Debug)]
pub struct DictationPress {
    started: Instant,
    mode: VoiceShortcutMode,
    modifiers: Modifiers,
}

/// The composer's dictation bookkeeping.
#[derive(Default)]
pub struct ComposerSpeech {
    pub press: Option<DictationPress>,
    /// The owner this composer told the indicator it shows.
    pub presented: Option<SharedString>,
    /// The modifiers down now, for a press that ends when its chord's modifier comes up.
    pub modifiers: Modifiers,
}

/// `shouldFinishDictationOnRelease`.
fn finishes_on_release(mode: VoiceShortcutMode, held: Duration) -> bool {
    mode == VoiceShortcutMode::Hold || (mode == VoiceShortcutMode::Auto && held >= HOLD_THRESHOLD)
}

/// The dictation shortcut's label as the composer shows it.
fn dictation_shortcut(cx: &App) -> Option<SharedString> {
    let state = cx.try_global::<ShortcutState>()?;
    shortcut_label_for_command(&state.config, &KeybindingCommand::Static(StaticCommand::ComposerDictation), state.state, state.platform)
}

fn with_shortcut(label: SharedString, shortcut: Option<SharedString>) -> SharedString {
    match shortcut {
        Some(shortcut) => format!("{label} ({shortcut})").into(),
        None => label,
    }
}

impl ChatComposer {
    /// The draft a recording here writes into.
    pub fn voice_owner(&self) -> SharedString {
        self.state.draft_key.0.clone()
    }

    /// `dictationDisabled`: no project, a pending approval, or nothing to dictate into.
    pub(super) fn dictation_disabled(&self) -> bool {
        !self.state.voice_available || self.state.project_required || self.state.pending == ComposerPending::Approval
    }

    /// This draft's dictation state.
    pub fn voice_state(&self, cx: &App) -> VoiceInputState {
        VoiceInput::try_global(cx).map(|voice| voice.read(cx).state_for(&self.voice_owner())).unwrap_or(VoiceInputState::IDLE)
    }

    /// `freezesEditor`: the editor stays read-only until the transcript lands.
    pub(super) fn voice_freezes_editor(&self, cx: &App) -> bool {
        self.voice_state(cx).blocks_submission()
    }

    /// `available`: the mic shows only for a chosen, ready destination.
    pub(super) fn voice_available_here(&self, cx: &App) -> bool {
        VoiceInput::try_global(cx).is_some_and(|voice| voice.read(cx).available(&self.voice_owner(), cx))
    }

    fn voice_target(&self, cx: &mut Context<Self>) -> VoiceTarget {
        let owner = self.voice_owner();
        let weak = cx.weak_entity();
        let (read_weak, commit_weak) = (weak.clone(), weak);
        let read_owner = owner.clone();
        let commit_owner = owner.clone();
        let store = self.draft_store.clone();
        let read_store = store.clone();
        let key = self.state.draft_key.clone();
        let read_key = key.clone();
        // A thread draft's key is `<environment>:<thread>`; anything else is a new draft.
        let (label, route) = match owner.split_once(':') {
            Some((environment, thread)) => {
                let (environment, thread) = (t3code_contracts::EnvironmentId::new(environment), t3code_contracts::ThreadId::new(thread));
                let title = crate::state::StateBridge::environment(&environment, cx)
                    .and_then(|live| live.read(cx).shell.thread(&thread).map(|row| SharedString::from(row.title.to_string())));
                (title, crate::router::Route::Thread(crate::state::shell::thread_key(&environment, &thread)))
            }
            None => (None, crate::router::Route::Draft(crate::router::DraftId(owner.clone()))),
        };
        VoiceTarget {
            owner_key: owner,
            label,
            read_draft: Rc::new(move |cx| {
                // While this composer shows the draft, the live editor is the draft; else the stored one.
                if let Some(composer) = read_weak.upgrade().filter(|c| c.read(cx).voice_owner() == read_owner) {
                    let editor = composer.read(cx).editor().read(cx);
                    return Some((editor.text().to_owned(), editor.selection()));
                }
                let text = read_store.borrow().load(&read_key).map(|draft| draft.prompt).unwrap_or_default();
                let end = text.len();
                Some((text, end..end))
            }),
            commit_draft: Rc::new(move |text, selection, cx| {
                if let Some(composer) = commit_weak.upgrade().filter(|c| c.read(cx).voice_owner() == commit_owner) {
                    composer.update(cx, |composer, cx| composer.commit_transcript(text, selection.start, cx));
                    return;
                }
                let mut draft = store.borrow().load(&key).unwrap_or_default();
                draft.prompt = text;
                store.borrow_mut().save(&key, &draft);
            }),
            open: Some(Rc::new(move |window, cx| crate::speech::voice_input_session::navigate(route.clone(), window, cx))),
            context: VoiceRecordingContext {
                // A thread draft's key is `<environment>:<thread>`; a new draft names none yet.
                environment_id: self.voice_owner().split_once(':').map(|(environment, _)| t3code_contracts::EnvironmentId::new(environment)),
                project_name: None,
                project_words: Vec::new(),
                cleanup: true,
            },
        }
    }

    /// `commitDraft`: the transcript replaces the prompt as one undo step, the caret after it.
    fn commit_transcript(&mut self, text: String, cursor: usize, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            let frozen = std::mem::replace(&mut editor.disabled, false);
            let len = editor.text().len();
            editor.replace_range(0..len, &text, cx);
            editor.set_cursor(cursor, cx);
            editor.disabled = frozen;
        });
        self.schedule_save(cx);
        cx.notify();
    }

    /// Starts dictation into this draft.
    pub fn start_dictation(&mut self, cx: &mut Context<Self>) {
        if self.dictation_disabled() {
            return;
        }
        let target = self.voice_target(cx);
        let voice = VoiceInput::global(cx);
        voice.update(cx, |voice, cx| voice.start(target, cx));
    }

    fn stop_dictation(&mut self, cx: &mut Context<Self>) {
        let owner = self.voice_owner();
        VoiceInput::global(cx).update(cx, |voice, cx| voice.stop(&owner, cx));
    }

    fn cancel_dictation(&mut self, cx: &mut Context<Self>) {
        self.speech.press = None;
        let owner = self.voice_owner();
        VoiceInput::global(cx).update(cx, |voice, cx| voice.cancel(&owner, cx));
    }

    /// `composer.dictation`: starts, or finishes a recording. With no destination ready it
    /// leaves the key alone, returning false so the key falls through.
    pub fn toggle_dictation(&mut self, cx: &mut Context<Self>) -> bool {
        let state = self.voice_state(cx);
        let can_start = matches!(state.phase, VoiceInputPhase::Idle | VoiceInputPhase::Error);
        if can_start && (!self.voice_available_here(cx) || self.dictation_disabled()) {
            return false;
        }
        if !can_start && state.phase != VoiceInputPhase::Recording {
            return true;
        }
        self.speech.press = None;
        if state.phase == VoiceInputPhase::Recording {
            self.stop_dictation(cx);
            return true;
        }
        let mode = ClientSettingsStore::get(cx).voice_shortcut_mode;
        self.speech.press = Some(DictationPress { started: Instant::now(), mode, modifiers: self.speech.modifiers });
        self.start_dictation(cx);
        true
    }

    /// A key came up: a held press finishes the recording.
    pub(super) fn dictation_key_up(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.speech.press.take() else { return };
        if finishes_on_release(press.mode, press.started.elapsed()) {
            let owner = self.voice_owner();
            VoiceInput::global(cx).update(cx, |voice, cx| voice.stop_when_recording(&owner, cx));
        }
    }

    /// Modifiers changed: releasing one the chord used ends the press, since macOS can omit
    /// the letter's key-up while Command is held.
    pub(super) fn dictation_modifiers(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        self.speech.modifiers = modifiers;
        let Some(press) = self.speech.press else { return };
        let released = (press.modifiers.platform && !modifiers.platform)
            || (press.modifiers.control && !modifiers.control)
            || (press.modifiers.alt && !modifiers.alt)
            || (press.modifiers.shift && !modifiers.shift);
        if released {
            self.dictation_key_up(cx);
        }
    }

    /// Escape cancels a press or a running dictation. True when it did.
    pub(super) fn dictation_escape(&mut self, cx: &mut Context<Self>) -> bool {
        let phase = self.voice_state(cx).phase;
        if self.speech.press.is_none() && matches!(phase, VoiceInputPhase::Idle | VoiceInputPhase::Error) {
            return false;
        }
        self.cancel_dictation(cx);
        true
    }

    /// `submitAfterDictation`: a send during dictation finishes the recording and sends with
    /// its transcript. True when the send waits for it.
    pub(super) fn submit_waits_for_dictation(&mut self, intent: crate::composer_logic::ComposerSubmissionIntent, cx: &mut Context<Self>) -> bool {
        if !self.voice_state(cx).blocks_submission() {
            return false;
        }
        let owner = self.voice_owner();
        let weak: WeakEntity<Self> = cx.weak_entity();
        let submit = Box::new(move |cx: &mut App| {
            _ = weak.update(cx, |composer, cx| composer.submit(intent, cx));
        });
        VoiceInput::global(cx).update(cx, |voice, cx| voice.submit_after_dictation(owner, submit, cx));
        true
    }

    /// Tells the indicator which draft this composer shows (`presentVoiceOwner`).
    pub(super) fn sync_voice_presentation(&mut self, cx: &mut Context<Self>) {
        let owner = self.voice_owner();
        if self.speech.presented.as_ref() == Some(&owner) {
            return;
        }
        let voice = VoiceInput::global(cx);
        let previous = self.speech.presented.replace(owner.clone());
        voice.update(cx, |voice, cx| {
            if let Some(previous) = previous {
                voice.unpresent(&previous, cx);
            }
            voice.present(owner, cx);
        });
    }

    /// The dictation controls beside the send button: the capsule or the mic button.
    pub(super) fn render_speech(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.voice_state(cx);
        if !self.voice_available_here(cx) && state.phase == VoiceInputPhase::Idle {
            return None;
        }
        let shortcut = dictation_shortcut(cx);
        let disabled = self.dictation_disabled();
        if state.phase == VoiceInputPhase::Idle {
            return Some(
                div()
                    .id("composer-voice-tip")
                    .flex_none()
                    .tooltip(tooltip(with_shortcut(t!("Start voice input"), shortcut)))
                    .child(
                        Button::new("composer-voice")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::IconSm)
                            .icon(IconName::Mic)
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.dictation_disabled() {
                                    this.start_dictation(cx);
                                }
                            })),
                    )
                    .into_any_element(),
            );
        }
        let owner = self.voice_owner();
        let voice = VoiceInput::try_global(cx);
        let started = voice.as_ref().and_then(|voice| voice.read(cx).recording_started_at(&owner));
        let levels = voice.as_ref().and_then(|voice| voice.read(cx).activity_for(&owner).map(|a| a.levels)).unwrap_or_default();
        let this = cx.weak_entity();
        let actions = PillActions {
            cancel: Rc::new({
                let this = this.clone();
                move |_, cx| _ = this.update(cx, |this, cx| this.cancel_dictation(cx))
            }),
            finish: Rc::new({
                let this = this.clone();
                move |_, cx| _ = this.update(cx, |this, cx| this.stop_dictation(cx))
            }),
            skip: Rc::new({
                let owner = owner.clone();
                move |_, cx| VoiceInput::global(cx).update(cx, |voice, _| voice.skip_post_processing(&owner))
            }),
            retry: Rc::new(move |_, cx| _ = this.update(cx, |this, cx| this.start_dictation(cx))),
        };
        Some(recording_pill(&state, started, &levels, shortcut, disabled, actions, c))
    }

    /// The live transcript above the footer (`aria-label="Live transcription"`).
    pub(super) fn render_live_transcript(&self, c: &ComposerColors, cx: &App) -> Option<AnyElement> {
        let phase = self.voice_state(cx).phase;
        if !matches!(phase, VoiceInputPhase::Recording | VoiceInputPhase::Transcribing) {
            return None;
        }
        let voice = VoiceInput::try_global(cx)?;
        let preview = voice.read(cx).activity_for(&self.voice_owner())?.preview.clone()?;
        Some(
            div()
                .mx(c.r(px(16.)))
                .mb(c.r(px(12.)))
                .max_h(c.r(px(128.)))
                .flex()
                .flex_col()
                .justify_end()
                .overflow_hidden()
                .rounded(c.r(px(8.)))
                .border_1()
                .border_color(c.border.opacity(0.6))
                .bg(c.muted.opacity(0.3))
                .px(c.r(px(12.)))
                .py(c.r(px(8.)))
                .text_size(c.r(px(14.)))
                .line_height(c.r(px(14.)) * 1.625)
                .child(
                    div()
                        .child(gpui_kit::StyledText::new(format!("{}{}", preview.committed, preview.tentative)).with_highlights(
                            (!preview.tentative.is_empty()).then(|| {
                                (preview.committed.len()..preview.committed.len() + preview.tentative.len(), gpui_kit::HighlightStyle {
                                    color: Some(c.muted_foreground),
                                    ..Default::default()
                                })
                            }),
                        )),
                )
                .into_any_element(),
        )
    }

    /// A compact footer gives the capsule the whole row (`dictationTakesFooter`).
    pub(super) fn dictation_takes_footer(&self, compact: bool, cx: &App) -> bool {
        compact && self.voice_state(cx).phase != VoiceInputPhase::Idle
    }

    /// Follows the app's dictation so the capsule and meter redraw.
    pub(super) fn observe_voice(cx: &mut Context<Self>) -> gpui_kit::Subscription {
        let voice: Entity<VoiceInput> = VoiceInput::global(cx);
        cx.observe(&voice, |_, _, cx| cx.notify())
    }

    /// Releases the indicator's presentation when the composer goes away.
    pub(super) fn release_voice_presentation(&mut self, cx: &mut App) {
        if let Some(owner) = self.speech.presented.take()
            && let Some(voice) = VoiceInput::try_global(cx)
        {
            voice.update(cx, |voice, cx| voice.unpresent(&owner, cx));
        }
    }
}

/// What the capsule's buttons do.
pub struct PillActions {
    pub cancel: Rc<dyn Fn(&mut Window, &mut App)>,
    pub finish: Rc<dyn Fn(&mut Window, &mut App)>,
    pub skip: Rc<dyn Fn(&mut Window, &mut App)>,
    /// Record again or retry transcription.
    pub retry: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// `VoiceInputPill`: h-8, p-0.5, a 1px inset ring, rounded-full.
fn pill(error: bool, c: &ComposerColors) -> gpui_kit::Div {
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(c.r(px(6.)))
        .h(c.r(px(32.)))
        .p(c.r(px(2.)))
        .rounded_full()
        .border_1()
        .when(error, |el| el.bg(c.error.opacity(0.06)).text_color(c.error).border_color(c.error.opacity(0.24)))
        .when(!error, |el| el.bg(c.muted.opacity(0.48)).border_color(c.border))
}

/// `ComposerSpeechRecordingPill`: every non-idle phase, the error capsule included.
pub fn recording_pill(
    state: &VoiceInputState,
    started: Option<Instant>,
    levels: &[f32; WAVEFORM_BARS],
    shortcut: Option<SharedString>,
    disabled: bool,
    actions: PillActions,
    c: &ComposerColors,
) -> AnyElement {
    if state.phase == VoiceInputPhase::Error {
        return error_pill(state, disabled, actions, c);
    }
    let recording = state.phase == VoiceInputPhase::Recording;
    let trailing = recording || state.phase == VoiceInputPhase::PostProcessing;
    let cancel_action = actions.cancel.clone();
    let cancel = div().id("composer-voice-cancel-tip").flex_none().tooltip(tooltip(with_shortcut(t!("Cancel voice input"), Some("Esc".into())))).child(
        Button::new("composer-voice-cancel")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSmPill)
            .icon(IconName::X)
            .on_click(move |_, window, cx| cancel_action(window, cx)),
    );
    let status = match state.phase {
        VoiceInputPhase::Preparing => Some(t!("Starting…")),
        VoiceInputPhase::Transcribing => Some(t!("Transcribing…")),
        VoiceInputPhase::PostProcessing => Some(t!("Cleaning up…")),
        _ => None,
    };
    let body = div().flex().flex_1().min_w_0().items_center().gap(c.r(px(8.))).overflow_hidden().when(!trailing, |el| el.pr(c.r(px(10.))));
    let body = if recording {
        let elapsed = started.map_or(0, |at| at.elapsed().as_secs());
        body.child(div().flex_none().size(c.r(px(8.))).rounded_full().bg(c.error))
            .child(
                div()
                    .flex_none()
                    .text_size(c.r(px(12.)))
                    .line_height(c.r(px(16.)))
                    .text_color(c.muted_foreground)
                    .child(format!("{}:{:02}", elapsed / 60, elapsed % 60)),
            )
            .child(div().mr(c.r(px(6.))).flex().w(c.r(px(64.))).min_w_0().flex_shrink(1.).flex_grow(1.).child(waveform(levels, c)))
    } else {
        body.child(Spinner::new(SpinnerSize::Sm).muted()).child(
            div().min_w_0().truncate().text_size(c.r(px(14.))).line_height(c.r(px(20.))).text_color(c.muted_foreground).children(status),
        )
    };
    let trailing_control = match state.phase {
        VoiceInputPhase::PostProcessing => {
            let skip = actions.skip.clone();
            Some(
                Button::new("composer-voice-skip")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::SmPill)
                    .label(t!("Skip", context = "verb, skips cleaning up the transcript"))
                    .on_click(move |_, window, cx| skip(window, cx))
                    .into_any_element(),
            )
        }
        VoiceInputPhase::Recording => {
            let finish = actions.finish.clone();
            Some(
                div()
                    .id("composer-voice-finish-tip")
                    .flex_none()
                    .tooltip(tooltip(with_shortcut(t!("Finish voice input"), shortcut)))
                    .child(
                        Button::new("composer-voice-finish")
                            .variant(ButtonVariant::Default)
                            .size(ButtonSize::IconSmPill)
                            .icon(IconName::Check)
                            .on_click(move |_, window, cx| finish(window, cx)),
                    )
                    .into_any_element(),
            )
        }
        _ => None,
    };
    // `grow-[1.5] basis-0`.
    pill(false, c).flex_grow(1.5).flex_basis(px(0.)).child(cancel).child(body).children(trailing_control).into_any_element()
}

/// `VoiceWaveform`: 24 two-pixel bars, newest at the right, each as tall and as opaque as its
/// level. Heights stand in for the web's `scaleY`.
fn waveform(levels: &[f32; WAVEFORM_BARS], c: &ComposerColors) -> AnyElement {
    let full = c.r(px(16.));
    div()
        .flex()
        .h(full)
        .min_w_0()
        .flex_1()
        .items_center()
        .justify_end()
        .gap(c.r(px(2.)))
        .overflow_hidden()
        .children(levels.iter().map(|&level| {
            div().flex_none().w(c.r(px(2.))).h(full * level.max(WAVEFORM_REST)).rounded_full().bg(c.foreground).opacity(0.3 + level * 0.55)
        }))
        .into_any_element()
}

/// `VoiceInputErrorPill`.
fn error_pill(state: &VoiceInputState, disabled: bool, actions: PillActions, c: &ComposerColors) -> AnyElement {
    let message = state.error.as_ref().map(voice_text).unwrap_or_else(|| t!("Voice input failed"));
    let label = state.error_code.map(error_label).unwrap_or_else(|| t!("Voice input failed"));
    let settings = state.error_action == Some(VoiceErrorAction::Settings);
    let action = if settings {
        t!("Settings")
    } else if state.error_code == Some(VoiceInputErrorCode::TranscriptionFailed) {
        t!("Retry", context = "verb, retries transcription")
    } else {
        t!("Record again")
    };
    let cancel = actions.cancel.clone();
    let dismiss = div().id("composer-voice-dismiss-tip").flex_none().tooltip(tooltip(t!("Dismiss ({shortcut})", shortcut = "Esc"))).child(
        Button::new("composer-voice-dismiss")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSmPill)
            .icon(IconName::X)
            .on_click(move |_, window, cx| cancel(window, cx)),
    );
    let status = div()
        .id("composer-voice-error")
        .flex()
        .min_w_0()
        .items_center()
        .gap(c.r(px(6.)))
        .text_size(c.r(px(14.)))
        .line_height(c.r(px(20.)))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .tooltip(tooltip(message))
        .child(Icon::new(IconName::CircleAlert).size(IconSize::Custom(c.r(px(14.)))).color(c.error))
        .child(div().min_w_0().truncate().child(label));
    let trailing = if state.error_action.is_some() {
        let (cancel, retry) = (actions.cancel.clone(), actions.retry.clone());
        div()
            .ml_auto()
            .flex_none()
            .child(
                Button::new("composer-voice-retry")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::SmPill)
                    .label(action)
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        if settings {
                            cancel(window, cx);
                            crate::speech::voice_input_session::open_voice_settings(window, cx);
                        } else {
                            retry(window, cx);
                        }
                    }),
            )
            .into_any_element()
    } else {
        div().flex_none().w(c.r(px(6.))).into_any_element()
    };
    pill(true, c).child(dismiss).child(status).child(trailing).into_any_element()
}

/// `VOICE_ERROR_LABELS`.
fn error_label(code: VoiceInputErrorCode) -> SharedString {
    match code {
        VoiceInputErrorCode::MicrophoneBlocked => t!("Mic blocked"),
        VoiceInputErrorCode::DraftUnavailable => t!("Draft unavailable"),
        VoiceInputErrorCode::AlreadyRecording => t!("Already recording"),
        VoiceInputErrorCode::Unavailable => t!("Voice input unavailable"),
        VoiceInputErrorCode::StillTranscribing => t!("Still transcribing"),
        VoiceInputErrorCode::LanguageUnsupported => t!("Language unsupported"),
        VoiceInputErrorCode::StartFailed => t!("Couldn’t start"),
        VoiceInputErrorCode::Interrupted => t!("Recording interrupted"),
        VoiceInputErrorCode::FinishFailed => t!("Couldn’t finish"),
        VoiceInputErrorCode::TranscriptionFailed => t!("Transcription failed"),
        VoiceInputErrorCode::DraftChanged => t!("Draft changed"),
        VoiceInputErrorCode::NoSpeech => t!("No speech heard"),
        VoiceInputErrorCode::MicrophoneFailed => t!("Mic failed"),
        VoiceInputErrorCode::MicrophoneDisconnected => t!("Mic disconnected"),
    }
}
