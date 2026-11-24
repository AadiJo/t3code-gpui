//! The Usage providers and Text generators sections of the Providers page.
//! Ports `UsageProviderSettings.tsx` with `AddUsageLimitSourceDialog.tsx`, and
//! `TextGeneratorsSettings.tsx`: the list with its "…" menu (Edit, Show
//! models, Remove) and enable switch, the connect form, and the models dialog.
//! Saving a generator first tests it with `server.listTextGeneratorModels`.

use std::collections::BTreeMap;

use gpui_kit::{AnyElement, Context, IntoElement, SharedString, Window, div, prelude::*};
use t3code_contracts::{
    ClientServerSettingsPatch, ListTextGeneratorModelsInput, TextGeneratorConfig,
    TextGeneratorConfigApiKey, TextGeneratorKind, UsageLimitSourceConfig, UsageLimitSourceConfigKind,
    UsageLimitSourceConfigManagementKey,
};

use super::provider_settings_panel::{ProviderSettingsPanel, provider_instance_icon};
use super::server_settings_controls::{field_input, hint, tinted};
use super::settings_layout::{SettingsRow, SettingsSection};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::menu::{MenuItem, MenuPopup, dropdown_menu};
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::switch::Switch;
use crate::i18n::t;

/// `CLI_PROXY_NAME`.
const CLI_PROXY_NAME: &str = "CLI Proxy";

/// `TEXT_GENERATOR_PRESETS`: (id, label, kind, base URL; empty for a custom server).
fn presets() -> [(&'static str, SharedString, TextGeneratorKind, &'static str); 4] {
    [
        ("openai", "OpenAI".into(), TextGeneratorKind::OpenaiCompatible, "https://api.openai.com/v1"),
        ("openrouter", "OpenRouter".into(), TextGeneratorKind::OpenaiCompatible, "https://openrouter.ai/api/v1"),
        ("anthropic", "Anthropic".into(), TextGeneratorKind::Anthropic, "https://api.anthropic.com/v1"),
        ("custom", t!("Custom (OpenAI-compatible)"), TextGeneratorKind::OpenaiCompatible, ""),
    ]
}

/// `textGeneratorDisplayName`.
pub fn text_generator_display_name(config: &TextGeneratorConfig) -> String {
    if let Some(label) = config.label.as_deref().filter(|l| !l.is_empty()) {
        return label.to_owned();
    }
    if let Some((_, label, _, _)) = presets().into_iter().find(|(_, _, kind, url)| *url == config.base_url && *kind == config.kind) {
        return label.to_string();
    }
    url::Url::parse(&config.base_url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| config.base_url.clone())
}

/// `textGeneratorBrand`: the service whose mark the generator shows.
fn text_generator_brand(config: &TextGeneratorConfig) -> &'static str {
    if config.kind == TextGeneratorKind::Anthropic {
        return "anthropic";
    }
    let url = url::Url::parse(&config.base_url).ok();
    let host = url.as_ref().and_then(|u| u.host_str()).unwrap_or_default();
    if host == "openrouter.ai" || host.ends_with(".openrouter.ai") {
        "openrouter"
    } else if host == "api.openai.com" {
        "openai"
    } else if url.as_ref().and_then(|u| u.port()) == Some(11434) || host.contains("ollama") {
        "ollama"
    } else {
        "openaiCompatible"
    }
}

