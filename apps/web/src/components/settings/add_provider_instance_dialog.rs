//! "Add provider". Ports `AddProviderInstanceDialog.tsx` and its wizard
//! steps: pick a driver (featured first, the rest under "Show more"), name
//! the instance (label, derived or typed id, accent colour), then fill the
//! driver's settings form, and create it with `server.updateSettings` and a
//! `create` provider instance mutation.
//!
//! The ACP Registry search step and the managed ChatGPT account flow are not
//! built; the registry entry adds an instance by agent id or local command.

use std::collections::{BTreeMap, HashSet};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{TestSupportExt as _, App, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{
    ClientServerSettingsPatch, ProviderInstanceConfig, ProviderInstanceId, ProviderInstanceMutation,
    ProviderInstanceMutationCreate, ServerUpdateSettingsPayload,
};

use super::provider_driver_meta::{DRIVERS, FieldControl, driver_meta, local_acp_fields};
use super::provider_settings_panel::{provider_instance_icon, toast_error};
use super::provider_accent_color_picker::ProviderAccentColorPicker;
use super::server_settings_controls::{Fields, field_input, hint, tinted};
use super::server_settings_data::ServerPanelsModel;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize};
use crate::components::managed_policy::is_managed_driver_allowed;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::dialog_glass;
use crate::components::ui::select::{Select, SelectSize};
use crate::components::ui::switch::Switch;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::provider_instances::{derive_available_instance_id, derive_instance_id, normalize_provider_accent_color};
use crate::theme_color::with_alpha;

/// `validateInstanceId`.
pub fn validate_instance_id(id: &str, existing: &HashSet<String>) -> Option<SharedString> {
    if id.is_empty() {
        return Some(t!("Instance ID is required."));
    }
    if id.chars().count() > 64 {
        return Some(t!("Instance ID must be 64 characters or fewer."));
    }
    let mut chars = id.chars();
    let pattern = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !pattern {
        return Some(t!("Instance ID must start with a letter and use only letters, digits, '-', or '_'."));
    }
    existing.contains(id).then(|| t!("An instance named ''{id}'' already exists.", id = id))
}

/// The dialog closes, or created an instance (its id).
#[derive(Clone, Debug)]
pub enum AddProviderInstanceEvent {
    Close,
    Created(String),
}

