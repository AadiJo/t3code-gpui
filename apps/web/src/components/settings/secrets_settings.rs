//! Settings → Secrets (hidden route `/settings/secrets`). Ports
//! `SecretsSettings.tsx` and `SecretProviderDialog.tsx`: the configured
//! secret providers and "Add a 1Password provider" (Connect server or
//! service account). Saved with `server.updateSettings` (`secretProviders`).

use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use t3code_contracts::{ClientServerSettingsPatch, ClientServerSettingsPatchSecretProvidersValue, SecretProviderKind};

use super::server_settings_controls::{field_input, save_patch};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::i18n::t;

/// The add-provider form.
struct AddForm {
    name: Entity<InputState>,
    account: Entity<InputState>,
    token: Entity<InputState>,
}

/// The Secrets page.
pub struct SecretsSettingsPanel {
    model: Entity<ServerPanelsModel>,
    form: Option<AddForm>,
    _observe: Subscription,
}

impl SecretsSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, form: None, _observe: observe }
    }

    /// Opens "Add a 1Password provider".
    pub fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = |placeholder: SharedString, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).masked(masked))
        };
        self.form = Some(AddForm {
            name: input(t!("Personal"), false, window, cx),
            account: input("my.1password.com".into(), false, window, cx),
            token: input(t!("Service account token"), true, window, cx),
        });
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        let label = form.name.read(cx).value().trim().to_owned();
        let account = form.account.read(cx).value().trim().to_owned();
        let token = form.token.read(cx).value().trim().to_owned();
        if label.is_empty() {
            return;
        }
        let id = format!("onepassword-{}", label.to_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
        let value = ClientServerSettingsPatchSecretProvidersValue {
            kind: SecretProviderKind::new("onepassword"),
            label,
            config: Some(serde_json::json!({ "account": account, "token": token })),
        };
        let patch = ClientServerSettingsPatch { secret_providers: Some([(id, Some(value))].into_iter().collect()), ..Default::default() };
        save_patch(&self.model, "secretProviders", patch, cx);
        self.form = None;
        cx.notify();
    }
}

impl Render for SecretsSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let providers = snapshot.settings.secret_providers.clone().unwrap_or_default();
        let mut section = SettingsSection::new(t!("Secret providers")).header_action(
            Button::new("secret-provider-add")
                .variant(ButtonVariant::GhostMuted)
                .size(ButtonSize::Xs)
                .label(t!("Add secret provider"))
                .disabled(snapshot.read_only)
                .on_click(cx.listener(|this, _, window, cx| this.open_add(window, cx))),
        );
        if providers.is_empty() {
            section.extend([SettingsRow::new(t!("No secret providers configured.")).description(t!("Add a 1Password account, then pick it as the source of a provider variable.")).into_any_element()]);
        }
        for (id, provider) in &providers {
            let model = self.model.clone();
            let id = id.clone();
            section.extend([SettingsRow::new(SharedString::from(provider.label.clone()))
                .description(SharedString::from(provider.kind.clone()))
                .control(Button::new(SharedString::from(format!("secret-remove-{id}"))).variant(ButtonVariant::GhostDestructive).size(ButtonSize::Xs).label(t!("Remove")).disabled(snapshot.read_only).on_click(move |_, _, cx| {
                    let patch = ClientServerSettingsPatch { secret_providers: Some([(id.clone(), None)].into_iter().collect()), ..Default::default() };
                    save_patch(&model, "secretProviders", patch, cx);
                }))
                .into_any_element()]);
        }
        let dialog = self.form.as_ref().map(|form| {
            let label = |text: SharedString| div().ui_text(theme, TextSize::Xs, weight::MEDIUM).child(text);
            let surface = DialogSurface::new(t!("Add a 1Password provider"))
                .child(div().flex().flex_col().gap(theme.sp(1.5)).child(label(t!("Name"))).child(field_input(&form.name, false, false, false)))
                .child(div().flex().flex_col().gap(theme.sp(1.5)).child(label(t!("Account"))).child(field_input(&form.account, true, false, false)))
                .child(div().flex().flex_col().gap(theme.sp(1.5)).child(label(t!("Service account token"))).child(field_input(&form.token, true, false, false)))
                .footer(Button::new("secret-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                    this.form = None;
                    cx.notify();
                })))
                .footer(Button::new("secret-save").label(t!("Add provider")).on_click(cx.listener(|this, _, _, cx| this.save(cx))));
            let this = cx.entity().downgrade();
            Modal::dialog("secret-provider-dialog", surface, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.form = None;
                    cx.notify();
                })
                .ok();
            })
        });
        div().relative().size_full().child(page_container(cx).child(section)).children(dialog)
    }
}
