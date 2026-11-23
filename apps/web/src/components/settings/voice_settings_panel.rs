//! Voice. Ports the rows of `VoiceSettingsPanel.tsx` that are saved
//! settings: dictation shortcut mode, language, translate, silence
//! filtering, filler words, streaming, hardware and model unloading (on the
//! environment), the dictionaries, cleanup instructions and the correction
//! cue. Microphone capture, the transcription destination and model list,
//! cloud keys and the microphone and model tests need audio capture and the
//! `/api/voice` endpoints, which this app does not have yet; the microphone
//! row lists the saved order.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AnyElement, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_contracts::{
    ClientServerSettingsPatch, SpeechAcceleration, SpeechAccelerationKnown, SpeechModelUnloadTimeout,
    SpeechPostProcessingPromptSettingsMode, VoiceMicrophoneEntry, VoiceShortcutMode,
};

use super::settings_controls::{SettingsScope, client_row, client_select, client_switch, select_control};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::settings_panels::advanced_settings;
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::input::TextInput;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;

/// The Voice page.
pub struct VoiceSettings {
    correction: Entity<InputState>,
    advanced_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl VoiceSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let value = ClientSettingsStore::get(cx).speech_correction_word.as_str().to_owned();
        let correction = cx.new(|cx| InputState::new(window, cx).placeholder("sorry").default_value(value));
        let subscription = cx.subscribe(&correction, |_, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value: String = state.read(cx).value().chars().take(50).collect();
                ClientSettingsStore::update(cx, |s| s.speech_correction_word = value.as_str().into());
            }
        });
        Self { correction, advanced_open: false, _subscriptions: vec![subscription] }
    }
}