/// The wizard.
pub struct AddProviderInstanceDialog {
    model: Entity<ServerPanelsModel>,
    step: usize,
    driver: String,
    show_more: bool,
    local_acp: bool,
    label: Entity<InputState>,
    instance_id: Entity<InputState>,
    accent_picker: Entity<ProviderAccentColorPicker>,
    /// The accent chosen in the picker, normalized.
    accent: Option<String>,
    /// Typed ids per driver; `None` follows the label.
    id_override: BTreeMap<String, String>,
    config: serde_json::Map<String, serde_json::Value>,
    fields: Fields,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AddProviderInstanceEvent> for AddProviderInstanceDialog {}

impl AddProviderInstanceDialog {
    pub fn new(model: Entity<ServerPanelsModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let label = cx.new(|cx| InputState::new(window, cx).placeholder(t!("e.g. Work")));
        let instance_id = cx.new(|cx| InputState::new(window, cx).placeholder("codex_work"));
        let accent_picker = cx.new(|cx| ProviderAccentColorPicker::new(window, cx));
        let on_label = cx.subscribe(&label, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
                let _ = this;
            }
        });
        let on_id = cx.subscribe(&instance_id, |this, state, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                let value = state.read(cx).value().to_string();
                this.id_override.insert(this.driver.clone(), value);
                cx.notify();
            }
        });
        let mut this = Self {
            model,
            step: 0,
            driver: "codex".into(),
            show_more: false,
            local_acp: false,
            label,
            instance_id,
            accent_picker,
            accent: None,
            id_override: BTreeMap::new(),
            config: serde_json::Map::new(),
            fields: Fields::default(),
            saving: false,
            _subscriptions: vec![on_label, on_id],
        };
        let first_allowed = {
            let snapshot = this.model.read(cx).snapshot();
            DRIVERS.iter().find(|d| **d != "acpRegistry" && is_managed_driver_allowed(snapshot.managed_policy.as_deref(), d)).copied()
        };
        if let Some(driver) = first_allowed {
            this.driver = driver.into();
        }
        this
    }

    pub fn step(&self) -> usize {
        self.step
    }

    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// Picks a driver (step one).
    pub fn pick_driver(&mut self, driver: &str, cx: &mut Context<Self>) {
        self.driver = driver.into();
        self.config.clear();
        self.fields = Fields::default();
        cx.notify();
    }

    /// The label and id fields, for scripted checks.
    pub fn identity_fields(&self) -> (&Entity<InputState>, &Entity<InputState>) {
        (&self.label, &self.instance_id)
    }

    fn existing_ids(&self, cx: &App) -> HashSet<String> {
        let snapshot = self.model.read(cx).snapshot();
        let mut ids: HashSet<String> = snapshot.settings.provider_instances.clone().unwrap_or_default().into_keys().collect();
        ids.extend(snapshot.providers.iter().map(|p| p.instance_id.clone()));
        ids
    }

    /// The id the instance will get: typed, or derived from the label.
    pub fn resolved_instance_id(&self, cx: &App) -> String {
        if let Some(typed) = self.id_override.get(&self.driver).filter(|v| !v.is_empty()) {
            return typed.clone();
        }
        let label = self.label.read(cx).value();
        let meta_label = driver_meta(&self.driver).map_or(self.driver.clone(), |m| m.label.to_owned());
        let label = if label.trim().is_empty() { format!("{meta_label} Workspace") } else { label.to_string() };
        derive_available_instance_id(derive_instance_id(&self.driver, &label), &self.existing_ids(cx))
    }

    fn id_error(&self, cx: &App) -> Option<SharedString> {
        validate_instance_id(&self.resolved_instance_id(cx), &self.existing_ids(cx))
    }

    fn steps(&self) -> Vec<SharedString> {
        let mut steps = vec![t!("Provider", context = "wizard step"), t!("Identity", context = "wizard step")];
        if self.driver == "acpRegistry" {
            if !self.local_acp {
                steps.push(t!("Sign in", context = "wizard step"));
            }
        } else {
            steps.push(t!("Config", context = "wizard step"));
        }
        steps
    }

    /// `resolveWizardNavigation`: leaving the identity step needs a valid id.
    pub fn go_to(&mut self, step: usize, cx: &mut Context<Self>) {
        let last = self.steps().len() - 1;
        let step = step.min(last);
        if step > 1 && self.id_error(cx).is_some() {
            self.step = 1;
        } else {
            self.step = step;
        }
        cx.notify();
    }

    /// Creates the instance.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.id_error(cx).is_some() {
            self.step = 1;
            cx.notify();
            return;
        }
        let id = self.resolved_instance_id(cx);
        let label = self.label.read(cx).value().trim().to_owned();
        let accent = normalize_provider_accent_color(self.accent.as_deref());
        let mut config = self.config.clone();
        if self.driver == "acpRegistry" {
            config.insert("source".into(), serde_json::json!(if self.local_acp { "local" } else { "registry" }));
        }
        let instance = ProviderInstanceConfig {
            driver: t3code_contracts::ProviderDriverKind::new(self.driver.as_str()),
            display_name: (!label.is_empty()).then_some(label),
            accent_color: accent,
            environment: None,
            enabled: Some(true),
            config: (!config.is_empty()).then_some(serde_json::Value::Object(config)),
        };
        let input = ServerUpdateSettingsPayload {
            patch: ClientServerSettingsPatch::default(),
            provider_instance_mutation: Some(ProviderInstanceMutation::Create(ProviderInstanceMutationCreate {
                instance_id: ProviderInstanceId::new(id.as_str()),
                instance,
            })),
        };
        let provider = driver_meta(&self.driver).map_or(self.driver.clone(), |m| m.label.to_owned());
        self.saving = true;
        let this = cx.entity().downgrade();
        self.model.update(cx, |model, cx| {
            model.run("add-instance", move |s| s.update_settings(input), move |result, cx| {
                this.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(_) => {
                            Toaster::push(
                                ToastData::new(ToastKind::Success, t!("Provider instance added")).description(t!(
                                    "{provider} instance ''{instanceId}'' was added.",
                                    provider = provider,
                                    instanceId = id.clone()
                                )),
                                cx,
                            );
                            cx.emit(AddProviderInstanceEvent::Created(id.clone()));
                        }
                        Err(error) => toast_error(t!("Could not add provider instance"), error, cx),
                    }
                    cx.notify();
                })
                .ok();
            }, cx)
        });
        cx.notify();
    }

    fn render_driver_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let policy = self.model.read(cx).snapshot().managed_policy.clone();
        let card = |driver: &'static str, this: &Self, cx: &mut Context<Self>| {
            let meta = driver_meta(driver);
            let label = meta.as_ref().map_or(driver, |m| m.label);
            let selected = this.driver == driver;
            let allowed = is_managed_driver_allowed(policy.as_deref(), driver);
            // `ProviderOptionGrid` radio: ring-1 white/5 on white/3, checked ring-2 primary on primary/15 with a check badge.
            div()
                .id(SharedString::from(format!("add-driver-{driver}")))
                .relative()
                .flex()
                .items_center()
                .gap(theme.sp(3.))
                // The web draws these borders as rings, outside layout; the padding gives the width back.
                .when(!selected, |d| d.px(theme.sp(3.) - px(1.)).py(theme.sp(3.) - px(1.)))
                .when(selected, |d| d.px(theme.sp(3.) - px(2.)).py(theme.sp(3.) - px(2.)))
                .rounded(theme.radii.lg)
                .when(!selected, |d| d.border_1().border_color(with_alpha(c.foreground(), 0.05)).bg(with_alpha(c.foreground(), 0.03)))
                .when(selected, |d| d.border_2().border_color(c.primary()).bg(with_alpha(c.primary(), 0.15)))
                .when(!allowed, |d| d.opacity(0.5))
                .when(allowed && !selected, |d| d.cursor_pointer().hover(|d| d.bg(with_alpha(c.foreground(), 0.05))))
                .when(allowed, |d| d.on_click(cx.listener(move |this, _, _, cx| this.pick_driver(driver, cx))))
                .child(provider_instance_icon(driver, label, None, cx))
                .child(div().flex_1().min_w_0().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label))
                .when(selected, |d| {
                    d.child(
                        div()
                            .size(theme.sp(5.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(c.primary())
                            .child(Icon::new(IconName::Check).size(IconSize::Sm).color(c.primary_foreground())),
                    )
                })
                .children(meta.and_then(|m| m.badge_label).map(|b| Badge::new(b).variant(BadgeVariant::Warning).size(BadgeSize::Sm)))
                .test_support()
        };
        let featured: Vec<&'static str> =
            DRIVERS.iter().copied().filter(|d| *d != "acpRegistry" && driver_meta(d).is_some_and(|m| m.featured)).collect();
        let more: Vec<&'static str> =
            DRIVERS.iter().copied().filter(|d| *d != "acpRegistry" && driver_meta(d).is_some_and(|m| !m.featured)).collect();
        let grid = |drivers: &[&'static str], this: &Self, cx: &mut Context<Self>| {
            div().grid().grid_cols(2).gap(theme.sp(3.)).children(drivers.iter().map(|d| card(d, this, cx)).collect::<Vec<_>>())
        };
        let first = grid(&featured, self, cx);
        let acp_selected = self.driver == "acpRegistry";
        let more_content = self.show_more.then(|| {
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(4.))
                .pt(theme.sp(4.))
                .child(grid(&more, self, cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.sp(3.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(div().flex_1().border_t_1().border_color(with_alpha(c.border, 0.7)))
                        .child(t!("Or choose from ACP Registry"))
                        .child(div().flex_1().border_t_1().border_color(with_alpha(c.border, 0.7))),
                )
                .child(
                    div()
                        .flex()
                        .gap(theme.sp(2.))
                        .child(
                            Button::new("add-acp-manual")
                                .variant(if acp_selected && !self.local_acp { ButtonVariant::Secondary } else { ButtonVariant::Outline })
                                .size(ButtonSize::Sm)
                                .label(t!("Enter manually"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pick_driver("acpRegistry", cx);
                                    this.local_acp = false;
                                })),
                        )
                        .child(
                            Button::new("add-acp-local")
                                .variant(if acp_selected && self.local_acp { ButtonVariant::Secondary } else { ButtonVariant::Outline })
                                .size(ButtonSize::Sm)
                                .label(t!("Local ACP command"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pick_driver("acpRegistry", cx);
                                    this.local_acp = true;
                                })),
                        ),
                )
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(2.))
                    .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("Provider", context = "wizard step")))
                    .child(first),
            )
            .child(
                div().flex().pt(theme.sp(2.)).child(
                    Button::new("add-driver-more")
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Xs)
                        .label(if self.show_more { t!("Show less") } else { t!("Show more") })
                        .trailing_icon(if self.show_more { IconName::ChevronDown } else { IconName::ChevronRight })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_more = !this.show_more;
                            cx.notify();
                        })),
                ),
            )
            .children(more_content)
    }

    fn render_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label_text = self.label.read(cx).value();
        let display: SharedString = if label_text.trim().is_empty() { driver_meta(&self.driver).map_or_else(|| self.driver.clone().into(), |m| m.label.into()) } else { label_text };
        let this = cx.entity().downgrade();
        let accent = self.accent.clone();
        let picker = self.accent_picker.clone();
        picker.update(cx, |p, cx| {
            p.sync(display, accent.as_deref(), move |color, _, cx| {
                this.update(cx, |this, cx| {
                    this.accent = (!color.is_empty()).then_some(color);
                    cx.notify();
                })
                .ok();
            }, window, cx)
        });
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let error = self.id_error(cx);
        let resolved = self.resolved_instance_id(cx);
        let label_row = |title: SharedString, desc: SharedString, control: gpui_kit::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(theme.sp(6.))
                .py(theme.sp(2.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(theme.sp(1.))
                        .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(title))
                        .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(desc)),
                )
                .child(div().w(theme.sp(56.)).child(control))
        };
        div()
            .flex()
            .flex_col()
            .child(label_row(t!("Label", context = "noun, display name"), t!("Shown in the provider list."), field_input(&self.label, false, false, false).into_any_element()))
            .child(label_row(
                t!("Instance ID"),
                t!("Letters, digits, '-', or '_'."),
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(1.))
                    .child(field_input(&self.instance_id, true, error.is_some(), false))
                    .child(hint(resolved, cx))
                    .into_any_element(),
            ))
            .children(error.map(|e| tinted(e, c.destructive(), cx)))
            .child(label_row(t!("Accent color"), t!("Optional marker shown in the picker."), div().flex().justify_end().child(picker).into_any_element()))
    }

    fn render_config(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let fields = match driver_meta(&self.driver) {
            Some(_) if self.driver == "acpRegistry" && self.local_acp => local_acp_fields(),
            Some(m) => m.fields,
            None => Vec::new(),
        };
        if fields.is_empty() {
            return div()
                .child(hint(t!("This driver has no required configuration. You can add the instance now."), cx))
                .into_any_element();
        }
        let mut body = div().flex().flex_col().gap(theme.sp(3.));
        for field in fields {
            let key = field.key;
            let control = match field.control {
                FieldControl::Switch => {
                    let checked = self.config.get(key).and_then(|v| v.as_bool()).or(field.default_bool).unwrap_or(false);
                    Switch::new(SharedString::from(format!("add-cfg-{key}")), checked)
                        .on_change({
                            let this = cx.entity().downgrade();
                            move |v, _, cx| {
                                this.update(cx, |this, cx| {
                                    this.config.insert(key.into(), serde_json::Value::Bool(v));
                                    cx.notify();
                                })
                                .ok();
                            }
                        })
                        .into_any_element()
                }
                FieldControl::Select => {
                    let labels: Vec<SharedString> = field.options.iter().map(|o| o.label.clone()).collect();
                    let values: Vec<&'static str> = field.options.iter().map(|o| o.value).collect();
                    let ix = self.config.get(key).and_then(|v| v.as_str()).and_then(|v| values.iter().position(|o| *o == v)).unwrap_or(0);
                    let this = cx.entity().downgrade();
                    Select::new(SharedString::from(format!("add-cfg-{key}")), labels, ix, move |ix, _, cx| {
                        let value = values[ix];
                        this.update(cx, |this, cx| {
                            this.config.insert(key.into(), serde_json::Value::String(value.into()));
                            cx.notify();
                        })
                        .ok();
                    })
                    .size(SelectSize::Sm)
                    .into_any_element()
                }
                _ => {
                    let input = self.fields.get(format!("cfg:{key}"), "", field.placeholder.clone().unwrap_or_default(), field.control == FieldControl::Password, window, cx);
                    let value = input.read(cx).value().trim().to_owned();
                    if value.is_empty() {
                        self.config.remove(key);
                    } else {
                        self.config.insert(key.into(), serde_json::Value::String(value));
                    }
                    field_input(&input, true, false, false).into_any_element()
                }
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(6.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .gap(theme.sp(1.))
                            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(field.label.clone()))
                            .children(field.description.clone().map(|d| hint(d, cx))),
                    )
                    .child(div().w(theme.sp(56.)).child(control)),
            );
        }
        body.into_any_element()
    }
}

