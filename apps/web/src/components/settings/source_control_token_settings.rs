//! Ports `SourceControlTokenSettings.tsx` with the parts of
//! `SecretSourceInput.tsx` it uses: a host's personal access token, typed in
//! or read from a configured secret provider, saved per hostname under
//! `sourceControlHosts.<kind>.tokens`.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use serde_json::Value;
use t3code_contracts::ClientServerSettingsPatch;

use super::server_settings_controls::save_patch;
use super::server_settings_data::ServerPanelsModel;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::i18n::t;

/// What a host with token credentials needs (`SourceControlHostCredentials`).
pub struct TokenCredentials {
    pub kind: &'static str,
    pub label: &'static str,
    pub public_host: &'static str,
    pub cli_name: &'static str,
    pub cli_label: &'static str,
    pub token_env_var: &'static str,
    pub new_token_url: fn(&str) -> String,
}

/// GitHub's credentials, the only built-in host that has them.
pub const GITHUB_CREDENTIALS: TokenCredentials = TokenCredentials {
    kind: "github",
    label: "GitHub",
    public_host: "github.com",
    cli_name: "gh",
    cli_label: "GitHub CLI",
    token_env_var: "GH_TOKEN",
    new_token_url: |host| format!("https://{host}/settings/personal-access-tokens/new"),
};