/// `sourceIdFromUrl`: the hub's id, unique among saved hubs.
fn source_id_from_url(url: &str, sources: &BTreeMap<String, UsageLimitSourceConfig>) -> String {
    if let Some((id, _)) = sources.iter().find(|(_, s)| s.url == url) {
        return id.clone();
    }
    let host = url::Url::parse(url).ok().map(|u| match u.port() {
        Some(p) => format!("{}:{p}", u.host_str().unwrap_or_default()),
        None => u.host_str().unwrap_or_default().to_owned(),
    });
    let host = host.unwrap_or_else(|| url.to_owned()).to_lowercase();
    let mut slug = String::new();
    for ch in host.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let base = format!("cliproxy-{}", if slug.is_empty() { "hub" } else { slug });
    let mut id = base.clone();
    let mut suffix = 2;
    while sources.contains_key(&id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

/// A dialog the two sections open.
#[derive(Clone, Debug)]
pub enum ExtraDialog {
    AddHub,
    RemoveHub { id: String, label: String },
    /// The connect form, for a new generator (`id` None) or an existing one.
    EditGenerator { id: Option<String>, preset: usize, error: Option<SharedString>, checking: bool },
    RemoveGenerator { id: String, name: String },
    Models { name: String, models: Option<Result<Vec<String>, SharedString>> },
}

impl ProviderSettingsPanel {
    fn save_settings(&mut self, key: &'static str, patch: ClientServerSettingsPatch, cx: &mut Context<Self>) {
        self.update_settings(key, patch, cx);
    }

    /// `UsageProviderSettings`.
    pub(super) fn render_usage_sources(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let read_only = snapshot.read_only;
        let sources = snapshot.settings.usage_limit_sources.clone().unwrap_or_default();
        let mut section = SettingsSection::new(t!("Usage providers"));
        if !read_only {
            section = section.header_action(
                Button::new("usage-add-hub")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Xs)
                    .icon(IconName::Plus)
                    .label(t!("Add hub"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.fields.reset_prefix("hub:");
                        this.extra_dialog = Some(ExtraDialog::AddHub);
                        cx.notify();
                    })),
            );
        }
        if sources.is_empty() {
            section.extend([SettingsRow::new(t!("No hubs configured.")).into_any_element()]);
        }
        for (id, source) in sources {
            let label = source.label.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| source.url.clone());
            let mut description = if source.enabled.unwrap_or(true) {
                CLI_PROXY_NAME.to_owned()
            } else {
                t!("{name} · Disabled", name = CLI_PROXY_NAME).to_string()
            };
            if label != source.url {
                description.push_str(&format!(" · {}", source.url));
            }
            let mut row = SettingsRow::new(SharedString::from(label.clone())).description(description);
            if !read_only {
                let (remove_id, remove_label) = (id.clone(), label.clone());
                row = row.control(Button::new(SharedString::from(format!("usage-remove-{id}"))).variant(ButtonVariant::Ghost).size(ButtonSize::Xs).label(t!("Remove")).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.extra_dialog = Some(ExtraDialog::RemoveHub { id: remove_id.clone(), label: remove_label.clone() });
                        cx.notify();
                    },
                )));
            }
            section.extend([row.into_any_element()]);
        }
        section.into_any_element()
    }

    /// `TextGeneratorsSettings`.
    pub(super) fn render_text_generators(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let read_only = snapshot.read_only;
        let generators = snapshot.settings.text_generators.clone().unwrap_or_default();
        let mut section = SettingsSection::new(t!("Text generators"));
        if !read_only {
            section = section.header_action(
                Button::new("text-generator-add")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Xs)
                    .icon(IconName::Plus)
                    .label(t!("Add generator"))
                    .on_click(cx.listener(|this, _, _, cx| this.open_generator(None, cx))),
            );
        }
        if generators.is_empty() {
            section.extend([SettingsRow::new(t!("No text generators yet"))
                .description(t!("Add a model API to write thread titles, commit messages, and other short text without starting a coding agent."))
                .into_any_element()]);
        }
        for (id, config) in generators {
            let name = text_generator_display_name(&config);
            let enabled = config.enabled.unwrap_or(true);
            let url = config.base_url.trim_start_matches("https://").trim_start_matches("http://").to_owned();
            let title = div()
                .flex()
                .items_center()
                .gap(theme.sp(2.5))
                .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                .child(provider_instance_icon(text_generator_brand(&config), &name, None, cx))
                .child(name.clone());
            let mut row = SettingsRow::new(SharedString::default()).status(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(1.))
                    .child(div().text_color(theme.colors.foreground()).child(title))
                    .child(div().pl(theme.sp(7.5)).truncate().child(format!("{}{url}", if enabled { String::new() } else { t!("Off · ").to_string() }))),
            );
            if !read_only {
                let (edit_id, models_id, models_config, remove_id, remove_name) = (id.clone(), id.clone(), config.clone(), id.clone(), name.clone());
                let this = cx.entity().downgrade();
                let menu = dropdown_menu(
                    SharedString::from(format!("text-generator-menu-{id}")),
                    Button::new(SharedString::from(format!("text-generator-more-{id}"))).variant(ButtonVariant::GhostMuted).size(ButtonSize::IconSm).icon(IconName::EllipsisVertical),
                    move |_, _| {
                        let (this, this2, this3) = (this.clone(), this.clone(), this.clone());
                        let (edit_id, models_id, models_config, remove_id, remove_name) =
                            (edit_id.clone(), models_id.clone(), models_config.clone(), remove_id.clone(), remove_name.clone());
                        MenuPopup::new()
                            .item(MenuItem::new(t!("Edit")).on_select(move |_, cx| {
                                this.update(cx, |this, cx| this.open_generator(Some(edit_id.clone()), cx)).ok();
                            }))
                            .item(MenuItem::new(t!("Show models")).disabled(!enabled).on_select(move |_, cx| {
                                this2.update(cx, |this, cx| this.show_models(models_id.clone(), models_config.clone(), cx)).ok();
                            }))
                            .item(MenuItem::new(t!("Remove")).destructive().on_select(move |_, cx| {
                                this3
                                    .update(cx, |this, cx| {
                                        this.extra_dialog = Some(ExtraDialog::RemoveGenerator { id: remove_id.clone(), name: remove_name.clone() });
                                        cx.notify();
                                    })
                                    .ok();
                            }))
                    },
                );
                let toggle_config = config.clone();
                let toggle_id = id.clone();
                let this = cx.entity().downgrade();
                row = row.control(div().flex().items_center().gap(theme.sp(2.)).child(menu).child(
                    Switch::new(SharedString::from(format!("text-generator-{id}")), enabled).on_change(move |on, _, cx| {
                        let mut next = toggle_config.clone();
                        next.enabled = Some(on);
                        let patch = ClientServerSettingsPatch { text_generators: Some([(toggle_id.clone(), Some(next))].into_iter().collect()), ..Default::default() };
                        this.update(cx, |this, cx| this.save_settings("text-generators", patch, cx)).ok();
                    }),
                ));
            }
            section.extend([row.into_any_element()]);
        }
        section.into_any_element()
    }

    /// Opens the connect form for a new generator, or for the generator `id`.
    pub fn open_generator(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.fields.reset_prefix("generator:");
        let snapshot = self.model.read(cx).snapshot();
        let existing = id.as_ref().and_then(|id| snapshot.settings.text_generators.as_ref()?.get(id).cloned());
        let preset = existing
            .as_ref()
            .and_then(|c| presets().iter().position(|(_, _, kind, url)| *url == c.base_url && *kind == c.kind))
            .unwrap_or(if existing.is_some() { 3 } else { 0 });
        self.extra_dialog = Some(ExtraDialog::EditGenerator { id, preset, error: None, checking: false });
        cx.notify();
    }

    fn show_models(&mut self, id: String, config: TextGeneratorConfig, cx: &mut Context<Self>) {
        let name = text_generator_display_name(&config);
        self.extra_dialog = Some(ExtraDialog::Models { name, models: None });
        let input: ListTextGeneratorModelsInput = serde_json::from_value(serde_json::json!({ "id": id, "refresh": true })).expect("input");
        let this = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("text-generator-models", move |s| s.list_text_generator_models(input), move |result, cx| {
                this.update(cx, |this, cx| {
                    if let Some(ExtraDialog::Models { models, .. }) = this.extra_dialog.as_mut() {
                        *models = Some(result.map(|r| r.models.into_iter().map(|m| m.name).collect()).map_err(|e| e.message));
                    }
                    cx.notify();
                })
                .ok();
            }, cx)
        });
        cx.notify();
    }

    /// Tests the form's generator (`listTextGeneratorModels` with `test`) and saves it.
    fn connect_generator(&mut self, cx: &mut Context<Self>) {
        let Some(ExtraDialog::EditGenerator { id, preset, .. }) = self.extra_dialog.clone() else { return };
        let (preset_id, _, kind, preset_url) = presets()[preset].clone();
        let url = self.fields.value("generator:url", cx).map(|v| v.trim().to_owned()).unwrap_or_default();
        let base_url = if preset_url.is_empty() { url } else { preset_url.to_owned() };
        let key = self.fields.value("generator:key", cx).map(|v| v.trim().to_owned()).unwrap_or_default();
        let label = self.fields.value("generator:name", cx).map(|v| v.trim().to_owned()).unwrap_or_default();
        if base_url.is_empty() {
            if let Some(ExtraDialog::EditGenerator { error, .. }) = self.extra_dialog.as_mut() {
                *error = Some(t!("Enter a base URL."));
            }
            cx.notify();
            return;
        }
        let snapshot = self.model.read(cx).snapshot().clone();
        let existing = id.as_ref().and_then(|id| snapshot.settings.text_generators.as_ref()?.get(id).cloned());
        let api_key = if key.is_empty() { existing.as_ref().and_then(|c| c.api_key.clone()) } else { Some(TextGeneratorConfigApiKey::Text(key)) };
        let config = TextGeneratorConfig {
            kind,
            label: (!label.is_empty()).then_some(label),
            base_url,
            api_key,
            enabled: Some(existing.as_ref().and_then(|c| c.enabled).unwrap_or(true)),
        };
        let id = id.unwrap_or_else(|| {
            let taken: Vec<String> = snapshot.settings.text_generators.iter().flatten().map(|(k, _)| k.clone())
                .chain(snapshot.settings.provider_instances.iter().flatten().map(|(k, _)| k.clone()))
                .chain(snapshot.providers.iter().map(|p| p.instance_id.to_string()))
                .collect();
            let mut candidate = preset_id.to_owned();
            let mut n = 2;
            while taken.contains(&candidate) {
                candidate = format!("{preset_id}_{n}");
                n += 1;
            }
            candidate
        });
        if let Some(ExtraDialog::EditGenerator { checking, error, .. }) = self.extra_dialog.as_mut() {
            *checking = true;
            *error = None;
        }
        let input: ListTextGeneratorModelsInput = serde_json::from_value(serde_json::json!({ "id": id, "config": config, "test": true })).expect("input");
        let this = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("text-generator-test", move |s| s.list_text_generator_models(input), move |result, cx| {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(_) => {
                            let patch = ClientServerSettingsPatch { text_generators: Some([(id.clone(), Some(config.clone()))].into_iter().collect()), ..Default::default() };
                            this.save_settings("text-generators", patch, cx);
                            this.extra_dialog = None;
                        }
                        Err(e) => {
                            if let Some(ExtraDialog::EditGenerator { checking, error, .. }) = this.extra_dialog.as_mut() {
                                *checking = false;
                                *error = Some(e.message);
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            }, cx)
        });
        cx.notify();
    }

    fn add_hub(&mut self, cx: &mut Context<Self>) {
        let url = self.fields.value("hub:url", cx).map(|v| v.trim().to_owned()).unwrap_or_default();
        let key = self.fields.value("hub:key", cx).map(|v| v.trim().to_owned()).unwrap_or_default();
        let label = self.fields.value("hub:label", cx).map(|v| v.trim().to_owned()).unwrap_or_default();
        if url.is_empty() || key.is_empty() {
            return;
        }
        let sources = self.model.read(cx).snapshot().settings.usage_limit_sources.clone().unwrap_or_default();
        let id = source_id_from_url(&url, &sources);
        let config = UsageLimitSourceConfig {
            kind: UsageLimitSourceConfigKind::Cliproxy,
            label: (!label.is_empty()).then_some(label),
            url,
            management_key: Some(UsageLimitSourceConfigManagementKey::Text(key)),
            enabled: Some(true),
        };
        let patch = ClientServerSettingsPatch { usage_limit_sources: Some([(id, Some(config))].into_iter().collect()), ..Default::default() };
        self.save_settings("usage-sources", patch, cx);
        self.extra_dialog = None;
        cx.notify();
    }

    /// The open hub or generator dialog, if any.
    pub(super) fn render_extra_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = &cx.app_theme().clone();
        let dialog = self.extra_dialog.clone()?;
        let environment = self.model.read(cx).snapshot().environment_label.clone();
        let cancel = |id: &'static str, cx: &mut Context<Self>| {
            Button::new(id).variant(ButtonVariant::Outline).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                this.extra_dialog = None;
                cx.notify();
            }))
        };
        let labeled = |label: SharedString, control: AnyElement| {
            div().flex().flex_col().gap(theme.sp(1.5)).child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(label)).child(control).into_any_element()
        };
        let surface = match dialog {
            ExtraDialog::AddHub => {
                let url = self.fields.get("hub:url", "", "https://hub.example.ts.net:8318", false, window, cx);
                let key = self.fields.get("hub:key", "", "", true, window, cx);
                let label = self.fields.get("hub:label", "", t!("Defaults to the hub's host name"), false, window, cx);
                DialogSurface::new(t!("Add a CLIProxyAPI hub"))
                    .description(t!("Show the quota of every account the hub pools, next to the providers on {environment}. The key stays on that server.", environment = environment))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(4.))
                            .child(labeled(t!("Hub URL"), field_input(&url, false, false, false).into_any_element()))
                            .child(labeled(t!("Management key"), field_input(&key, false, false, false).into_any_element()))
                            .child(labeled(t!("Label (optional)"), field_input(&label, false, false, false).into_any_element())),
                    )
                    .footer(cancel("hub-cancel", cx))
                    .footer(Button::new("hub-add").label(t!("Add hub")).on_click(cx.listener(|this, _, _, cx| this.add_hub(cx))))
            }
            ExtraDialog::RemoveHub { id, label } => DialogSurface::new(t!("Remove {label}?", label = label))
                .description(t!("The hub's management key is deleted from this server. Its accounts leave the Limits view; the hub itself is untouched. Add it again with the URL and key to bring them back."))
                .footer(cancel("hub-remove-cancel", cx))
                .footer(Button::new("hub-remove").variant(ButtonVariant::Destructive).label(t!("Remove hub")).on_click(cx.listener(move |this, _, _, cx| {
                    let patch = ClientServerSettingsPatch { usage_limit_sources: Some([(id.clone(), None)].into_iter().collect()), ..Default::default() };
                    this.save_settings("usage-sources", patch, cx);
                    this.extra_dialog = None;
                    cx.notify();
                }))),
            ExtraDialog::RemoveGenerator { id, name } => DialogSurface::new(t!("Remove {name}?", name = name))
                .description(t!("This deletes the API key stored on {environment}. Anything set to use this generator falls back to the text generation model.", environment = environment))
                .footer(cancel("generator-remove-cancel", cx))
                .footer(Button::new("generator-remove").variant(ButtonVariant::Destructive).label(t!("Remove")).on_click(cx.listener(move |this, _, _, cx| {
                    let patch = ClientServerSettingsPatch { text_generators: Some([(id.clone(), None)].into_iter().collect()), ..Default::default() };
                    this.save_settings("text-generators", patch, cx);
                    this.extra_dialog = None;
                    cx.notify();
                }))),
            ExtraDialog::Models { name, models } => {
                let body: AnyElement = match models {
                    None => hint(t!("Loading the model list."), cx),
                    Some(Err(message)) => tinted(SharedString::from(format!("{} {message}", t!("The model list couldn't load."))), theme.colors.destructive(), cx),
                    Some(Ok(list)) if list.is_empty() => hint(t!("No models match."), cx),
                    Some(Ok(list)) => div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(1.))
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .children(list.into_iter().map(|m| div().child(m)))
                        .into_any_element(),
                };
                DialogSurface::new(t!("{name} models", name = name)).child(body).footer(
                    Button::new("generator-models-done").label(t!("Done")).on_click(cx.listener(|this, _, _, cx| {
                        this.extra_dialog = None;
                        cx.notify();
                    })),
                )
            }
            ExtraDialog::EditGenerator { id, preset, error, checking } => {
                let snapshot = self.model.read(cx).snapshot().clone();
                let existing = id.as_ref().and_then(|id| snapshot.settings.text_generators.as_ref()?.get(id).cloned());
                let title = match &existing {
                    Some(c) => t!("Edit {name}", name = text_generator_display_name(c)),
                    None => t!("Add a text generator"),
                };
                let all = presets();
                let custom = all[preset].3.is_empty();
                let key_required = all[preset].2 == TextGeneratorKind::Anthropic || !custom;
                let url = self.fields.get("generator:url", existing.as_ref().map(|c| c.base_url.clone()).unwrap_or_default(), "http://localhost:11434/v1", false, window, cx);
                let key = self.fields.get("generator:key", "", if existing.as_ref().is_some_and(|c| c.api_key.is_some()) { t!("Saved") } else { SharedString::default() }, true, window, cx);
                let name = self.fields.get("generator:name", existing.as_ref().and_then(|c| c.label.clone()).unwrap_or_default(), if custom { t!("e.g. My local model") } else { all[preset].1.clone() }, false, window, cx);
                let this = cx.entity().downgrade();
                let service = Select::new("generator-service", all.iter().map(|(id, label, _, _)| if *id == "custom" { t!("Custom") } else { label.clone() }).collect::<Vec<_>>(), preset, move |ix, _, cx| {
                    this.update(cx, |this, cx| {
                        if let Some(ExtraDialog::EditGenerator { preset, .. }) = this.extra_dialog.as_mut() {
                            *preset = ix;
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .size(SelectSize::Sm);
                let mut form = div().flex().flex_col().gap(theme.sp(4.)).child(labeled(t!("Service"), service.into_any_element()));
                if custom {
                    form = form.child(labeled(t!("Base URL"), field_input(&url, false, false, false).into_any_element()));
                }
                form = form
                    .child(labeled(if key_required { t!("API key") } else { t!("API key (optional)") }, field_input(&key, false, false, false).into_any_element()))
                    .child(labeled(t!("Name (optional)"), field_input(&name, false, false, false).into_any_element()))
                    .children(error.map(|e| tinted(e, theme.colors.destructive(), cx)));
                DialogSurface::new(title)
                    .child(form)
                    .footer(cancel("generator-cancel", cx))
                    .footer(
                        Button::new("generator-connect")
                            .label(if checking { t!("Connecting") } else if existing.is_some() { t!("Save and continue") } else { t!("Connect") })
                            .disabled(checking)
                            .on_click(cx.listener(|this, _, _, cx| this.connect_generator(cx))),
                    )
            }
        };
        let this = cx.entity().downgrade();
        Some(
            Modal::dialog("provider-extra-dialog", surface, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.extra_dialog = None;
                    cx.notify();
                })
                .ok();
            })
            .into_any_element(),
        )
    }
}