impl Render for AddProviderInstanceDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let steps = self.steps();
        let last = steps.len() - 1;
        let environment = self.model.read(cx).snapshot().environment_label.clone();
        // `WizardSteps`: equal cells in a white/4 tray; the current cell is a card.
        let stepper = div()
            .grid()
            .grid_cols(steps.len() as u16)
            .gap(theme.sp(1.))
            .p(theme.sp(1.))
            .rounded(theme.radii.xl)
            .bg(with_alpha(c.foreground(), 0.04))
            .border_1()
            .border_color(with_alpha(c.foreground(), 0.05))
            .children(steps.iter().enumerate().map(|(ix, label)| {
                let active = ix == self.step;
                let done = ix < self.step;
                div()
                    .id(SharedString::from(format!("add-step-{ix}")))
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .min_w_0()
                    .px(theme.sp(2.5))
                    .py(theme.sp(2.))
                    .rounded(theme.radii.lg)
                    .cursor_pointer()
                    .when(active, |d| d.bg(c.card()).border_1().border_color(with_alpha(c.foreground(), 0.05)))
                    .when(!active, |d| d.hover(|d| d.bg(c.card())))
                    .on_click(cx.listener(move |this, _, _, cx| this.go_to(ix, cx)))
                    .child(
                        div()
                            .size(theme.sp(5.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border_1()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .map(|d| {
                                if done {
                                    d.bg(c.primary()).border_color(c.primary()).text_color(c.primary_foreground())
                                } else if active {
                                    d.bg(with_alpha(c.primary(), 0.1)).border_color(with_alpha(c.primary(), 0.3)).text_color(c.primary())
                                } else {
                                    d.bg(with_alpha(c.foreground(), 0.05)).border_color(with_alpha(c.foreground(), 0.1)).text_color(c.muted_foreground)
                                }
                            })
                            .child(if done {
                                Icon::new(IconName::Check).size(IconSize::Md).color(c.primary_foreground()).into_any_element()
                            } else {
                                (ix + 1).to_string().into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(if active { c.foreground() } else { c.muted_foreground })
                            .child(label.clone()),
                    )
            }));
        let panel = match self.step {
            0 => self.render_driver_grid(cx).into_any_element(),
            1 => self.render_identity(window, cx).into_any_element(),
            _ => self.render_config(window, cx).into_any_element(),
        };
        let next_label = if self.saving {
            t!("Adding...")
        } else if self.driver == "acpRegistry" && !self.local_acp && self.step == last {
            t!("Continue to sign-in")
        } else {
            t!("Add instance")
        };
        dialog_glass(theme)
            .id("add-provider-dialog")
            .w(theme.rem(48.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .p(theme.sp(6.))
                    .pb(theme.sp(4.))
                    .child(div().ui_text(theme, TextSize::Xl, weight::SEMIBOLD).child(t!("Add provider")))
                    .child(div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(t!(
                        "Add an account or configure a provider on {environment}.",
                        environment = environment
                    )))
                    .child(stepper),
            )
            // `WizardPanel`: white/2 with a white/5 hairline above and below.
            .child(
                div()
                    .px(theme.sp(6.))
                    .py(theme.sp(5.))
                    .bg(with_alpha(c.foreground(), 0.02))
                    .border_t_1()
                    .border_b_1()
                    .border_color(with_alpha(c.foreground(), 0.05))
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(panel),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.sp(2.))
                    .px(theme.sp(6.))
                    .py(theme.sp(4.))
                    .child(
                        Button::new("add-back")
                            .variant(if self.step == 0 { ButtonVariant::GhostMuted } else { ButtonVariant::Outline })
                            .size(ButtonSize::Sm)
                            .label(if self.step == 0 { t!("Cancel") } else { t!("Back") })
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.step == 0 {
                                    cx.emit(AddProviderInstanceEvent::Close);
                                } else {
                                    this.go_to(this.step - 1, cx);
                                }
                            })),
                    )
                    .when(self.step == 0 && self.driver == "codex", |d| {
                        // The ChatGPT sign-in (`AddManagedCodexAccountDialog`) is not ported; see the report.
                        d.child(Button::new("add-configure").variant(ButtonVariant::Outline).size(ButtonSize::Sm).label(t!("Configure manually")).on_click(cx.listener(|this, _, _, cx| this.go_to(1, cx))))
                            .child(chatgpt_button(cx))
                    })
                    .when(self.step < last && !(self.step == 0 && self.driver == "codex"), |d| {
                        d.child(Button::new("add-next").size(ButtonSize::Sm).label(t!("Next")).on_click(cx.listener(|this, _, _, cx| this.go_to(this.step + 1, cx))))
                    })
                    .when(self.step == last, |d| {
                        d.child(
                            Button::new("add-submit")
                                .size(ButtonSize::Sm)
                                .label(next_label)
                                .disabled(self.saving)
                                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                        )
                    }),
            )
    }
}

/// `ChatGptConnectionButton` (size sm): the primary button with the OpenAI mark.
fn chatgpt_button(cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    div()
        .id("add-chatgpt")
        .flex()
        .items_center()
        .gap(theme.sp(1.5))
        .h(theme.sp(7.))
        .px(theme.sp(2.5))
        .rounded(theme.radii.lg)
        .bg(c.primary())
        .text_color(c.primary_foreground())
        .ui_text(theme, TextSize::Sm, weight::MEDIUM)
        .cursor_pointer()
        .hover(|d| d.opacity(0.9))
        .child(Icon::new(BrandIcon::OpenAi).size(IconSize::Md).color(c.primary_foreground()))
        .child(t!("Continue with ChatGPT"))
}
