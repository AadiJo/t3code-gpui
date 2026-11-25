//! `VoiceInputSetup.tsx`: the dialog a dictation start opens when the transcription
//! environment has no speech model. Step 0 offers the model setup picked for the person's
//! language and downloads it with progress; step 1 picks a microphone and starts dictating.
//! The state lives on the app's [`VoiceInput`]; this view draws it over the window.

use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, prelude::*, relative,
};
use t3code_contracts::{AuthEnvironmentScope, EnvironmentSpeechModelState, VoiceMicrophoneEntry, VoiceMicrophoneEntryDefault, VoiceMicrophoneEntryDevice};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::scroll_area::scroll_area;
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::wizard::{wizard_footer, wizard_header, wizard_panel, wizard_popup};
use crate::connection::ConnectionRuntime;
use crate::i18n::{t, t_rich};
use crate::speech::language_names::language_label;
use crate::speech::microphones::{self, SystemMicrophone};
use crate::speech::voice_input_session::{self as session, VoiceInput};
use crate::theme_color::with_alpha;

/// The dialog view; one per app.
pub struct VoiceInputSetupDialog {
    microphones: Vec<SystemMicrophone>,
    loading_microphones: bool,
    /// The step the microphone list was last loaded for.
    listed_for_step: Option<u8>,
    _refresh: Option<Task<()>>,
    _observe: Subscription,
}

struct SetupGlobal(Entity<VoiceInputSetupDialog>);
impl Global for SetupGlobal {}

impl VoiceInputSetupDialog {
    /// The layer the window root draws over everything.
    pub fn layer(cx: &mut App) -> AnyElement {
        if let Some(global) = cx.try_global::<SetupGlobal>() {
            return global.0.clone().into_any_element();
        }
        let voice = VoiceInput::global(cx);
        let entity = cx.new(|cx| Self {
            microphones: Vec::new(),
            loading_microphones: false,
            listed_for_step: None,
            _refresh: None,
            _observe: cx.observe(&voice, |this: &mut Self, voice, cx| {
                let setup = voice.read(cx).setup();
                // The microphone step lists devices each time it shows.
                let step = (setup.open && setup.step == 1).then_some(1);
                if step != this.listed_for_step {
                    this.listed_for_step = step;
                    if step.is_some() {
                        this.refresh_microphones(cx);
                    }
                }
                cx.notify();
            }),
        });
        cx.set_global(SetupGlobal(entity.clone()));
        entity.into_any_element()
    }

