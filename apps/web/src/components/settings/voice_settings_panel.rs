//! Voice. Ports `VoiceSettingsPanel.tsx` as it renders while transcription
//! is Off: the microphone order (`MicrophoneOrderList`), the microphone
//! test, the dictation shortcut, the transcription destination, silence
//! filtering and filler words, both dictionaries (`DictionaryEditor`), and
//! the cleanup switch.
//!
//! This app has no audio capture and no `/api/voice` client yet, so the
//! microphone list shows the saved order without device enumeration, the
//! Record button and the cleanup switch are disabled, and "Transcribe with"
//! offers Off only. The rows the web adds once a destination is chosen
//! (language, translate, the model browser, advanced speech settings,
//! cleanup instructions and the correction cue) stay hidden, as they do on
//! the web while transcription is Off.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_contracts::{ClientServerSettingsPatch, SpeechCustomWords, SpeechCustomWordsInnerItem, VoiceMicrophoneEntry, VoiceShortcutMode};

use super::settings_controls::{SelectWidth, SettingsScope, client_select, client_switch, inheritance_marker, sized_select};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::TextInput;
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::switch::Switch;
use crate::components::ui::tooltip::tooltip;
use crate::hooks::use_settings::{ClientSettingsStore, ServerSettingsStore};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// The dictionary a draft adds to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dictionary {
    Device,
    Shared,
}

/// The Voice page and its two dictionary drafts.
pub struct VoiceSettings {
    device_draft: Entity<InputState>,
    shared_draft: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl VoiceSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("Add words, separated by commas")))
        };
        let device_draft = draft(window, cx);
        let shared_draft = draft(window, cx);
        let subscriptions = [(device_draft.clone(), Dictionary::Device), (shared_draft.clone(), Dictionary::Shared)]
            .into_iter()
            .map(|(state, dictionary)| {
                cx.subscribe_in(&state, window, move |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.add_terms(dictionary, window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                })
            })
            .collect();
        Self { device_draft, shared_draft, _subscriptions: subscriptions }
    }

    fn draft(&self, dictionary: Dictionary) -> &Entity<InputState> {
        match dictionary {
            Dictionary::Device => &self.device_draft,
            Dictionary::Shared => &self.shared_draft,
        }
    }

    fn words(dictionary: Dictionary, cx: &App) -> SpeechCustomWords {
        match dictionary {
            Dictionary::Device => ClientSettingsStore::get(cx).speech_custom_words.clone(),
            Dictionary::Shared => {
                SettingsScope::settings(cx).and_then(|s| s.speech_project_custom_words).unwrap_or_default()
            }
        }
    }

    fn save(dictionary: Dictionary, words: SpeechCustomWords, cx: &mut App) {
        match dictionary {
            Dictionary::Device => ClientSettingsStore::update(cx, |s| s.speech_custom_words = words),
            Dictionary::Shared => SettingsScope::update(
                ClientServerSettingsPatch { speech_project_custom_words: Some(words), ..Default::default() },
                cx,
            ),
        }
    }

    /// The draft's new terms: split on commas and newlines, trimmed, without
    /// ones already in the dictionary (case-insensitively). `None` when the
    /// draft cannot be added (`canAddTerm`).
    fn new_terms(&self, dictionary: Dictionary, cx: &App) -> Option<Vec<String>> {
        let words = Self::words(dictionary, cx);
        let mut seen: std::collections::HashSet<String> = words
            .iter()
            .flat_map(|w| std::iter::once(&w.term).chain(w.aliases.iter()))
            .map(|s| s.to_lowercase())
            .collect();
        let draft = self.draft(dictionary).read(cx).value();
        let terms: Vec<String> = draft
            .split([',', '\n'])
            .map(|term| term.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|term| !term.is_empty() && seen.insert(term.to_lowercase()))
            .collect();
        let fits = !terms.is_empty() && terms.iter().all(|t| t.chars().count() <= 50) && words.len() + terms.len() <= 100;
        fits.then_some(terms)
    }

    fn add_terms(&mut self, dictionary: Dictionary, window: &mut Window, cx: &mut Context<Self>) {
        let Some(terms) = self.new_terms(dictionary, cx) else { return };
        let mut words = Self::words(dictionary, cx);
        words.extend(terms.into_iter().map(|term| SpeechCustomWordsInnerItem { term, aliases: Vec::new() }));
        Self::save(dictionary, words, cx);
        self.draft(dictionary).update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    /// `DictionaryEditor`: a one-line field with an outline Add button, then
    /// the entries as chips with a remove button.
    fn dictionary_editor(&self, dictionary: Dictionary, disabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let words = Self::words(dictionary, cx);
        let can_add = !disabled && self.new_terms(dictionary, cx).is_some();
        let slug = if dictionary == Dictionary::Device { "device" } else { "shared" };
        let add = cx.listener(move |this, _, window, cx| this.add_terms(dictionary, window, cx));
        let entries = (!words.is_empty()).then(|| {
            div().mt(theme.sp(2.5)).flex().flex_wrap().gap(theme.sp(1.5)).children(words.iter().enumerate().map(|(ix, word)| {
                let term = word.term.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.))
                    .rounded(theme.radii.md)
                    .border_1()
                    .border_color(with_alpha(c.border, 0.6))
                    .bg(with_alpha(c.muted, 0.3))
                    .py(theme.sp(0.5))
                    .pl(theme.sp(2.))
                    .pr(theme.sp(0.5))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .child(div().font_weight(weight::MEDIUM).child(SharedString::from(word.term.clone())))
                    .when(!word.aliases.is_empty(), |d| {
                        d.child(div().text_color(c.muted_foreground).child(format!("← {}", word.aliases.join(", "))))
                    })
                    .child(
                        Button::new(SharedString::from(format!("dictionary-{slug}-remove-{ix}")))
                            .variant(ButtonVariant::GhostMuted)
                            .size(ButtonSize::IconXs)
                            .icon(IconName::X)
                            .disabled(disabled)
                            .on_click(move |_, _, cx| {
                                let words = Self::words(dictionary, cx).into_iter().filter(|w| w.term != term).collect();
                                Self::save(dictionary, words, cx);
                            }),
                    )
            }))
        });
        div()
            .mt(theme.sp(3.))
            .pb(theme.sp(2.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(theme.sp(2.))
                    .child(div().flex_1().min_w_0().child(TextInput::new(self.draft(dictionary)).disabled(disabled)))
                    .child(
                        Button::new(SharedString::from(format!("dictionary-{slug}-add")))
                            .variant(ButtonVariant::Outline)
                            .icon(IconName::Plus)
                            .label(t!("Add"))
                            .disabled(!can_add)
                            .on_click(add),
                    ),
            )
            .children(entries)
            .into_any_element()
    }
}

