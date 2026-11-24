//! The Account row of a provider's Setup section. Ports
//! `ProviderAuthenticationSection.tsx`: every sign-in flow the server drives
//! (a browser page, with or without consent; a device code to type in a
//! browser; a localhost callback URL pasted back; credential fields; and a
//! login running in a terminal), with progress, cancel, retry and sign-out.
//!
//! The terminal flow embeds a real terminal (`provider_auth_terminal`).

use std::collections::BTreeMap;

use gpui_kit::{AnyElement, ClipboardItem, Context, IntoElement, SharedString, Window, div, prelude::*, px};
use t3code_contracts::{
    ProviderAuthCancelInput, ProviderAuthCompleteInput, ProviderAuthInteraction, ProviderAuthRespondInput,
    ProviderAuthResponse, ProviderAuthResponseBrowser, ProviderAuthResponseBrowserAction, ProviderAuthResponseCredentials,
    ProviderAuthStartInput, ProviderAuthState, ProviderAuthStatePhase, ProviderInstanceId,
    ProviderSetupInput, ServerProviderAuthStatus,
};

use super::provider_settings_panel::ProviderSettingsPanel;
use super::server_settings_controls::{field_input, tinted};
use super::server_settings_data::{Reply, RpcFailure, ServerSettingsSource};
use super::settings_layout::SettingsRow;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::select::{Select, SelectSize};
use crate::i18n::t;
use crate::provider_instances::ServerProvider;
use crate::theme_color::with_alpha;

/// Per-instance sign-in UI state the panel keeps between renders.
#[derive(Clone, Debug, Default)]
pub struct AuthUi {
    /// The method picked in the select; empty is "Provider default".
    pub method_id: String,
    /// The last command's failure, shown under the row.
    pub error: Option<SharedString>,
    /// Asking before signing out.
    pub confirm_logout: bool,
}

/// What the row is doing, derived from the provider snapshot and the auth state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStage {
    Active,
    SignedIn,
    Discovering,
    NeedsExternalSetup,
    Idle,
}

/// `isWebUrl`: only http and https links are opened.
fn is_web_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// The stage and the account description for one provider.
pub fn auth_stage(provider: &ServerProvider, auth: Option<&ProviderAuthState>) -> (AuthStage, SharedString) {
    let phase = auth.map(|a| a.phase);
    let active = matches!(phase, Some(ProviderAuthStatePhase::Starting | ProviderAuthStatePhase::Waiting | ProviderAuthStatePhase::Verifying));
    let signed_in = provider.auth.status == ServerProviderAuthStatus::Authenticated
        || (provider.auth.status == ServerProviderAuthStatus::Unknown && phase == Some(ProviderAuthStatePhase::Succeeded));
    let discovering = provider.driver == "acpRegistry" && !active && !signed_in && auth.is_none_or(|a| a.methods.is_none());
    let needs_external = !active
        && !signed_in
        && (provider.setup.as_ref().is_some_and(|s| !s.can_authenticate)
            || (provider.driver == "acpRegistry" && auth.and_then(|a| a.methods.as_ref()).is_some_and(Vec::is_empty)));
    let uses_credentials = provider.driver == "antigravity" && matches!(provider.auth.r#type.as_deref(), Some("gemini-api-key" | "agent-platform"));
    let interaction = auth.and_then(|a| a.interaction.as_ref());
    if active {
        let text = match phase {
            Some(ProviderAuthStatePhase::Starting) => auth
                .and_then(|a| a.message.clone())
                .map(SharedString::from)
                .unwrap_or_else(|| if uses_credentials { t!("Connecting…") } else { t!("Starting sign-in…") }),
            Some(ProviderAuthStatePhase::Verifying) => {
                if uses_credentials { t!("Checking credentials and loading models…") } else { t!("Checking your account…") }
            }
            _ => match interaction {
                Some(ProviderAuthInteraction::Terminal(_)) => t!("Complete sign-in in the terminal below."),
                Some(ProviderAuthInteraction::Credentials(_)) => t!("Enter your credentials below."),
                _ => t!("Finish signing in in your browser."),
            },
        };
        return (AuthStage::Active, text);
    }
    if signed_in {
        let text = match provider.auth.email.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            Some(email) => t!("Signed in as {email}", email = email),
            None => t!("Signed in."),
        };
        return (AuthStage::SignedIn, text);
    }
    if discovering {
        return (AuthStage::Discovering, t!("Discovering sign-in methods…"));
    }
    if needs_external {
        return (AuthStage::NeedsExternalSetup, t!("No in-app sign-in advertised. Follow the provider's docs to finish setup."));
    }
    let environment = SharedString::default();
    let _ = environment;
    (AuthStage::Idle, SharedString::default())
}