/// `hasToken`: a non-empty string (or redaction marker) or a secret reference.
fn has_token(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

fn is_reference(value: &Value) -> bool {
    value.get("provider").is_some() && value.get("reference").is_some()
}

/// The token form of one host.
pub struct SourceControlTokenSettings {
    model: Entity<ServerPanelsModel>,
    credentials: &'static TokenCredentials,
    host: Entity<InputState>,
    token: Entity<InputState>,
    reference: Entity<InputState>,
    /// The secret provider picked in the source menu; `None` is a typed value.
    provider: Option<String>,
    /// The token field's placeholder as last set.
    token_placeholder: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl SourceControlTokenSettings {
    pub fn new(model: Entity<ServerPanelsModel>, credentials: &'static TokenCredentials, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = cx.new(|cx| InputState::new(window, cx).placeholder(credentials.public_host).default_value(credentials.public_host));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let reference = cx.new(|cx| InputState::new(window, cx));
        let notify = |this: &mut Self, _: &Entity<InputState>, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        };
        let subscriptions =
            vec![cx.subscribe_in(&host, window, notify), cx.subscribe_in(&token, window, notify), cx.subscribe_in(&reference, window, notify)];
        Self { model, credentials, host, token, reference, provider: None, token_placeholder: SharedString::default(), _subscriptions: subscriptions }
    }

    fn tokens(&self, cx: &App) -> serde_json::Map<String, Value> {
        let settings = self.model.read(cx).snapshot().settings.clone();
        settings
            .source_control_hosts
            .as_ref()
            .and_then(|hosts| hosts.get(self.credentials.kind))
            .and_then(|host| host.get("tokens"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    }

    /// Secret providers a token can point at (`useSecretSources`).
    fn sources(&self, cx: &App) -> Vec<(String, SharedString)> {
        let settings = self.model.read(cx).snapshot().settings.clone();
        settings.secret_providers.iter().flatten().map(|(id, provider)| (id.clone(), SharedString::from(provider.label.clone()))).collect()
    }

    fn normalized_host(&self, cx: &App) -> String {
        self.host.read(cx).value().trim().to_lowercase()
    }

    /// The token to save: the typed value, or the reference to the picked provider.
    fn submission(&self, cx: &App) -> Option<Value> {
        match &self.provider {
            None => Some(self.token.read(cx).value().trim().to_owned()).filter(|t| !t.is_empty()).map(Value::String),
            Some(provider) => {
                let reference = self.reference.read(cx).value().trim().to_owned();
                (!reference.is_empty()).then(|| serde_json::json!({ "provider": provider, "reference": reference }))
            }
        }
    }

    fn saving(&self, cx: &App) -> bool {
        self.model.read(cx).is_pending(&self.save_key())
    }

    fn save_key(&self) -> String {
        format!("token-{}", self.credentials.kind)
    }

    fn save(&mut self, host: String, token: Value, window: &mut Window, cx: &mut Context<Self>) {
        // Sends the whole map; other tokens' redaction markers keep their values.
        let mut tokens = self.tokens(cx);
        tokens.insert(host, token);
        let mut entry = serde_json::Map::new();
        entry.insert("tokens".into(), Value::Object(tokens));
        let patch = ClientServerSettingsPatch {
            source_control_hosts: Some([(self.credentials.kind.to_owned(), entry)].into_iter().collect()),
            ..Default::default()
        };
        save_patch(&self.model, self.save_key(), patch, cx);
        self.token.update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.normalized_host(cx);
        if let Some(token) = self.submission(cx).filter(|_| !host.is_empty() && !self.saving(cx)) {
            self.save(host, token, window, cx);
        }
    }
}

impl Render for SourceControlTokenSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let creds = self.credentials;
        let tokens = self.tokens(cx);
        let host = self.normalized_host(cx);
        let saved = tokens.get(&host).filter(|v| has_token(v)).cloned();
        let saved_hosts: Vec<String> = tokens.iter().filter(|(_, v)| has_token(v)).map(|(k, _)| k.clone()).collect();
        let saving = self.saving(cx);
        let sources = self.sources(cx);
        let entity = cx.entity();
        let code = |text: &str| {
            div().rounded(theme.radii.sm).bg(c.muted).px(sp(1.)).mono_text(px(TextSize::Xs2.size(theme.fonts.interface)), weight::NORMAL).child(text.to_owned())
        };
        let token_access = t!("read and write access to pull requests and contents");
        let link_host = if host.is_empty() { creds.public_host.to_owned() } else { host.clone() };
        let url = (creds.new_token_url)(&link_host);
        let intro = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(sp(1.))
            .max_w(theme.rem(42.))
            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
            .text_color(c.muted_foreground)
            .child(t!("A token saved here is used before"))
            .child(code(creds.token_env_var))
            .child(t!("and the"))
            .child(code(creds.cli_name))
            .child(t!("login, so {label} works without the {cli}. Give it {access}.", label = creds.label, cli = creds.cli_label, access = token_access))
            .child(
                div()
                    .id("sc-token-create")
                    .flex()
                    .items_center()
                    .gap(sp(1.))
                    .cursor_pointer()
                    .text_color(c.foreground())
                    .hover(|s| s.underline())
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .child(t!("Create a token"))
                    .child(Icon::new(IconName::ExternalLink).size(IconSize::Custom(sp(3.)))),
            );
        let label = |text: SharedString| div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(text);
        let configured = saved.as_ref().is_some_and(|v| !is_reference(v));
        let token_placeholder = if configured { t!("Stored secret, enter a new value to replace") } else { t!("Not set") };
        if self.token_placeholder != token_placeholder {
            self.token_placeholder = token_placeholder.clone();
            self.token.update(cx, |input, cx| input.set_placeholder(token_placeholder, window, cx));
        }
        let source_menu = (!sources.is_empty() || self.provider.is_some()).then(|| {
            let current = self.provider.clone();
            let icon = if current.is_some() { IconName::KeyRound } else { IconName::Type };
            let trigger = Button::new("sc-token-source").variant(ButtonVariant::Outline).size(ButtonSize::Sm).icon(icon).trailing_icon(IconName::ChevronDown);
            let entity = entity.clone();
            let sources = sources.clone();
            dropdown_menu("sc-token-source-menu", trigger, move |_, _| {
                let pick = |provider: Option<String>| {
                    let entity = entity.clone();
                    move |_: &mut Window, cx: &mut App| {
                        entity.update(cx, |this, cx| {
                            this.provider = provider.clone();
                            cx.notify();
                        })
                    }
                };
                let mut menu = MenuPopup::new().item(
                    MenuItem::new(t!("Value", context = "secret source: a value typed in directly")).icon(IconName::Type).kind(MenuItemKind::Radio(current.is_none())).on_select(pick(None)),
                );
                for (id, label) in &sources {
                    menu = menu.item(MenuItem::new(label.clone()).icon(IconName::KeyRound).kind(MenuItemKind::Radio(current.as_deref() == Some(id))).on_select(pick(Some(id.clone()))));
                }
                menu
            })
        });
        let value_field = match self.provider {
            None => div().id("sc-token-value").child(TextInput::new(&self.token).size(InputSize::Sm).disabled(saving)).test_support().into_any_element(),
            Some(_) => TextInput::new(&self.reference).size(InputSize::Sm).mono().disabled(saving).into_any_element(),
        };
        let fields = div()
            .flex()
            .gap(sp(3.))
            .child(div().flex().flex_col().gap(sp(1.5)).w(theme.rem(12.)).flex_none().child(label(t!("Host"))).child(TextInput::new(&self.host).size(InputSize::Sm).disabled(saving)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(sp(1.5))
                    .child(label(t!("Token")))
                    .child(div().flex().min_w_0().items_center().gap(sp(1.5)).children(source_menu).child(div().flex_1().min_w_0().child(value_field))),
            );
        let status: SharedString = if saved_hosts.is_empty() {
            t!("No token saved; the server uses {envVar} or the {cliName} login.", envVar = creds.token_env_var, cliName = creds.cli_name)
        } else {
            t!("Saved for {hosts}.", hosts = saved_hosts.join(", "))
        };
        let can_save = !host.is_empty() && self.submission(cx).is_some() && !saving;
        let remove_entity = entity.clone();
        let save_entity = entity.clone();
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(3.))
            .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(status))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(sp(2.))
                    .when(saved.is_some(), |d| {
                        d.child(Button::new("sc-token-remove").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Remove")).disabled(saving).on_click(move |_, window, cx| {
                            remove_entity.update(cx, |this, cx| {
                                let host = this.normalized_host(cx);
                                this.save(host, Value::String(String::new()), window, cx);
                            })
                        }))
                    })
                    .child(Button::new("sc-token-save").size(ButtonSize::Xs).label(t!("Save")).disabled(!can_save).on_click(move |_, window, cx| save_entity.update(cx, |this, cx| this.submit(window, cx)))),
            );
        div().flex().flex_col().gap(sp(3.)).child(intro).child(fields).child(footer)
    }
}