/// `MicrophoneOrderList`: the saved order, preferred first, in a bordered
/// list. The first entry is the one in use, since this app cannot list the
/// connected devices.
fn microphone_list(cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let saved = ClientSettingsStore::get(cx).voice_microphones.clone();
    let count = saved.len().max(1);
    let names: Vec<(SharedString, bool)> = if saved.is_empty() {
        vec![(t!("System default"), false)]
    } else {
        saved
            .iter()
            .map(|entry| match entry {
                VoiceMicrophoneEntry::Device(device) if !device.label.is_empty() => (device.label.clone().into(), true),
                VoiceMicrophoneEntry::Device(_) => (t!("Saved microphone"), true),
                _ => (t!("System default"), false),
            })
            .collect()
    };
    let divider = with_alpha(c.border, 0.6);
    let move_entry = |from: usize, to: usize| {
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            ClientSettingsStore::update(cx, |s| {
                if from < s.voice_microphones.len() && to < s.voice_microphones.len() {
                    s.voice_microphones.swap(from, to);
                }
            })
        }
    };
    div()
        .mt(theme.sp(3.))
        .pb(theme.sp(2.))
        .child(
            div().flex().flex_col().rounded(theme.radii.lg).border_1().border_color(divider).children(
                names.into_iter().enumerate().map(|(ix, (name, forgettable))| {
                    let arrow = |id: &str, icon: IconName, enabled: bool, target: usize| {
                        Button::new(SharedString::from(format!("microphone-{id}-{ix}")))
                            .variant(ButtonVariant::GhostMuted)
                            .size(ButtonSize::IconXs)
                            .icon(icon)
                            .disabled(!enabled)
                            .on_click(move_entry(ix, target))
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap_x(theme.sp(2.))
                        .py(theme.sp(1.5))
                        .pl(theme.sp(3.))
                        .pr(theme.sp(1.5))
                        .when(ix > 0, |d| d.border_t_1().border_color(divider))
                        .child(
                            div()
                                .w(theme.rem(1.))
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(c.muted_foreground)
                                .child(format!("{}", ix + 1)),
                        )
                        .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(name.clone()))
                        .child(div().flex_1().flex().when(ix == 0, |d| {
                            d.child(
                                div()
                                    .rounded(theme.radii.sm)
                                    .bg(with_alpha(theme.fixed.success, 0.12))
                                    .px(theme.sp(1.))
                                    .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                    .text_color(theme.fixed.success_foreground)
                                    .child(t!("In use")),
                            )
                        }))
                        .child(
                            div()
                                .size(theme.sp(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::GripVertical).size(IconSize::Sm).color(with_alpha(c.muted_foreground, 0.7))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.sp(0.5))
                                .child(arrow("up", IconName::ArrowUp, ix > 0, ix.saturating_sub(1)))
                                .child(arrow("down", IconName::ArrowDown, ix + 1 < count, ix + 1))
                                .child(if forgettable {
                                    Button::new(SharedString::from(format!("microphone-forget-{ix}")))
                                        .variant(ButtonVariant::GhostMuted)
                                        .size(ButtonSize::IconXs)
                                        .icon(IconName::X)
                                        .on_click(move |_, _, cx| {
                                            ClientSettingsStore::update(cx, |s| {
                                                if ix < s.voice_microphones.len() {
                                                    s.voice_microphones.remove(ix);
                                                }
                                            })
                                        })
                                        .into_any_element()
                                } else {
                                    div().size(theme.sp(6.)).into_any_element()
                                }),
                        )
                }),
            ),
        )
        .into_any_element()
}