impl ProviderSettingsPanel {
    fn auth_ui(&mut self, instance: &str) -> &mut AuthUi {
        self.auth_ui.entry(instance.to_owned()).or_default()
    }

    /// Runs one sign-in command and records its failure on the row.
    pub(super) fn run_auth(
        &mut self,
        instance: String,
        call: impl FnOnce(&dyn ServerSettingsSource) -> Reply<ProviderAuthState> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.auth_ui(&instance).error = None;
        let this = cx.entity().downgrade();
        let key = format!("auth:{instance}");
        self.model.update(cx, |model, cx| {
            model.run(key, call, move |result: Result<ProviderAuthState, RpcFailure>, cx| {
                if let Err(error) = result {
                    this.update(cx, |this, cx| {
                        this.auth_ui(&instance).error = Some(error.message);
                        cx.notify();
                    })
                    .ok();
                }
            }, cx)
        });
        cx.notify();
    }

    /// Opens the sign-in page, after recording consent when the flow asks for it.
    fn open_sign_in_page(&mut self, instance: String, url: String, consent: Option<(String, String)>, cx: &mut Context<Self>) {
        if !is_web_url(&url) {
            self.auth_ui(&instance).error = Some(t!("This sign-in link is not a web address, so it was not opened."));
            cx.notify();
            return;
        }
        if let Some((flow_id, interaction_id)) = consent {
            let input = ProviderAuthRespondInput {
                instance_id: ProviderInstanceId::new(instance.as_str()),
                flow_id,
                interaction_id,
                response: ProviderAuthResponse::Browser(ProviderAuthResponseBrowser { action: ProviderAuthResponseBrowserAction::Accept }),
            };
            self.run_auth(instance, move |s| s.respond_provider_auth(input), cx);
        }
        cx.open_url(&url);
    }

