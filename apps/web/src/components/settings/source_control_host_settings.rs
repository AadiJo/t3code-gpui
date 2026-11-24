//! Ports `SourceControlHostSettings.tsx` with the `settings` variant of
//! `ProviderSettingsForm.tsx` for the hosts whose definition has a settings
//! schema but no token credentials (Bitbucket, GitCafe). Each field saves to
//! `sourceControlHosts.<kind>.<field>` when it is committed (Enter or blur).

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*};
use serde_json::Value;
use t3code_contracts::ClientServerSettingsPatch;

use super::server_settings_controls::save_patch;
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::SettingsRow;
use crate::ActiveAppTheme as _;
use crate::components::icons::IconName;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::i18n::t;

/// One schema field (`makeProviderSettingsSchema` key with its annotations).
pub struct HostField {
    pub key: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub placeholder: Option<&'static str>,
    /// `providerSettingsForm.control: "password"`.
    pub secret: bool,
}

/// A host with a settings form.
pub struct HostSettingsDefinition {
    pub kind: &'static str,
    pub label: &'static str,
    pub fields: &'static [HostField],
}

/// Bitbucket's settings schema, in its `order`.
pub const BITBUCKET_SETTINGS: HostSettingsDefinition = HostSettingsDefinition {
    kind: "bitbucket",
    label: "Bitbucket",
    fields: &[
        HostField {
            key: "accessToken",
            title: "Access token",
            description: "Scoped to one repository, project, or workspace. Create it in that item's Bitbucket settings.",
            placeholder: None,
            secret: true,
        },
        HostField {
            key: "email",
            title: "Atlassian account email",
            description: "With an API token instead of an access token.",
            placeholder: Some("you@example.com"),
            secret: false,
        },
        HostField {
            key: "apiToken",
            title: "API token",
            description: "Reaches every repository your Atlassian account can. Give it read and write access to repositories and pull requests, plus read:user:bitbucket and read:workspace:bitbucket.",
            placeholder: None,
            secret: true,
        },
    ],
};

/// GitCafe's settings schema.
pub const GITCAFE_SETTINGS: HostSettingsDefinition = HostSettingsDefinition {
    kind: "gitcafe",
    label: "GitCafe",
    fields: &[HostField {
        key: "token",
        title: "Token",
        description: "A git.cafe token, used before CAFE_TOKEN and the cafe login, so GitCafe works without the CLI signed in.",
        placeholder: None,
        secret: true,
    }],
};