impl Render for VoiceSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = ClientSettingsStore::get(cx).clone();
        let theme = cx.app_theme();
        let hint = match settings.voice_shortcut_mode {
            VoiceShortcutMode::Auto => t!("Tap to toggle, hold to talk"),
            VoiceShortcutMode::Hold => t!("Record while held"),
            VoiceShortcutMode::Toggle => t!("Tap to start, tap to stop"),
        };
        let refresh = div().id("voice-refresh-tip").tooltip(tooltip(t!("Refresh microphones"))).child(
            Button::new("voice-refresh-microphones").variant(ButtonVariant::Outline).size(ButtonSize::IconSm).icon(IconName::RefreshCw),
        );
        let input = SettingsSection::new(t!("Input"))
            .child(
                SettingsRow::new(t!("Microphone"))
                    .description(t!("Voice input uses the first connected microphone in this list."))
                    .control(refresh)
                    .child(microphone_list(cx)),
            )
            .child(
                SettingsRow::new(t!("Test microphone"))
                    .description(t!("Record ten seconds and play it back. The audio stays on this device."))
                    .control(
                        Button::new("voice-test-record")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .icon(IconName::Mic)
                            .label(t!("Record"))
                            .disabled(true),
                    ),
            )
            .child(
                client_select(
                    "dictation-shortcut-mode",
                    format!("{hint}."),
                    SelectWidth::Rem(10.),
                    vec![
                        (VoiceShortcutMode::Auto, t!("Auto", context = "dictation shortcut mode")),
                        (VoiceShortcutMode::Hold, t!("Hold", context = "dictation shortcut mode: record while the key is held")),
                        (VoiceShortcutMode::Toggle, t!("Toggle", context = "dictation shortcut mode: tap to start and stop")),
                    ],
                    |s| &s.voice_shortcut_mode,
                    |s, v| s.voice_shortcut_mode = v,
                    cx,
                )
                .title(t!("Dictation shortcut")),
            );

        let off_icon = Icon::new(IconName::MicOff).size(IconSize::Md).color(theme.colors.icon_muted);
        let destination = Select::new("select-voice-destination", [t!("Off", context = "voice transcription destination")], 0, |_, _, _| {})
            .size(SelectSize::Sm)
            .leading(off_icon);
        let transcription = SettingsSection::new(t!("Transcription"))
            .child(
                SettingsRow::new(t!("Transcribe with"))
                    .description(t!("Where your speech is turned into text."))
                    .control(div().max_w(theme.rem(18.)).child(sized_select(SelectWidth::Rem(10.), destination))),
            )
            .child(
                client_switch(
                    "speech-silence-filtering",
                    t!("Skip silence and background noise before transcribing."),
                    |s| &s.speech_silence_filtering,
                    |s, v| s.speech_silence_filtering = v,
                    cx,
                )
                .title(t!("Silence filtering")),
            )
            .child(
                client_switch(
                    "remove-filler-words",
                    t!("Drop “um” and “uh” from transcripts."),
                    |s| &s.speech_remove_filler_words,
                    |s, v| s.speech_remove_filler_words = v,
                    cx,
                )
                .title(t!("Remove filler words")),
            );

        let server = SettingsScope::settings(cx);
        let shared_words = server.as_ref().and_then(|s| s.speech_project_custom_words.clone());
        let shared_default = ServerSettingsStore::defaults().speech_project_custom_words.clone();
        let shared_summary =
            if shared_words.unwrap_or_default() == shared_default.unwrap_or_default() { t!("Built-in default") } else { t!("Set on the environment") };
        let mut shared_row = SettingsRow::new(setting_title("speech-project-dictionary"))
            .description(t!("Used by everyone who dictates on this environment."))
            .inheritance(inheritance_marker("speech-project-dictionary", shared_summary), true);
        if server.is_none() {
            shared_row = shared_row.status(t!("Reconnect the selected environment to change this setting."));
        }
        let dictionary = SettingsSection::new(t!("Dictionary"))
            .child(
                SettingsRow::new(t!("Your dictionary"))
                    .description(t!("Names and terms to spell your way, saved on this device."))
                    .child(self.dictionary_editor(Dictionary::Device, false, cx)),
            )
            .child(shared_row.child(self.dictionary_editor(Dictionary::Shared, server.is_none(), cx)));

        let cleanup_on = server.as_ref().and_then(|s| s.speech_post_processing_model_selection.as_ref()).is_some();
        let cleanup = SettingsSection::new(t!("Cleanup")).child(
            SettingsRow::new(t!("Clean up transcripts"))
                .description(t!("Off by default. Sends the transcript and nearby draft text, never audio, to the cleanup model."))
                .control(Switch::new("switch-speech-cleanup", cleanup_on).disabled(true)),
        );
        page_container(cx).child(input).child(transcription).child(dictionary).child(cleanup)
    }
}