    /// The Account row for `provider`.
    pub(super) fn render_auth_section(&mut self, provider: std::sync::Arc<ServerProvider>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let instance = provider.instance_id.clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let auth = snapshot.provider_auth.get(&instance).cloned();
        let pending = self.model.read(cx).is_pending(&format!("auth:{instance}"));
        let ui = self.auth_ui(&instance).clone();
        let (stage, mut description) = auth_stage(&provider, auth.as_ref());
        let uses_credentials =
            provider.driver == "antigravity" && matches!(provider.auth.r#type.as_deref(), Some("gemini-api-key" | "agent-platform"));
        if stage == AuthStage::Idle {
            description = if uses_credentials {
                t!("Connect on {environment} to load models.", environment = snapshot.environment_label.clone())
            } else {
                t!("Sign in on {environment}.", environment = snapshot.environment_label.clone())
            };
        }
        let interaction = auth.as_ref().and_then(|a| a.interaction.clone());
        let flow_id = auth.as_ref().and_then(|a| a.flow_id.clone());
        let disabled = snapshot.read_only || pending || stage == AuthStage::Discovering;
        let url = match &interaction {
            Some(ProviderAuthInteraction::Browser(b)) => Some(b.url.clone()),
            Some(ProviderAuthInteraction::DeviceCode(d)) => Some(d.url.clone()),
            _ => auth.as_ref().and_then(|a| a.authorization_url.clone()),
        };
        let phase = auth.as_ref().map(|a| a.phase);
        let methods = auth.as_ref().and_then(|a| a.methods.clone()).unwrap_or_default();

        let mut controls = div().flex().flex_wrap().items_center().gap(theme.sp(1.5));
        if stage != AuthStage::Active && methods.len() > 1 {
            let mut options = vec![t!("Provider default")];
            options.extend(methods.iter().map(|m| SharedString::from(m.name.clone())));
            let selected = methods.iter().position(|m| m.id == ui.method_id).map_or(0, |i| i + 1);
            let ids: Vec<String> = methods.iter().map(|m| m.id.clone()).collect();
            let this = cx.entity().downgrade();
            let id = instance.clone();
            controls = controls.child(div().w(theme.sp(44.)).child(
                Select::new(SharedString::from(format!("auth-method-{instance}")), options, selected, move |ix, _, cx| {
                    let method = if ix == 0 { String::new() } else { ids.get(ix - 1).cloned().unwrap_or_default() };
                    this.update(cx, |this, cx| {
                        this.auth_ui(&id).method_id = method;
                        cx.notify();
                    })
                    .ok();
                })
                .size(SelectSize::Sm),
            ));
        }
        if let Some(url) = url.clone() {
            let consent = match &interaction {
                Some(ProviderAuthInteraction::Browser(b)) if b.requires_consent => flow_id.clone().map(|f| (f, b.id.clone())),
                _ => None,
            };
            let (open_url, copy_url) = (url.clone(), url.clone());
            let id = instance.clone();
            let consent_for_copy = consent.clone();
            let id_for_copy = instance.clone();
            controls = controls
                .child(
                    Button::new(SharedString::from(format!("auth-open-{instance}")))
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .label(t!("Open browser"))
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| this.open_sign_in_page(id.clone(), open_url.clone(), consent.clone(), cx))),
                )
                .child(
                    Button::new(SharedString::from(format!("auth-copy-{instance}")))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::Copy)
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()));
                            if let Some((flow_id, interaction_id)) = consent_for_copy.clone() {
                                let input = ProviderAuthRespondInput {
                                    instance_id: ProviderInstanceId::new(id_for_copy.as_str()),
                                    flow_id,
                                    interaction_id,
                                    response: ProviderAuthResponse::Browser(ProviderAuthResponseBrowser {
                                        action: ProviderAuthResponseBrowserAction::Accept,
                                    }),
                                };
                                this.run_auth(id_for_copy.clone(), move |s| s.respond_provider_auth(input), cx);
                            }
                        })),
                );
        }
        let can_authenticate = provider.setup.as_ref().is_none_or(|s| s.can_authenticate);
        match stage {
            AuthStage::NeedsExternalSetup => {
                if let Some(docs) = provider.setup.as_ref().and_then(|s| s.documentation_url.clone()) {
                    controls = controls.child(
                        Button::new(SharedString::from(format!("auth-docs-{instance}")))
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .label(t!("Open docs"))
                            .on_click(move |_, _, cx| cx.open_url(&docs)),
                    );
                }
            }
            AuthStage::Active if flow_id.is_some() => {
                let id = instance.clone();
                let flow = flow_id.clone().unwrap_or_default();
                controls = controls.child(
                    Button::new(SharedString::from(format!("auth-cancel-{instance}")))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Sm)
                        .label(t!("Cancel"))
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let input = ProviderAuthCancelInput { instance_id: ProviderInstanceId::new(id.as_str()), flow_id: flow.clone() };
                            this.run_auth(id.clone(), move |s| s.cancel_provider_auth(input), cx);
                        })),
                );
            }
            AuthStage::Active => {}
            _ if can_authenticate => {
                let label = if stage == AuthStage::SignedIn {
                    t!("Change account")
                } else if matches!(phase, Some(ProviderAuthStatePhase::Failed | ProviderAuthStatePhase::Cancelled)) {
                    if uses_credentials { t!("Retry connection") } else { t!("Retry sign-in") }
                } else if uses_credentials {
                    t!("Connect")
                } else {
                    t!("Sign in")
                };
                let id = instance.clone();
                let method = (!ui.method_id.is_empty() && methods.iter().any(|m| m.id == ui.method_id)).then(|| ui.method_id.clone());
                controls = controls.child(
                    Button::new(SharedString::from(format!("auth-start-{instance}")))
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .label(label)
                        .disabled(disabled || !provider.enabled || !provider.installed || auth.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let input = ProviderAuthStartInput {
                                instance_id: ProviderInstanceId::new(id.as_str()),
                                method_id: method.clone(),
                                return_url: None,
                                callback_mode: None,
                            };
                            this.run_auth(id.clone(), move |s| s.start_provider_auth(input), cx);
                        })),
                );
            }
            _ => {}
        }
        if stage == AuthStage::SignedIn && provider.auth.can_logout.unwrap_or(can_authenticate) {
            let id = instance.clone();
            controls = controls.child(
                Button::new(SharedString::from(format!("auth-logout-{instance}")))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Sm)
                    .label(t!("Sign out"))
                    .disabled(disabled || auth.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.auth_ui(&id).confirm_logout = true;
                        cx.notify();
                    })),
            );
        }

        let mut details: Vec<AnyElement> = Vec::new();
        if ui.confirm_logout {
            let provider_name = provider.display_name.clone().unwrap_or_else(|| provider.driver.clone());
            let (yes, no) = (instance.clone(), instance.clone());
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .py(theme.sp(2.))
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(t!(
                        "Sign out of {provider} on {environment}? This stops running threads that share this sign-in. Thread history is kept.",
                        provider = provider_name,
                        environment = snapshot.environment_label.clone()
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(theme.sp(2.))
                            .child(Button::new(SharedString::from(format!("auth-logout-no-{instance}"))).variant(ButtonVariant::Ghost).size(ButtonSize::Sm).label(t!("Cancel")).on_click(cx.listener(move |this, _, _, cx| {
                                this.auth_ui(&no).confirm_logout = false;
                                cx.notify();
                            })))
                            .child(Button::new(SharedString::from(format!("auth-logout-yes-{instance}"))).variant(ButtonVariant::Destructive).size(ButtonSize::Sm).label(t!("Sign out")).on_click(cx.listener(move |this, _, _, cx| {
                                this.auth_ui(&yes).confirm_logout = false;
                                let input = ProviderSetupInput { instance_id: ProviderInstanceId::new(yes.as_str()) };
                                this.run_auth(yes.clone(), move |s| s.logout_provider_auth(input), cx);
                            }))),
                    )
                    .into_any_element(),
            );
        }
        match &interaction {
            Some(ProviderAuthInteraction::DeviceCode(code)) => {
                let copy = code.user_code.clone();
                details.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.sp(2.))
                        .py(theme.sp(2.))
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(t!("Enter code"))
                        .child(
                            div()
                                .px(theme.sp(1.5))
                                .rounded(theme.radii.sm)
                                .bg(with_alpha(c.muted, 0.6))
                                .mono_text(px(13.), weight::MEDIUM)
                                .text_color(c.foreground())
                                .child(code.user_code.clone()),
                        )
                        .child(
                            Button::new(SharedString::from(format!("auth-copy-code-{instance}")))
                                .variant(ButtonVariant::GhostMuted)
                                .size(ButtonSize::IconXs)
                                .icon(IconName::Copy)
                                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
                        )
                        .child(t!("in your browser."))
                        .into_any_element(),
                );
            }
            Some(ProviderAuthInteraction::Terminal(terminal)) => {
                let surface = self.auth_terminal(&instance, flow_id.as_deref().unwrap_or_default(), terminal, window, cx);
                // `h-64 overflow-hidden rounded-md border`.
                details.push(
                    div()
                        .py(theme.sp(2.))
                        .child(
                            div()
                                .relative()
                                .h(theme.sp(64.))
                                .overflow_hidden()
                                .rounded(theme.radii.md)
                                .border_1()
                                .border_color(c.border)
                                .child(surface),
                        )
                        .into_any_element(),
                );
            }
            Some(ProviderAuthInteraction::Credentials(form)) => {
                let mut body = div().flex().flex_col().gap(theme.sp(2.)).py(theme.sp(2.));
                let mut names = Vec::new();
                for field in &form.fields {
                    let input = self.fields.get(format!("{instance}:auth:cred:{}", field.name), "", "", field.secret, window, cx);
                    names.push((field.name.clone(), input.clone()));
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(1.))
                            .ui_text(theme, TextSize::Sm, weight::NORMAL)
                            .child(field.label.clone())
                            .child(div().w(theme.sp(72.)).child(field_input(&input, false, false, disabled))),
                    );
                }
                let id = instance.clone();
                let flow = flow_id.clone().unwrap_or_default();
                let interaction_id = form.id.clone();
                body = body.child(
                    div().flex().child(
                        Button::new(SharedString::from(format!("auth-credentials-{instance}")))
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .label(t!("Connect"))
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let values: BTreeMap<String, String> =
                                    names.iter().map(|(name, field)| (name.clone(), field.read(cx).value().to_string())).collect();
                                this.fields.reset_prefix(&format!("{id}:auth:cred:"));
                                let input = ProviderAuthRespondInput {
                                    instance_id: ProviderInstanceId::new(id.as_str()),
                                    flow_id: flow.clone(),
                                    interaction_id: interaction_id.clone(),
                                    response: ProviderAuthResponse::Credentials(ProviderAuthResponseCredentials { values }),
                                };
                                this.run_auth(id.clone(), move |s| s.respond_provider_auth(input), cx);
                            })),
                    ),
                );
                details.push(body.into_any_element());
            }
            _ => {}
        }
        let accepts_callback = url.is_some()
            && match &interaction {
                Some(ProviderAuthInteraction::Browser(b)) => b.accepts_callback == Some(true),
                None => true,
                _ => false,
            };
        if accepts_callback {
            let field = self.fields.get(format!("{instance}:auth:callback"), "", "http://localhost:1455/auth/callback?code=…", false, window, cx);
            let filled = !field.read(cx).value().trim().is_empty();
            let id = instance.clone();
            let flow = flow_id.clone().unwrap_or_default();
            let read = field.clone();
            details.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .py(theme.sp(2.))
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(t!("If the final localhost page does not load, paste its full URL here."))
                    .child(div().w_full().child(field_input(&field, true, false, disabled)))
                    .child(
                        div().flex().child(
                            Button::new(SharedString::from(format!("auth-callback-{instance}")))
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Sm)
                                .label(t!("Continue"))
                                .disabled(disabled || !filled)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let callback_url = read.read(cx).value().to_string();
                                    if flow.is_empty() || callback_url.trim().is_empty() {
                                        return;
                                    }
                                    this.fields.set(&format!("{id}:auth:callback"), "", window, cx);
                                    let input = ProviderAuthCompleteInput {
                                        instance_id: ProviderInstanceId::new(id.as_str()),
                                        flow_id: flow.clone(),
                                        callback_url,
                                    };
                                    this.run_auth(id.clone(), move |s| s.complete_provider_auth(input), cx);
                                })),
                        ),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = &ui.error {
            details.push(div().py(theme.sp(2.)).child(tinted(error.clone(), c.destructive(), cx)).into_any_element());
        }
        let status = (phase == Some(ProviderAuthStatePhase::Failed)).then(|| auth.as_ref().and_then(|a| a.message.clone())).flatten();
        let mut row = SettingsRow::new(t!("Account")).description(description).control(controls);
        if let Some(status) = status {
            row = row.status(SharedString::from(status));
        }
        row.extend(details);
        row.into_any_element()
    }
}