impl Render for VoiceSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = ClientSettingsStore::get(cx).clone();
        let theme = cx.app_theme();
        let muted = theme.colors.muted_foreground;
        let mics: Vec<SharedString> = settings
            .voice_microphones
            .iter()
            .map(|entry| match entry {
                VoiceMicrophoneEntry::Default(_) => t!("System default"),
                VoiceMicrophoneEntry::Device(device) => device.label.clone().into(),
                VoiceMicrophoneEntry::Unknown(_) => "?".into(),
            })
            .collect();
        let mic_list = div()
            .flex()
            .flex_col()
            .gap(theme.sp(1.))
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(muted)
            .children(if mics.is_empty() { vec![t!("System default")] } else { mics });
        let hint = match settings.voice_shortcut_mode {
            VoiceShortcutMode::Auto => t!("Tap to toggle, hold to talk"),
            VoiceShortcutMode::Hold => t!("Record while held"),
            VoiceShortcutMode::Toggle => t!("Tap to start, tap to stop"),
        };
        let input = SettingsSection::new(t!("Input"))
            .child(
                SettingsRow::new(setting_title("voice-microphone"))
                    .description(t!("Voice input uses the first connected microphone in this list."))
                    .child(mic_list),
            )
            .child(client_select(
                "dictation-shortcut-mode",
                format!("{hint}."),
                vec![
                    (VoiceShortcutMode::Auto, t!("Auto", context = "dictation shortcut mode")),
                    (VoiceShortcutMode::Hold, t!("Hold", context = "dictation shortcut mode: record while the key is held")),
                    (VoiceShortcutMode::Toggle, t!("Toggle", context = "dictation shortcut mode: tap to start and stop")),
                ],
                |s| &s.voice_shortcut_mode,
                |s, v| s.voice_shortcut_mode = v,
                cx,
            ));

        let mut languages = vec![(t3code_contracts::SpeechLanguage::from("auto"), t!("Auto", context = "dictation shortcut mode"))];
        if settings.speech_language.as_str() != "auto" {
            languages.push((settings.speech_language.clone(), SharedString::from(settings.speech_language.as_str().to_owned())));
        }
        let server = SettingsScope::settings(cx);
        let acceleration_auto = server.as_ref().and_then(|s| s.speech_acceleration.clone()).is_none_or(|a| a == SpeechAcceleration::Known(SpeechAccelerationKnown::Auto));
        let unload = server.as_ref().and_then(|s| s.speech_model_unload_timeout.clone()).unwrap_or(SpeechModelUnloadTimeout::Min15);
        let unload_options = [
            (SpeechModelUnloadTimeout::Never, t!("Never", context = "unload the speech model")),
            (SpeechModelUnloadTimeout::Immediately, t!("Immediately")),
            (SpeechModelUnloadTimeout::Min2, t!("After 2 minutes")),
            (SpeechModelUnloadTimeout::Min5, t!("After 5 minutes")),
            (SpeechModelUnloadTimeout::Min10, t!("After 10 minutes")),
            (SpeechModelUnloadTimeout::Min15, t!("After 15 minutes")),
            (SpeechModelUnloadTimeout::Hour1, t!("After 1 hour")),
        ];
        let unload_index = unload_options.iter().position(|(v, _)| *v == unload).unwrap_or(5);
        let unload_values: Vec<SpeechModelUnloadTimeout> = unload_options.iter().map(|(v, _)| v.clone()).collect();
        let hardware = div()
            .flex()
            .gap(theme.sp(2.))
            .child(select_control(
                "select-speech-acceleration".into(),
                Rc::from([SharedString::new_static("Auto"), SharedString::new_static("CPU")]),
                usize::from(!acceleration_auto),
                |index, _, cx| {
                    let value = if index == 0 { SpeechAccelerationKnown::Auto } else { SpeechAccelerationKnown::Cpu };
                    SettingsScope::update(ClientServerSettingsPatch { speech_acceleration: Some(SpeechAcceleration::Known(value)), ..Default::default() }, cx)
                },
            ))
            .child(select_control(
                "select-speech-unload".into(),
                unload_options.iter().map(|(_, label)| label.clone()).collect::<Vec<_>>().into(),
                unload_index,
                move |index, _, cx| {
                    SettingsScope::update(ClientServerSettingsPatch { speech_model_unload_timeout: Some(unload_values[index].clone()), ..Default::default() }, cx)
                },
            ));
        let this = cx.entity();
        let toggle: Rc<dyn Fn(&mut Window, &mut gpui_kit::App)> = Rc::new(move |_, cx| {
            this.update(cx, |this, cx| {
                this.advanced_open = !this.advanced_open;
                cx.notify();
            })
        });
        let transcription = SettingsSection::new(t!("Transcription"))
            .child(client_select("speech-language", t!("The provider detects your language automatically."), languages, |s| &s.speech_language, |s, v| s.speech_language = v, cx).title(t!("Language")))
            .child(
                client_switch(
                    "speech-language",
                    t!("Speak any language and get English text."),
                    |s| &s.speech_translate_to_english,
                    |s, v| s.speech_translate_to_english = v,
                    cx,
                )
                .title(t!("Translate to English")),
            )
            .child(client_switch(
                "speech-silence-filtering",
                t!("Skip silence and background noise before transcribing."),
                |s| &s.speech_silence_filtering,
                |s, v| s.speech_silence_filtering = v,
                cx,
            ))
            .child(client_switch("remove-filler-words", t!("Drop “um” and “uh” from transcripts."), |s| &s.speech_remove_filler_words, |s, v| s.speech_remove_filler_words = v, cx))
            .child(advanced_settings(
                "voice-advanced",
                self.advanced_open,
                toggle,
                vec![
                    client_switch("speech-streaming", t!("This model doesn’t support native streaming."), |s| &s.speech_streaming, |s, v| s.speech_streaming = v, cx).into_any_element(),
                    SettingsRow::new(setting_title("speech-hardware"))
                        .description(t!("Auto uses the environment’s GPU when it has one."))
                        .disabled(server.is_none())
                        .control(hardware)
                        .into_any_element(),
                ],
            ));

        let word_list = |words: &[t3code_contracts::SpeechCustomWordsInnerItem]| -> AnyElement {
            div()
                .flex()
                .flex_wrap()
                .gap(theme.sp(1.5))
                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                .children(words.iter().map(|word| {
                    div().px(theme.sp(2.)).py(theme.sp(0.5)).rounded(theme.radii.md).bg(theme.colors.secondary).child(word.term.clone())
                }))
                .into_any_element()
        };
        let project_words = server.as_ref().and_then(|s| s.speech_project_custom_words.clone()).unwrap_or_default();
        let dictionary = SettingsSection::new(t!("Dictionary"))
            .child(
                SettingsRow::new(t!("Your dictionary"))
                    .description(t!("Names and terms to spell your way, saved on this device."))
                    .child(word_list(&settings.speech_custom_words)),
            )
            .child(
                SettingsRow::new(setting_title("speech-project-dictionary"))
                    .description(t!("Used by everyone who dictates on this environment."))
                    .disabled(server.is_none())
                    .child(word_list(&project_words)),
            );

        let custom = settings.speech_post_processing_prompt.mode == SpeechPostProcessingPromptSettingsMode::Custom;
        let cleanup_on = server.as_ref().and_then(|s| s.speech_post_processing_model_selection.as_ref()).is_some();
        let cleanup = SettingsSection::new(t!("Cleanup"))
            .child(
                SettingsRow::new(t!("Clean up transcripts"))
                    .description(t!("Off by default. Sends the transcript and nearby draft text, never audio, to the cleanup model."))
                    .control(crate::components::ui::switch::Switch::new("switch-speech-cleanup", cleanup_on).disabled(true)),
            )
            .child(client_row(
                "speech-post-processing",
                if custom { t!("Your own instructions for cleanup.") } else { t!("Fix punctuation, numbers, lists, and spoken corrections.") },
                |s| &s.speech_post_processing_prompt,
                |s, v| s.speech_post_processing_prompt = v,
                select_control("select-cleanup-instructions".into(), Rc::from([t!("Built-in"), t!("Custom")]), usize::from(custom), |index, _, cx| {
                    ClientSettingsStore::update(cx, |s| {
                        s.speech_post_processing_prompt.mode =
                            if index == 1 { SpeechPostProcessingPromptSettingsMode::Custom } else { SpeechPostProcessingPromptSettingsMode::Default }
                    })
                }),
                cx,
            ).title(t!("Instructions")))
            .child(client_row(
                "speech-correction-word",
                t!("The phrase you say to fix a mistake, like “scratch that”."),
                |s| &s.speech_correction_word,
                |s, v| s.speech_correction_word = v,
                div().w(gpui_kit::rems(10.)).child(TextInput::new(&self.correction)),
                cx,
            ));
        page_container(cx).child(input).child(transcription).child(dictionary).child(cleanup)
    }
}