    /// The view, when one exists (for scripted checks).
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<SetupGlobal>().map(|global| global.0.clone())
    }

    fn refresh_microphones(&mut self, cx: &mut Context<Self>) {
        self.loading_microphones = true;
        let background = cx.background_executor().clone();
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let devices = background.spawn(async { microphones::list_audio_inputs() }).await;
            _ = this.update(cx, |this, cx| {
                this.microphones = devices;
                this.loading_microphones = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The microphones the step lists, by name (for scripted checks).
    pub fn microphone_names(&self) -> Vec<String> {
        self.microphones.iter().map(|device| device.name.clone()).collect()
    }

    /// The microphone the step would record from, and its label.
    fn selected_microphone(&self, cx: &App) -> (Option<String>, SharedString) {
        let order = microphones::current_order(cx);
        match microphones::preferred_microphone(&order, &self.microphones) {
            Some(VoiceMicrophoneEntry::Device(saved)) => {
                let device = self.microphones.iter().find(|device| microphones::entry_matches(saved, device));
                match device {
                    Some(device) => (Some(device.name.clone()), device.name.clone().into()),
                    None => (Some(saved.device_id.clone()), t!("Selected microphone (Unavailable)")),
                }
            }
            _ => (None, t!("System default")),
        }
    }
}

fn update_voice(cx: &mut App, change: impl FnOnce(&mut VoiceInput, &mut Context<VoiceInput>)) {
    VoiceInput::global(cx).update(cx, change);
}

/// Closes the dialog and opens Settings → Voice (`openSettings`). The port's settings page has
/// no anchor jump, so the models row is not scrolled to.
fn open_settings(window: &mut Window, cx: &mut App) {
    update_voice(cx, |voice, cx| voice.set_setup_open(false, cx));
    session::open_voice_settings(window, cx);
}

impl Render for VoiceInputSetupDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(voice) = VoiceInput::try_global(cx) else { return div().into_any_element() };
        let voice = voice.read(cx);
        let setup = voice.setup().clone();
        if !setup.open {
            return div().into_any_element();
        }
        let environment_id = voice.setup_environment().cloned();
        let supported = matches!(voice.status(cx), Some(t3code_contracts::EnvironmentSpeechStatus::Supported(_)));
        let can_retry = voice.can_retry_setup_catalog(cx);
        let can_manage = environment_id.as_ref().and_then(|id| VoiceInput::grants(id, AuthEnvironmentScope::SettingsWrite, cx)) == Some(true);
        let environment_label = environment_id.as_ref().and_then(|id| {
            ConnectionRuntime::is_running(cx)
                .then(|| ConnectionRuntime::state(cx).read(cx).environment(id).map(|env| SharedString::from(env.entry.label.clone())))
                .flatten()
        });
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let muted = c.muted_foreground;
        let xs = |text: SharedString| div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(muted).child(text);

        let (title, description) = if setup.step == 0 {
            (
                t!("Set up voice input"),
                match &environment_label {
                    Some(destination) => {
                        t!("Download a speech model once. Your audio is transcribed on {destination}.", destination = destination.clone())
                    }
                    None => t!("Download a speech model once. Your audio is transcribed on your transcription environment."),
                },
            )
        } else {
            (t!("Voice input is ready"), t!("Pick a microphone, then start talking."))
        };

        let body: AnyElement = if setup.step == 0 {
            let mut section = div().flex().flex_col().gap(theme.sp(3.)).ui_text(&theme, TextSize::Sm, weight::NORMAL);
            if let Some(model) = &setup.model {
                let progress = model.downloaded.map(|downloaded| (downloaded / model.size * 100.).round());
                let note: SharedString = match (&setup.language, model.recommended) {
                    (Some(language), true) => SharedString::from(
                        t_rich!(
                            "Recommended for {languageName}. Includes a 1.2 MB silence detector.",
                            languageName = language_label(language).to_string()
                        )
                        .plain(),
                    ),
                    _ => SharedString::from(t_rich!("Includes a 1.2 MB silence detector.").plain()),
                };
                let mut card = div()
                    .id("voice-setup-model")
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .rounded(theme.radii.lg)
                    .border_1()
                    .border_color(with_alpha(c.border, 0.7))
                    .bg(with_alpha(c.card(), 0.6))
                    .p(theme.sp(3.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .justify_between()
                            .gap(theme.sp(3.))
                            .child(div().min_w_0().truncate().ui_text(&theme, TextSize::Sm, weight::MEDIUM).child(model.name.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                                    .text_color(muted)
                                    .child(t!("{size} MB", size = (model.size / 1024. / 1024.).round() as i64)),
                            ),
                    )
                    .child(xs(note));
                if setup.downloading {
                    let label = if model.state == EnvironmentSpeechModelState::Verifying {
                        t!("Verifying…")
                    } else {
                        match progress {
                            None => t!("Starting download…"),
                            Some(progress) => t!("{progress}% downloaded", progress = progress as i64),
                        }
                    };
                    let fraction = progress.unwrap_or(0.).clamp(0., 100.) as f32 / 100.;
                    card = card.child(
                        div()
                            .id("voice-setup-progress")
                            .flex()
                            .flex_col()
                            .gap(theme.sp(1.5))
                            .child(
                                div()
                                    .h(theme.sp(1.))
                                    .overflow_hidden()
                                    .rounded_full()
                                    .bg(c.muted)
                                    .child(div().h_full().w(relative(fraction)).bg(c.primary())),
                            )
                            .child(xs(label))
                            .test_support(),
                    );
                }
                section = section.child(card.test_support());
            } else if setup.error.is_none() {
                section = section.child(div().text_color(muted).child(t_rich!("Loading transcription models…").plain()));
            }
            if let Some(error) = &setup.error {
                section = section.child(
                    div()
                        .id("voice-setup-error")
                        .flex()
                        .items_start()
                        .gap(theme.sp(1.5))
                        .text_color(c.destructive())
                        .child(
                            div()
                                .mt(theme.sp(0.5))
                                .flex_none()
                                .child(Icon::new(IconName::CircleAlert).size(IconSize::Custom(theme.sp(3.5))).color(c.destructive())),
                        )
                        .child(div().min_w_0().child(error.clone()))
                        .test_support(),
                );
            }
            section.into_any_element()
        } else {
            let (selected, selected_label) = self.selected_microphone(cx);
            let unavailable = selected.as_ref().is_some_and(|id| !self.microphones.iter().any(|device| &device.name == id));
            let mut options: Vec<SharedString> = vec![t!("System default")];
            let mut values: Vec<Option<String>> = vec![None];
            if unavailable {
                options.push(t!("Selected microphone (Unavailable)"));
                values.push(selected.clone());
            }
            for device in &self.microphones {
                options.push(device.name.clone().into());
                values.push(Some(device.name.clone()));
            }
            let current = values.iter().position(|value| *value == selected).unwrap_or(0);
            let select = Select::new("voice-setup-microphone-select", options, current, move |ix, _, cx| {
                let Some(value) = values.get(ix).cloned() else { return };
                let entry = match value {
                    Some(name) if !microphones::is_system_default_device_id(&name) => {
                        VoiceMicrophoneEntry::Device(VoiceMicrophoneEntryDevice { device_id: name.clone(), label: name })
                    }
                    _ => VoiceMicrophoneEntry::Default(VoiceMicrophoneEntryDefault {}),
                };
                let next = microphones::prefer_microphone(&microphones::current_order(cx), entry);
                microphones::save_order(next, cx);
            })
            .size(SelectSize::Sm)
            .value_label(selected_label)
            .leading(Icon::new(IconName::Mic).size(IconSize::Custom(theme.sp(3.5))).color(muted));
            let refresh = Button::new("voice-setup-refresh-microphones")
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .icon(IconName::RefreshCw)
                .icon_size(IconSize::Custom(theme.sp(3.5)))
                .disabled(self.loading_microphones)
                .on_click(cx.listener(|this, _, _, cx| this.refresh_microphones(cx)));
            let refresh = div().id("voice-setup-refresh").child(refresh).test_support();
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(2.))
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .child(div().flex().items_center().gap(theme.sp(2.)).child(div().id("voice-setup-microphone").flex_1().min_w_0().child(select).test_support()).child(refresh))
                .when(unavailable, |d| {
                    d.child(
                        div()
                            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                            .text_color(c.destructive())
                            .child(t_rich!("That microphone is unplugged. Pick another one.").plain()),
                    )
                })
                .into_any_element()
        };

        let footer = if setup.step == 0 {
            let leading = Button::new("voice-setup-models")
                .variant(ButtonVariant::Ghost)
                .label(if setup.model.is_some() { t!("Other models") } else { t!("Choose a model") })
                .on_click(|_, window, cx| open_settings(window, cx))
                .into_any_element();
            let mut buttons: Vec<AnyElement> = Vec::new();
            if can_retry {
                buttons.push(
                    Button::new("voice-setup-retry")
                        .label(t_rich!("Retry loading models").plain())
                        .on_click(|_, _, cx| update_voice(cx, |voice, cx| voice.retry_setup_catalog(cx)))
                        .into_any_element(),
                );
            }
            if setup.downloading {
                buttons.push(
                    Button::new("voice-setup-cancel-download")
                        .variant(ButtonVariant::Outline)
                        .label(t_rich!("Cancel download").plain())
                        .disabled(!can_manage)
                        .on_click(|_, _, cx| update_voice(cx, |voice, cx| voice.cancel_setup_download(cx)))
                        .into_any_element(),
                );
            }
            if setup.model.is_some() {
                let label = if setup.downloading {
                    t!("Downloading…")
                } else if setup.error.is_some() {
                    t!("Try again")
                } else if can_manage {
                    t!("Download model")
                } else {
                    t!("Needs an admin to download")
                };
                buttons.push(
                    Button::new("voice-setup-download")
                        .label(label)
                        .disabled(!can_manage || setup.downloading || !supported)
                        .on_click(|_, _, cx| update_voice(cx, |voice, cx| voice.download_setup_model(cx)))
                        .into_any_element(),
                );
            }
            wizard_footer(&theme, Some(leading), buttons)
        } else {
            let leading = Button::new("voice-setup-settings")
                .variant(ButtonVariant::Ghost)
                .label(t_rich!("Voice settings").plain())
                .on_click(|_, window, cx| open_settings(window, cx))
                .into_any_element();
            let start = Button::new("voice-setup-start")
                .icon(IconName::Mic)
                .label(t_rich!("Start dictating").plain())
                .on_click(|_, _, cx| update_voice(cx, |voice, cx| voice.start_setup_recording(cx)))
                .into_any_element();
            wizard_footer(&theme, Some(leading), [start])
        };

        let close = |_: &mut Window, cx: &mut App| update_voice(cx, |voice, cx| voice.set_setup_open(false, cx));
        let content = wizard_popup(&theme, false)
            .id("voice-input-setup")
            .max_h(theme.rem(48.))
            .child(
                scroll_area("voice-input-setup-scroll")
                    .flex()
                    .flex_col()
                    .child(wizard_header(&theme, title, Some(description)))
                    .child(wizard_panel(&theme, false, [body]))
                    .child(footer),
            )
            .child(
                div().absolute().top(theme.sp(2.)).right(theme.sp(2.)).child(
                    Button::new("voice-input-setup-close").variant(ButtonVariant::Ghost).size(ButtonSize::Icon).icon(IconName::X).on_click(
                        move |_, window, cx| close(window, cx),
                    ),
                ),
            )
            .test_support();
        Modal::dialog("voice-input-setup-modal", content, close).into_any_element()
    }
}