/// The form of one host.
pub struct SourceControlHostSettings {
    model: Entity<ServerPanelsModel>,
    definition: &'static HostSettingsDefinition,
    inputs: Vec<Entity<InputState>>,
    /// Each secret field's placeholder as last set.
    placeholders: Vec<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl SourceControlHostSettings {
    pub fn new(model: Entity<ServerPanelsModel>, definition: &'static HostSettingsDefinition, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = Self::saved_of(&model, definition, cx);
        let mut subscriptions = Vec::new();
        let inputs: Vec<Entity<InputState>> = definition
            .fields
            .iter()
            .enumerate()
            .map(|(ix, field)| {
                let initial = if field.secret { String::new() } else { saved.get(field.key).and_then(Value::as_str).unwrap_or_default().to_owned() };
                let input = cx.new(|cx| {
                    let state = InputState::new(window, cx).masked(field.secret).default_value(initial);
                    match field.placeholder {
                        Some(placeholder) => state.placeholder(placeholder),
                        None => state,
                    }
                });
                subscriptions.push(cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.commit(ix, window, cx);
                    }
                }));
                input
            })
            .collect();
        let placeholders = vec![SharedString::default(); definition.fields.len()];
        Self { model, definition, inputs, placeholders, _subscriptions: subscriptions }
    }

    fn saved_of(model: &Entity<ServerPanelsModel>, definition: &HostSettingsDefinition, cx: &gpui_kit::App) -> serde_json::Map<String, Value> {
        model.read(cx).snapshot().settings.source_control_hosts.as_ref().and_then(|hosts| hosts.get(definition.kind)).cloned().unwrap_or_default()
    }

    fn saved(&self, cx: &gpui_kit::App) -> serde_json::Map<String, Value> {
        Self::saved_of(&self.model, self.definition, cx)
    }

    fn save(&mut self, fields: serde_json::Map<String, Value>, cx: &mut Context<Self>) {
        if fields.is_empty() {
            return;
        }
        let patch = ClientServerSettingsPatch {
            source_control_hosts: Some([(self.definition.kind.to_owned(), fields)].into_iter().collect()),
            ..Default::default()
        };
        save_patch(&self.model, format!("host-settings-{}", self.definition.kind), patch, cx);
    }

    /// Saves field `ix` when its text differs from the saved value; a configured secret keeps
    /// its value when the field is left empty.
    fn commit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let field = &self.definition.fields[ix];
        let text = self.inputs[ix].read(cx).value().trim().to_owned();
        let saved = self.saved(cx);
        let current = saved.get(field.key).and_then(Value::as_str).unwrap_or_default();
        if field.secret && text.is_empty() || !field.secret && text == current {
            return;
        }
        self.save([(field.key.to_owned(), Value::String(text))].into_iter().collect(), cx);
        if field.secret {
            self.inputs[ix].update(cx, |input, cx| input.set_value("", window, cx));
        }
    }
}

impl Render for SourceControlHostSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let saved = self.saved(cx);
        let has_saved = saved.values().any(|v| v.as_str() != Some(""));
        let saving = self.model.read(cx).is_pending(&format!("host-settings-{}", self.definition.kind));
        let entity = cx.entity();
        let mut rows = Vec::new();
        for (ix, field) in self.definition.fields.iter().enumerate() {
            let configured = field.secret && saved.get(field.key).is_some_and(|v| v.as_str() != Some(""));
            if field.secret {
                let placeholder = if configured { t!("Stored secret, enter a new value to replace") } else { t!("Not set") };
                if self.placeholders[ix] != placeholder {
                    self.placeholders[ix] = placeholder.clone();
                    self.inputs[ix].update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
                }
            }
            let input = TextInput::new(&self.inputs[ix]).size(InputSize::Sm).disabled(saving);
            let clear = configured.then(|| {
                let entity = entity.clone();
                let key = field.key;
                Button::new(SharedString::from(format!("host-clear-{}-{key}", self.definition.kind)))
                    .variant(ButtonVariant::GhostDestructive)
                    .size(ButtonSize::IconSm)
                    .icon(IconName::X)
                    .on_click(move |_, _, cx| entity.update(cx, |this, cx| this.save([(key.to_owned(), Value::String(String::new()))].into_iter().collect(), cx)))
            });
            let field_id = SharedString::from(format!("host-field-{}-{}", self.definition.kind, field.key));
            let control = div()
                .flex()
                .w(theme.rem(14.))
                .min_w_0()
                .items_center()
                .gap(theme.sp(2.))
                .child(div().id(field_id).flex_1().min_w_0().child(input).test_support())
                .children(clear);
            rows.push(SettingsRow::new(field.title).description(field.description).control(control));
        }
        let remove_entity = entity.clone();
        div().flex().flex_col().children(rows).when(has_saved, |d| {
            d.child(
                div().flex().justify_end().pt(theme.sp(2.)).child(
                    Button::new(SharedString::from(format!("host-remove-{}", self.definition.kind)))
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Xs)
                        .label(t!("Remove"))
                        .disabled(saving)
                        .on_click(move |_, _, cx| {
                            remove_entity.update(cx, |this, cx| {
                                let cleared = this.saved(cx).keys().map(|k| (k.clone(), Value::String(String::new()))).collect();
                                this.save(cleared, cx);
                            })
                        }),
                ),
            )
        })
    }
}
