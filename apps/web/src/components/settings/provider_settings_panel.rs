//! Settings → Providers. Ports `ProviderSettingsPanel.tsx`,
//! `ProviderInstanceCard.tsx`, `ProviderSettingsForm.tsx` (fields from the
//! driver table), `ProviderEnvironmentSection`, `ProviderModelsSection`,
//! `UsageProviderSettings.tsx` and `TextGeneratorsSettings.tsx` (lists), and
//! the Advanced section. Sign-in lives in `provider_authentication_section`.
//!
//! The panel acts on one environment through [`ServerPanelsModel`].

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use gpui_kit::{TestSupportExt as _, 
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use t3code_contracts::{
    ClientServerSettingsPatch, ClientServerSettingsPatchSetModelFavoritesItem, ClientServerSettingsPatchSetModelsHiddenItem,
    ProviderDriverKind, ProviderInstanceConfig, ProviderInstanceId, ProviderInstanceMutation,
    ProviderInstanceMutationRemove, ProviderInstanceMutationUpsert, ServerProviderUpdateInput,
    ServerProviderUpdateStatus, ServerRefreshProvidersPayload, ServerUpdateSettingsPayload,
};

use super::add_provider_instance_dialog::{AddProviderInstanceDialog, AddProviderInstanceEvent};
use super::provider_accent_color_picker::ProviderAccentColorPicker;
use super::provider_driver_meta::{DRIVERS, DriverMeta, FieldControl, driver_meta, local_acp_fields};
use super::provider_status::{
    AdvisoryEmphasis, ProviderStatusKey, get_provider_status_key, get_provider_summary,
    get_provider_version_advisory_presentation, get_provider_version_label, provider_auth_label,
};
use super::server_settings_controls::{Fields, field_input, hint, interval_seconds_field, reset_button, select_row, tinted};
use super::server_settings_data::{RpcFailure, ServerPanelsModel};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize};
use crate::components::managed_policy::{
    ManagedControl, allows_no_provider, disallowed_provider_message, is_managed_driver_allowed, managed_instance_locks,
    managed_notice,
};
use crate::components::provider_updates_action::ProviderUpdatesAction;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::switch::Switch;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::t;
use crate::provider_instances::{
    ProviderInstanceConfig as StoredInstance, ServerProvider, default_instance_id_for_driver, instance_enabled,
    normalize_provider_accent_color, provider_accent_foreground_color, provider_instance_initials,
    resolve_provider_instance_enabled,
};
use crate::theme_color::{parse_css_color, with_alpha};
use crate::timestamp_format::{format_relative_time, now_ms};

/// `PROVIDER_HEALTH_INTERVAL_STEP_SECONDS`.
pub const PROVIDER_HEALTH_INTERVAL_STEP_SECONDS: u64 = 30;
/// The Balanced profile's provider health interval, in seconds.
const DEFAULT_PROVIDER_HEALTH_SECONDS: u64 = 300;

/// One row of the instance list (`InstanceRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceRow {
    pub instance_id: String,
    /// `None` for a default slot never configured.
    pub explicit: Option<StoredInstance>,
    pub driver: String,
    pub is_default: bool,
    pub is_dirty: bool,
}

impl InstanceRow {
    fn instance(&self) -> StoredInstance {
        self.explicit.clone().unwrap_or_else(|| StoredInstance {
            driver: self.driver.clone(),
            display_name: None,
            accent_color: None,
            environment: None,
            enabled: None,
            config: None,
        })
    }
}

/// The list's rows: each driver's default slot (Codex and Claude always, others
/// once configured or enabled), then its custom instances, then instances of
/// drivers this build lacks. Cursor's default slot needs a Cursor snapshot.
pub fn instance_rows(
    settings: &t3code_contracts::ServerSettings,
    providers: &[Arc<ServerProvider>],
    target: Option<&str>,
) -> Vec<InstanceRow> {
    let instances = settings.provider_instances.clone().unwrap_or_default();
    let visible: Vec<&str> = DRIVERS
        .iter()
        .copied()
        .filter(|d| *d != "cursor" || providers.iter().any(|p| p.instance_id == "cursor"))
        .collect();
    let mut rows = Vec::new();
    for driver in &visible {
        let meta = driver_meta(driver);
        let default_id = default_instance_id_for_driver(driver);
        let explicit = instances.get(default_id).cloned();
        let dirty = explicit.is_some();
        let has_slot = meta.as_ref().is_none_or(|m| m.has_default_instance) || dirty;
        let enabled = explicit
            .as_ref()
            .map(instance_enabled)
            .unwrap_or_else(|| resolve_provider_instance_enabled(driver, None, None));
        if has_slot && (*driver == "codex" || *driver == "claudeAgent" || dirty || enabled || Some(default_id) == target) {
            rows.push(InstanceRow {
                instance_id: default_id.into(),
                explicit,
                driver: (*driver).into(),
                is_default: true,
                is_dirty: dirty,
            });
        }
        for (id, instance) in &instances {
            if instance.driver == *driver && id != default_id {
                rows.push(InstanceRow {
                    instance_id: id.clone(),
                    explicit: Some(instance.clone()),
                    driver: instance.driver.clone(),
                    is_default: false,
                    is_dirty: false,
                });
            }
        }
    }
    for (id, instance) in &instances {
        if !visible.contains(&instance.driver.as_str()) {
            rows.push(InstanceRow {
                instance_id: id.clone(),
                explicit: Some(instance.clone()),
                driver: instance.driver.clone(),
                is_default: visible.contains(&id.as_str()),
                is_dirty: false,
            });
        }
    }
    rows
}

/// The mark for a driver, or `None` to draw initials.
pub fn driver_brand(driver: &str) -> Option<BrandIcon> {
    Some(match driver {
        "codex" => BrandIcon::OpenAi,
        "claudeAgent" => BrandIcon::ClaudeAi,
        "cursor" => BrandIcon::Cursor,
        "grok" => BrandIcon::Grok,
        "antigravity" => BrandIcon::Antigravity,
        "kiro" => BrandIcon::Kiro,
        "acpRegistry" => BrandIcon::AcpRegistry,
        _ => return None,
    })
}

/// `ProviderInstanceIcon`: the driver mark (or initials) with an accent badge.
pub fn provider_instance_icon(driver: &str, name: &str, accent: Option<&str>, cx: &App) -> AnyElement {
    let theme = &cx.app_theme().clone();
    let c = &theme.colors;
    let mark = match driver_brand(driver) {
        Some(brand) => Icon::new(brand).size(IconSize::Md).color(with_alpha(c.foreground(), 0.8)).into_any_element(),
        None => div()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.radii.sm)
            .bg(c.muted)
            .ui_text(theme, TextSize::Xs2, weight::SEMIBOLD)
            .text_color(c.muted_foreground)
            .child(provider_instance_initials(name))
            .into_any_element(),
    };
    let badge = accent.and_then(|a| {
        let fg = provider_accent_foreground_color(a);
        Some((parse_css_color(a)?.hsla(), parse_css_color(fg)?.hsla()))
    });
    div()
        .relative()
        .flex_none()
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
        .children(badge.map(|(bg, _fg)| {
            div()
                .absolute()
                .right(px(-2.))
                .bottom(px(-2.))
                .size(px(9.))
                .rounded(theme.radii.full)
                .border_1()
                .border_color(c.background())
                .bg(bg)
        }))
        .into_any_element()
}

fn config_string(config: Option<&serde_json::Value>, key: &str) -> Option<String> {
    config?.as_object()?.get(key)?.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// What the panel asks before it removes or resets an instance.
#[derive(Clone, Debug)]
enum PendingConfirm {
    Remove { instance_id: String, name: String },
    Reset { driver: String, name: String, turns_off: bool },
}

/// Events the panel's parent may act on.
#[derive(Clone, Debug)]
pub enum ProviderSettingsEvent {
    /// The add dialog created an instance; it is now selected.
    Created(String),
}

/// The Providers page for one environment.
pub struct ProviderSettingsPanel {
    pub(super) model: Entity<ServerPanelsModel>,
    pub(super) selected: Option<String>,
    pub(super) fields: Fields,
    pub(super) add_dialog: Option<Entity<AddProviderInstanceDialog>>,
    confirm: Option<PendingConfirm>,
    updating: BTreeSet<String>,
    /// Per-instance sign-in UI state (chosen method, last error).
    pub(super) auth_ui: HashMap<String, super::provider_authentication_section::AuthUi>,
    /// One accent picker per instance, created on first render of its editor.
    accent_pickers: HashMap<String, Entity<ProviderAccentColorPicker>>,
    /// The page's content width, for the web's `@min-[48rem]/providers` query.
    width: f32,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProviderSettingsEvent> for ProviderSettingsPanel {}

impl ProviderSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, target: Option<String>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            selected: target,
            fields: Fields::default(),
            add_dialog: None,
            confirm: None,
            updating: BTreeSet::new(),
            auth_ui: HashMap::new(),
            accent_pickers: HashMap::new(),
            width: 0.,
            _subscriptions: vec![observe],
        }
    }

    pub fn model(&self) -> &Entity<ServerPanelsModel> {
        &self.model
    }

    /// Selects an instance in the list (`selectInstance`).
    pub fn select(&mut self, instance_id: &str, cx: &mut Context<Self>) {
        self.selected = Some(instance_id.to_owned());
        cx.notify();
    }

    /// Opens the add provider dialog.
    pub fn open_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let dialog = cx.new(|cx| AddProviderInstanceDialog::new(model, window, cx));
        let sub = cx.subscribe(&dialog, |this, _, event: &AddProviderInstanceEvent, cx| match event {
            AddProviderInstanceEvent::Close => {
                this.add_dialog = None;
                cx.notify();
            }
            AddProviderInstanceEvent::Created(id) => {
                this.add_dialog = None;
                this.selected = Some(id.clone());
                cx.emit(ProviderSettingsEvent::Created(id.clone()));
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.add_dialog = Some(dialog);
        cx.notify();
    }

    /// A text field the panel created, for scripted checks.
    pub fn field(&self, key: &str) -> Option<Entity<gpui_kit::component::input::InputState>> {
        self.fields.field(key)
    }

    pub fn add_dialog(&self) -> Option<&Entity<AddProviderInstanceDialog>> {
        self.add_dialog.as_ref()
    }

    fn rows(&self, cx: &App) -> Vec<InstanceRow> {
        let snapshot = self.model.read(cx).snapshot();
        instance_rows(&snapshot.settings, &snapshot.providers, self.selected.as_deref())
    }

    /// `persistInstance`: `server.updateSettings` with an upsert mutation.
    pub fn persist_instance(&mut self, instance_id: &str, next: StoredInstance, cx: &mut Context<Self>) {
        let input = ServerUpdateSettingsPayload {
            patch: ClientServerSettingsPatch::default(),
            provider_instance_mutation: Some(ProviderInstanceMutation::Upsert(ProviderInstanceMutationUpsert {
                instance_id: ProviderInstanceId::new(instance_id),
                instance: stored_to_config(&next),
            })),
        };
        self.model.update(cx, |model, cx| {
            model.run(
                format!("instance:{instance_id}"),
                move |s| s.update_settings(input),
                |result, cx| {
                    if let Err(error) = result {
                        toast_error(t!("Could not update provider instance"), error, cx);
                    }
                },
                cx,
            )
        });
    }

    fn remove_instance(&mut self, instance_id: String, reset: bool, cx: &mut Context<Self>) {
        let input = ServerUpdateSettingsPayload {
            patch: ClientServerSettingsPatch::default(),
            provider_instance_mutation: Some(ProviderInstanceMutation::Remove(ProviderInstanceMutationRemove {
                instance_id: ProviderInstanceId::new(instance_id.as_str()),
            })),
        };
        if !reset && self.selected.as_deref() == Some(&instance_id) {
            self.selected = None;
        }
        self.model.update(cx, |model, cx| {
            model.run(
                format!("instance:{instance_id}"),
                move |s| s.update_settings(input),
                move |result, cx| {
                    if let Err(error) = result {
                        let title = if reset { t!("Could not reset provider instance") } else { t!("Could not delete provider instance") };
                        toast_error(title, error, cx);
                    }
                },
                cx,
            )
        });
    }

    fn refresh_providers(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.run(
                "refresh-providers",
                |s| s.refresh_providers(ServerRefreshProvidersPayload {
                    refresh_models: Some(true),
                    reload_secrets: Some(true),
                    ..Default::default()
                }),
                |_, _| {},
                cx,
            )
        });
    }

    /// `runProviderUpdate`: `server.updateProvider`, once per instance at a time.
    pub fn run_provider_update(&mut self, driver: &str, instance_id: &str, target_version: Option<String>, cx: &mut Context<Self>) {
        if !self.updating.insert(instance_id.to_owned()) {
            return;
        }
        let input = ServerProviderUpdateInput {
            provider: ProviderDriverKind::new(driver),
            target_version,
            instance_id: Some(ProviderInstanceId::new(instance_id)),
        };
        let provider_name = crate::provider_instances::provider_display_name(driver).unwrap_or(driver).to_owned();
        let id = instance_id.to_owned();
        let this = cx.entity().downgrade();
        self.model.update(cx, |model, cx| {
            model.run(
                format!("update:{id}"),
                move |s| s.update_provider(input),
                move |result, cx| {
                    if let Err(error) = result {
                        toast_error(t!("Could not update {provider}", provider = provider_name), error, cx);
                    }
                    this.update(cx, |this, cx| {
                        this.updating.remove(&id);
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            )
        });
        cx.notify();
    }

    fn set_model_hidden(&mut self, instance_id: &str, model: &str, hidden: bool, cx: &mut Context<Self>) {
        let patch = ClientServerSettingsPatch {
            set_models_hidden: Some(vec![ClientServerSettingsPatchSetModelsHiddenItem {
                provider: ProviderInstanceId::new(instance_id),
                model: model.into(),
                hidden,
            }]),
            ..Default::default()
        };
        self.update_settings("model-prefs", patch, cx);
    }

    fn set_model_favorite(&mut self, instance_id: &str, model: &str, favorite: bool, cx: &mut Context<Self>) {
        let patch = ClientServerSettingsPatch {
            set_model_favorites: Some(vec![ClientServerSettingsPatchSetModelFavoritesItem {
                provider: ProviderInstanceId::new(instance_id),
                model: model.into(),
                favorite,
            }]),
            ..Default::default()
        };
        self.update_settings("model-prefs", patch, cx);
    }

    /// `updateSettings`: a plain settings patch.
    pub fn update_settings(&mut self, key: &'static str, patch: ClientServerSettingsPatch, cx: &mut Context<Self>) {
        let input = ServerUpdateSettingsPayload { patch, provider_instance_mutation: None };
        self.model.update(cx, |model, cx| {
            model.run(key, move |s| s.update_settings(input), |result, cx| {
                if let Err(error) = result {
                    toast_error(t!("Could not save settings"), error, cx);
                }
            }, cx)
        });
    }

    fn render_header_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let model = self.model.read(cx);
        let refreshing = model.is_pending("refresh-providers");
        let snapshot = model.snapshot();
        let last_checked = snapshot.providers.iter().map(|p| p.checked_at.as_str()).max().map(str::to_owned);
        let label = if refreshing {
            t!("Refreshing providers")
        } else {
            match last_checked.as_deref().and_then(|at| format_relative_time(at, now_ms())) {
                Some(parts) if parts.suffix.is_some() => t!("Checked {time} ago", time = parts.value),
                Some(parts) => t!("Checked {time}", time = parts.value),
                None if last_checked.is_some() => t!("Checked unavailable"),
                None => SharedString::default(),
            }
        };
        let updates = ProviderUpdatesAction::new(self.model.clone());
        div()
            .flex()
            .items_center()
            .gap(theme.sp(2.))
            .child(updates)
            .child(
                div().id("providers-refresh-tip").tooltip(tooltip(TooltipSurface::new(t!("Refresh provider status")))).child(
                    Button::new("providers-refresh")
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Xs)
                        .icon(IconName::RefreshCw)
                        .label(label)
                        .disabled(refreshing)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_providers(cx))),
                ),
            )
            .into_any_element()
    }

    fn render_list_row(&self, row: &InstanceRow, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let instance = row.instance();
        let meta = driver_meta(&row.driver);
        let live = snapshot.providers.iter().find(|p| p.instance_id == row.instance_id).cloned();
        let enabled = instance_enabled(&instance);
        let selected = self.selected.as_deref() == Some(row.instance_id.as_str());
        let name = instance
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| meta.as_ref().map_or(row.driver.clone(), |m| m.label.to_owned()));
        let accent = normalize_provider_accent_color(instance.accent_color.as_deref());
        let effective = live.as_ref().map(|p| {
            let mut p = (**p).clone();
            p.enabled = enabled;
            p
        });
        let status_key = if enabled { get_provider_status_key(effective.as_ref()) } else { ProviderStatusKey::Disabled };
        let summary = if enabled {
            get_provider_summary(effective.as_ref())
        } else {
            super::provider_status::ProviderSummary { headline: t!("Disabled", context = "provider status"), detail: None }
        };
        let needs_attention = matches!(status_key, ProviderStatusKey::Warning | ProviderStatusKey::Error);
        let updating = self.updating.contains(&row.instance_id)
            || live.as_ref().and_then(|p| p.update_state.as_ref()).is_some_and(|u| {
                matches!(u.status, ServerProviderUpdateStatus::Queued | ServerProviderUpdateStatus::Running)
            });
        let progress = updating.then(|| {
            live.as_ref()
                .and_then(|p| p.update_state.as_ref())
                .and_then(|u| u.message.clone())
                .map(SharedString::from)
                .unwrap_or_else(|| t!("Starting update"))
        });
        let version = get_provider_version_label(live.as_ref().and_then(|p| p.version.as_deref()));
        let advisory = live.as_ref().and_then(|p| get_provider_version_advisory_presentation(p, enabled));
        let compat = live.as_ref().filter(|_| enabled).and_then(|p| p.compatibility_advisory.as_ref());
        let inline_detail = match compat.map(|c| c.status.as_str()) {
            Some("broken") => Some(t!("Incompatible")),
            Some("unsupported") => Some(t!("Unsupported")),
            Some("graceful") => Some(t!("Limited support")),
            _ => summary.detail.clone(),
        };
        let status_line: SharedString = match progress {
            Some(progress) => t!("Updating · {progress}", progress = progress),
            None if needs_attention && inline_detail.is_some() => {
                format!("{} · {}", summary.headline, inline_detail.unwrap_or_default()).into()
            }
            None => summary.headline.clone(),
        };
        let read_only = snapshot.read_only;
        let driver_reason = (!is_managed_driver_allowed(snapshot.managed_policy.as_deref(), &row.driver)).then(|| {
            SharedString::from(disallowed_provider_message(
                snapshot.managed_policy.as_ref().map(|p| p.organization.as_str()),
                meta.as_ref().map_or(row.driver.as_str(), |m| m.label),
            ))
        });
        let id = row.instance_id.clone();
        let toggle_id = row.instance_id.clone();
        let toggle_instance = instance.clone();
        let this = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("provider-row-{}", row.instance_id)))
            .relative()
            .flex()
            .items_center()
            .gap(theme.sp(3.))
            .min_h(theme.sp(18.))
            .px(theme.sp(4.))
            .py(theme.sp(3.))
            .cursor_pointer()
            .when(selected, |d| d.bg(with_alpha(c.foreground(), 0.06)))
            .when(!selected, |d| d.hover(|d| d.bg(with_alpha(c.muted, 0.25))))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select(&id, cx)))
            .test_support()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .gap(theme.sp(3.))
                    .when(!enabled && !selected, |d| d.opacity(0.6))
                    .child(provider_instance_icon(&row.driver, &name, accent.as_deref(), cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.sp(2.))
                                    .min_w_0()
                                    .child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(name.clone()))
                                    .children(version.map(|v| {
                                        div().max_w(theme.sp(24.)).truncate().mono_text(px(12.), weight::NORMAL).text_color(c.muted_foreground).child(v)
                                    }))
                                    .children(advisory.map(|a| advisory_icon(a.emphasis, cx))),
                            )
                            .child(
                                div()
                                    .mt(theme.sp(0.5))
                                    .flex()
                                    .items_start()
                                    .gap(theme.sp(1.5))
                                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                    .text_color(with_alpha(c.muted_foreground, 0.8))
                                    .when(needs_attention && !updating, |d| {
                                        d.child(div().h(px(17.)).flex().items_center().child(
                                            div().size(px(6.)).rounded(theme.radii.full).bg(status_key.dot(theme)),
                                        ))
                                    })
                                    .child(div().line_clamp(2).child(status_line)),
                            ),
                    ),
            )
            .child(
                ManagedControl::new(
                    format!("provider-switch-lock-{}", row.instance_id),
                    if enabled { None } else { driver_reason },
                    Switch::new(SharedString::from(format!("provider-enable-{}", row.instance_id)), enabled)
                        .disabled(read_only)
                        .on_change(move |checked, _, cx| {
                            let mut next = toggle_instance.clone();
                            next.enabled = Some(checked);
                            let id = toggle_id.clone();
                            this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
                        }),
                ),
            )
            .into_any_element()
    }

    fn render_editor(&mut self, row: &InstanceRow, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let read_only = snapshot.read_only;
        let instance = row.instance();
        let meta = driver_meta(&row.driver);
        let live = snapshot.providers.iter().find(|p| p.instance_id == row.instance_id).cloned();
        let enabled = instance_enabled(&instance);
        let name = instance
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| meta.as_ref().map_or(row.driver.clone(), |m| m.label.to_owned()));
        let accent = normalize_provider_accent_color(instance.accent_color.as_deref());
        let locks = managed_instance_locks(snapshot.managed_policy.as_deref(), &row.driver);
        let version = get_provider_version_label(live.as_ref().and_then(|p| p.version.as_deref()));
        let advisory = live.as_ref().and_then(|p| get_provider_version_advisory_presentation(p, enabled));
        let iid = row.instance_id.clone();

        // Header: badge, version, advisory, reset or delete.
        let header_action = div()
            .flex()
            .items_center()
            .gap(theme.sp(1.5))
            .children(meta.as_ref().and_then(|m| m.badge_label.clone()).map(|b| Badge::new(b).variant(BadgeVariant::Warning).size(BadgeSize::Sm)))
            .children(version.clone().map(|v| div().mono_text(px(12.), weight::NORMAL).text_color(c.muted_foreground).child(v)))
            .children(advisory.clone().map(|a| {
                let driver = row.driver.clone();
                let id = row.instance_id.clone();
                let target = a.target_version.clone();
                let command = a.update_command.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.))
                    .child(advisory_icon(a.emphasis, cx))
                    .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(a.title.clone()))
                    .when(!read_only, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("provider-update-{id}")))
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Xs)
                                .label(if self.updating.contains(&id) {
                                    t!("Updating")
                                } else if let Some(v) = &target {
                                    t!("Install {version}", version = get_provider_version_label(Some(v)).unwrap_or_default())
                                } else {
                                    t!("Update now")
                                })
                                .disabled(self.updating.contains(&id))
                                .on_click(cx.listener(move |this, _, _, cx| this.run_provider_update(&driver, &id, target.clone(), cx))),
                        )
                    })
                    .children(command.map(|command| {
                        Button::new(SharedString::from(format!("provider-copy-update-{}", row.instance_id)))
                            .variant(ButtonVariant::GhostMuted)
                            .size(ButtonSize::IconXs)
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                                Toaster::push(
                                    ToastData::new(ToastKind::Success, t!("{provider} update command copied", provider = "Provider"))
                                        .description(t!("Run it in a terminal when you are ready to update.")),
                                    cx,
                                );
                            })
                    }))
            }))
            .when(row.is_default && row.is_dirty && !read_only, |d| {
                let driver = row.driver.clone();
                let name = name.clone();
                let turns_off = enabled && !resolve_provider_instance_enabled(&driver, None, None);
                d.child(reset_button(
                    format!("provider-reset-{iid}"),
                    t!("Reset {provider} provider settings to default", provider = meta.as_ref().map_or(driver.as_str(), |m| m.label)),
                    {
                        let this = cx.entity().downgrade();
                        move |_, cx| {
                            let pending = PendingConfirm::Reset { driver: driver.clone(), name: name.clone(), turns_off };
                            this.update(cx, |this, cx| {
                                this.confirm = Some(pending);
                                cx.notify();
                            })
                            .ok();
                        }
                    },
                ))
            })
            .when(!row.is_default && !read_only, |d| {
                let id = row.instance_id.clone();
                let name = name.clone();
                d.child(
                    Button::new(SharedString::from(format!("provider-delete-{id}")))
                        .variant(ButtonVariant::GhostDestructive)
                        .size(ButtonSize::IconXs)
                        .icon(IconName::Trash2)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm = Some(PendingConfirm::Remove { instance_id: id.clone(), name: name.clone() });
                            cx.notify();
                        })),
                )
            });

        // Status row.
        let effective = live.as_ref().map(|p| {
            let mut p = (**p).clone();
            p.enabled = enabled;
            p
        });
        let summary = if enabled {
            get_provider_summary(effective.as_ref())
        } else {
            super::provider_status::ProviderSummary { headline: t!("Disabled", context = "provider status"), detail: None }
        };
        let status_key = if enabled { get_provider_status_key(effective.as_ref()) } else { ProviderStatusKey::Disabled };
        let auth_email = live.as_ref().and_then(|p| p.auth.email.clone()).filter(|e| !e.trim().is_empty());
        let authenticated = enabled
            && live.as_ref().is_some_and(|p| p.auth.status == t3code_contracts::ServerProviderAuthStatus::Authenticated);
        let status_text: SharedString = if authenticated && let Some(email) = &auth_email {
            let label = live.as_ref().and_then(|p| provider_auth_label(p.auth.label.as_deref(), p.auth.r#type.as_deref()));
            match label {
                Some(label) => format!("{} · {label}", t!("Authenticated as {email}", email = email)).into(),
                None => t!("Authenticated as {email}", email = email),
            }
        } else {
            match &summary.detail {
                Some(detail) if matches!(status_key, ProviderStatusKey::Warning | ProviderStatusKey::Error) => {
                    format!("{} · {detail}", summary.headline).into()
                }
                _ => summary.headline.clone(),
            }
        };
        let update_problem = live
            .as_ref()
            .and_then(|p| p.update_state.as_ref())
            .filter(|u| matches!(u.status, ServerProviderUpdateStatus::Failed | ServerProviderUpdateStatus::Unchanged))
            .and_then(|u| u.message.clone());
        let url_action = live.as_ref().and_then(|p| p.auth.action.clone());
        let status_row = SettingsRow::new(t!("Status")).status(
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.sp(1.5))
                        .when(matches!(status_key, ProviderStatusKey::Warning | ProviderStatusKey::Error), |d| {
                            d.child(div().size(px(6.)).rounded(theme.radii.full).bg(status_key.dot(theme)))
                        })
                        .child(status_text),
                )
                .children(update_problem.map(|m| tinted(m, c.destructive(), cx)))
                .children(url_action.filter(|_| !read_only).map(|action| {
                    let url = action.url.clone();
                    let elicitation = action.elicitation_id.clone();
                    let id = row.instance_id.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(1.5))
                        .pt(theme.sp(1.))
                        .child(action.message.clone())
                        .child(div().mono_text(px(11.), weight::NORMAL).child(action.url.clone()))
                        .child(
                            Button::new(SharedString::from(format!("provider-url-auth-{id}")))
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Xs)
                                .icon(IconName::ExternalLink)
                                .label(t!("Continue authentication"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.open_url(&url);
                                    let input = t3code_contracts::AcpRegistryAcceptUrlAuthInput {
                                        instance_id: ProviderInstanceId::new(id.as_str()),
                                        elicitation_id: elicitation.clone(),
                                    };
                                    this.model.update(cx, |m, cx| {
                                        m.run(
                                            "acp-url-auth",
                                            move |s| s.accept_acp_registry_url_auth(input),
                                            |result, cx| match result {
                                                Ok(r) if !r.accepted => {
                                                    Toaster::push(
                                                        ToastData::new(ToastKind::Warning, t!("Authentication request expired"))
                                                            .description(t!("Refresh the provider and start the authentication flow again.")),
                                                        cx,
                                                    );
                                                }
                                                Err(error) => toast_error(t!("Could not continue authentication"), error, cx),
                                                _ => {}
                                            },
                                            cx,
                                        )
                                    });
                                })),
                        )
                })),
        );

        // Display name and accent: both save as soon as they change (no Save button).
        let name_instance = instance.clone();
        let name_id = row.instance_id.clone();
        let this = cx.entity().downgrade();
        let name_field = self.fields.draft(
            format!("{iid}:displayName"),
            instance.display_name.clone().unwrap_or_default(),
            meta.as_ref().map_or_else(|| t!("Instance label"), |m| SharedString::from(m.label)),
            false,
            window,
            cx,
            move |text, _, cx| {
                let name = text.trim().to_owned();
                let mut next = name_instance.clone();
                next.display_name = (!name.is_empty()).then_some(name);
                let id = name_id.clone();
                this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
            },
        );
        let picker = self
            .accent_pickers
            .entry(row.instance_id.clone())
            .or_insert_with(|| cx.new(|cx| ProviderAccentColorPicker::new(window, cx)))
            .clone();
        let accent_instance = instance.clone();
        let accent_id = row.instance_id.clone();
        let this = cx.entity().downgrade();
        picker.update(cx, |picker, cx| {
            picker.sync(name.clone().into(), accent.as_deref(), move |color, _, cx| {
                let mut next = accent_instance.clone();
                next.accent_color = (!color.is_empty()).then_some(color);
                let id = accent_id.clone();
                this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
            }, window, cx)
        });
        let display_row = SettingsRow::new(t!("Display name")).control(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(theme.sp(2.))
                .when(read_only, |d| d.opacity(0.5))
                .child(picker)
                .child(div().w(theme.sp(56.)).child(field_input(&name_field, false, false, read_only))),
        );

        let mut sections: Vec<AnyElement> = vec![detail_section(
            name.clone().into(),
            Some(provider_instance_icon(&row.driver, &name, accent.as_deref(), cx)),
            Some(header_action.into_any_element()),
            vec![status_row.into_any_element(), display_row.into_any_element()],
            cx,
        )];

        // Setup: sign-in and dedicated environment fields.
        let mut setup: Vec<AnyElement> = Vec::new();
        if let Some(provider) = live.as_ref().filter(|p| {
            !read_only
                && (p.setup.as_ref().is_some_and(|s| s.can_authenticate) || (p.driver == "acpRegistry" && p.installed))
        }) {
            setup.push(self.render_auth_section(provider.clone(), window, cx));
        } else if row.driver == "cursor" && !read_only && live.as_ref().and_then(|p| p.setup.as_ref()).is_some_and(|s| !s.can_authenticate) {
            setup.push(
                SettingsRow::new(t!("Cursor account"))
                    .description(t!("Using CURSOR_API_KEY. Remove it from this provider's environment to use browser sign-in."))
                    .into_any_element(),
            );
        }
        if let Some(meta) = &meta {
            for field in &meta.environment_fields {
                let value = instance
                    .environment
                    .iter()
                    .flatten()
                    .find(|v| v.name == field.name)
                    .and_then(|v| v.value.clone())
                    .unwrap_or_default();
                let commit_instance = instance.clone();
                let commit_id = row.instance_id.clone();
                let (var_name, sensitive) = (field.name, field.sensitive);
                let this = cx.entity().downgrade();
                let input = self.fields.draft(
                    format!("{iid}:env:{}", field.name),
                    value,
                    field.placeholder.clone().unwrap_or_default(),
                    field.sensitive,
                    window,
                    cx,
                    move |text, _, cx| {
                        let next = with_environment_value(&commit_instance, var_name, None, text.trim(), sensitive);
                        let id = commit_id.clone();
                        this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
                    },
                );
                let locked = locks.as_ref().filter(|l| l.whole_environment || l.environment_names.contains(field.name)).map(|l| l.reason.clone());
                let mut row_el = SettingsRow::new(field.label.clone())
                    .control(ManagedControl::new(format!("{iid}-envlock-{}", field.name), locked, div().w(theme.sp(56.)).child(field_input(&input, true, false, read_only))));
                if let Some(d) = &field.description {
                    row_el = row_el.description(d.clone());
                }
                setup.push(row_el.into_any_element());
            }
        }
        if !setup.is_empty() {
            let mut section = FlatSection::new(t!("Setup"));
            section.extend(setup);
            sections.push(section.into_any_element());
        }

        // Runtime: the driver's settings form.
        let form_fields = match &meta {
            Some(_) if row.driver == "acpRegistry" && config_string(instance.config.as_ref(), "source").as_deref() == Some("local") => local_acp_fields(),
            Some(m) => m.fields.clone(),
            None => Vec::new(),
        };
        match &meta {
            Some(meta) if !form_fields.is_empty() => {
                let mut section = FlatSection::new(t!("Runtime"));
                for field in &form_fields {
                    section.extend([self.render_form_field(row, &instance, meta, field, locks.as_ref(), read_only, window, cx)]);
                }
                sections.push(section.into_any_element());
            }
            None => {
                sections.push(
                    FlatSection::new(t!("Runtime"))
                        .child(SettingsRow::new(t!("Driver")).description(t!(
                            "This instance uses {driver}, which is not available in this build. Its configuration is preserved.",
                            driver = row.driver.clone()
                        )))
                        .into_any_element(),
                );
            }
            _ => {}
        }

        // Environment variables other than the dedicated fields.
        sections.push(self.render_environment_section(row, &instance, meta.as_ref(), locks.as_ref(), read_only, window, cx));

        // Models.
        if meta.is_some() {
            sections.push(self.render_models_section(row, live.as_deref(), &snapshot.settings, read_only, cx));
        }

        div().flex().flex_col().gap(theme.sp(6.)).py(theme.sp(4.)).children(sections).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_form_field(
        &mut self,
        row: &InstanceRow,
        instance: &StoredInstance,
        meta: &DriverMeta,
        field: &super::provider_driver_meta::FormField,
        locks: Option<&crate::components::managed_policy::ManagedInstanceLocks>,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let iid = row.instance_id.clone();
        let current = instance.config.as_ref().and_then(|c| c.get(field.key)).cloned();
        let lock = locks.filter(|l| l.config_keys.contains(field.key)).map(|l| l.reason.clone());
        let key = field.key;
        let base = instance.clone();
        let id = iid.clone();
        let this = cx.entity().downgrade();
        let set_value = move |value: Option<serde_json::Value>, cx: &mut App| {
            let mut next = base.clone();
            let mut config = next.config.clone().filter(|c| c.is_object()).unwrap_or_else(|| serde_json::json!({}));
            let map = config.as_object_mut().expect("object");
            match value {
                Some(v) => {
                    map.insert(key.to_owned(), v);
                }
                None => {
                    map.remove(key);
                }
            }
            next.config = (!map.is_empty()).then_some(config);
            let id = id.clone();
            this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
        };
        let control: AnyElement = match field.control {
            FieldControl::Switch => {
                let checked = current.as_ref().and_then(|v| v.as_bool()).or(field.default_bool).unwrap_or(false);
                Switch::new(SharedString::from(format!("{iid}-cfg-{key}")), checked)
                    .disabled(read_only || lock.is_some())
                    .on_change(move |v, _, cx| set_value(Some(serde_json::Value::Bool(v)), cx))
                    .into_any_element()
            }
            FieldControl::Select => {
                let options: Vec<SharedString> = field.options.iter().map(|o| o.label.clone()).collect();
                let values: Vec<&'static str> = field.options.iter().map(|o| o.value).collect();
                let selected = current.as_ref().and_then(|v| v.as_str()).and_then(|v| values.iter().position(|o| *o == v)).unwrap_or(0);
                return select_row(format!("{iid}-cfg-{key}"), field.label.clone(), field.description.clone(), options, selected, lock, move |ix, _, cx| {
                    set_value(values.get(ix).map(|v| serde_json::Value::String((*v).into())), cx)
                })
                .into_any_element();
            }
            _ => {
                let masked = field.control == FieldControl::Password;
                let shown = if field.secret { String::new() } else { current.as_ref().and_then(|v| v.as_str()).unwrap_or_default().to_owned() };
                let placeholder = if field.secret && current.is_some() { t!("Saved") } else { field.placeholder.clone().unwrap_or_default() };
                let clear_persist = field.clear_when_empty == super::provider_driver_meta::ClearWhenEmpty::Persist;
                let input = self.fields.draft(format!("{iid}:cfg:{key}"), shown, placeholder, masked, window, cx, move |text, _, cx| {
                    let text = text.trim().to_owned();
                    let value = if text.is_empty() {
                        clear_persist.then(|| serde_json::Value::String(String::new()))
                    } else {
                        Some(serde_json::Value::String(text))
                    };
                    set_value(value, cx);
                });
                div().w(theme.sp(56.)).child(field_input(&input, true, false, read_only || lock.is_some())).into_any_element()
            }
        };
        let _ = meta;
        let row_el = SettingsRow::new(field.label.clone()).control(ManagedControl::new(format!("{iid}-cfglock-{key}"), lock, control));
        match &field.description {
            Some(d) => row_el.description(d.clone()).into_any_element(),
            None => row_el.into_any_element(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_environment_section(
        &mut self,
        row: &InstanceRow,
        instance: &StoredInstance,
        meta: Option<&DriverMeta>,
        locks: Option<&crate::components::managed_policy::ManagedInstanceLocks>,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let iid = row.instance_id.clone();
        let dedicated: BTreeSet<&str> = meta.map(|m| m.environment_fields.iter().map(|f| f.name).collect()).unwrap_or_default();
        let variables: Vec<_> = instance.environment.iter().flatten().filter(|v| !dedicated.contains(v.name.as_str())).cloned().collect();
        let whole = locks.is_some_and(|l| l.whole_environment);
        let mut section = FlatSection::new(t!("Environment"));
        if let Some(l) = locks.filter(|l| l.whole_environment) {
            section.extend([div().px(theme.sp(4.)).py(theme.sp(3.)).child(managed_notice(l.reason.clone(), cx)).into_any_element()]);
        }
        let mut body = div().flex().flex_col().gap(theme.sp(2.)).when(whole || read_only, |d| d.opacity(0.5));
        for (ix, variable) in variables.iter().enumerate() {
            let sensitive = variable.sensitive == Some(true);
            let (rename_instance, rename_id, old_name) = (instance.clone(), iid.clone(), variable.name.clone());
            let this = cx.entity().downgrade();
            let name = self.fields.draft(format!("{iid}:var:{ix}:name"), variable.name.clone(), "VARIABLE_NAME", false, window, cx, move |text, _, cx| {
                let new_name = text.trim().to_owned();
                if new_name.is_empty() {
                    return;
                }
                let mut next = rename_instance.clone();
                if let Some(vars) = next.environment.as_mut() {
                    for v in vars.iter_mut().filter(|v| v.name == old_name) {
                        v.name = new_name.clone();
                    }
                }
                let id = rename_id.clone();
                this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
            });
            let (value_instance, value_id, var_name) = (instance.clone(), iid.clone(), variable.name.clone());
            let this = cx.entity().downgrade();
            let value = self.fields.draft(
                format!("{iid}:var:{ix}:value"),
                if sensitive { String::new() } else { variable.value.clone().unwrap_or_default() },
                if sensitive { t!("Sensitive, stored separately") } else { t!("value", context = "placeholder for an environment variable value") },
                sensitive,
                window,
                cx,
                move |text, _, cx| {
                    let next = with_environment_value(&value_instance, &var_name, Some(&var_name), &text, sensitive);
                    let id = value_id.clone();
                    this.update(cx, |this, cx| this.persist_instance(&id, next, cx)).ok();
                },
            );
            let remove_instance = instance.clone();
            let remove_name = variable.name.clone();
            let remove_id = iid.clone();
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(div().w(theme.sp(48.)).child(field_input(&name, true, false, read_only || whole)))
                    .child(div().flex_1().child(field_input(&value, true, false, read_only || whole)))
                    .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(
                        if variable.sensitive == Some(true) { t!("Sensitive, stored separately") } else { t!("Plain text") },
                    ))
                    .when(!read_only && !whole, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("{iid}-var-remove-{ix}")))
                                .variant(ButtonVariant::GhostMuted)
                                .size(ButtonSize::IconXs)
                                .icon(IconName::X)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let mut next = remove_instance.clone();
                                    next.environment = next.environment.map(|vars| vars.into_iter().filter(|v| v.name != remove_name).collect::<Vec<_>>()).filter(|v| !v.is_empty());
                                    this.fields.reset_prefix(&format!("{remove_id}:var:"));
                                    this.persist_instance(&remove_id, next, cx);
                                })),
                        )
                    }),
            );
        }
        let add_instance = instance.clone();
        let add_id = iid.clone();
        let header = SettingsRow::new(t!("Variables"))
            .description(t!("API keys, base URLs, and other per-instance CLI settings."))
            .control(
                Button::new(SharedString::from(format!("{iid}-var-add")))
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Xs)
                    .icon(IconName::Plus)
                    .label(t!("Add variable"))
                    .disabled(read_only || whole)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut next = add_instance.clone();
                        let mut vars = next.environment.clone().unwrap_or_default();
                        let mut n = 1;
                        while vars.iter().any(|v| v.name == format!("VARIABLE_{n}")) {
                            n += 1;
                        }
                        vars.push(t3code_contracts::ProviderInstanceEnvironmentInnerItem {
                            name: format!("VARIABLE_{n}"),
                            value: Some(String::new()),
                            sensitive: Some(false),
                            secret: None,
                            value_redacted: None,
                        });
                        next.environment = Some(vars);
                        this.fields.reset_prefix(&format!("{add_id}:var:"));
                        this.persist_instance(&add_id, next, cx);
                    })),
            );
        section.extend([
            header.into_any_element(),
            div()
                .px(theme.sp(4.))
                .pb(theme.sp(3.))
                .child(body)
                .child(hint(t!("Sensitive values are stored separately and never returned to the app."), cx))
                .into_any_element(),
        ]);
        section.into_any_element()
    }

    fn render_models_section(
        &self,
        row: &InstanceRow,
        live: Option<&ServerProvider>,
        settings: &t3code_contracts::ServerSettings,
        read_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let prefs = settings.provider_model_preferences.as_ref().and_then(|p| p.get(&row.instance_id));
        let hidden: BTreeSet<String> = prefs.and_then(|p| p.hidden_models.clone()).unwrap_or_default().into_iter().collect();
        let favorites: BTreeSet<String> = settings
            .favorites
            .iter()
            .flatten()
            .filter(|f| f.provider.as_str() == row.instance_id)
            .map(|f| f.model.clone())
            .collect();
        let models = live.map(|p| p.models.clone()).unwrap_or_default();
        let list = div().flex().flex_col().children(models.iter().map(|model| {
            let is_hidden = hidden.contains(&model.slug);
            let is_favorite = favorites.contains(&model.slug);
            let (hide_id, fav_id) = (row.instance_id.clone(), row.instance_id.clone());
            let (hide_slug, fav_slug) = (model.slug.clone(), model.slug.clone());
            div()
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .py(theme.sp(1.5))
                .border_b_1()
                .border_color(with_alpha(c.border, 0.4))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(theme.sp(2.))
                        .when(is_hidden, |d| d.opacity(0.5))
                        .child(div().truncate().ui_text(theme, TextSize::Sm, weight::NORMAL).child(model.name.clone()))
                        .child(div().truncate().mono_text(px(11.), weight::NORMAL).text_color(c.muted_foreground).child(model.slug.clone()))
                        .when(model.is_custom, |d| d.child(Badge::new(t!("Custom")).variant(BadgeVariant::Outline).size(BadgeSize::Sm))),
                )
                .child(
                    Button::new(SharedString::from(format!("model-fav-{}-{}", row.instance_id, model.slug)))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::IconXs)
                        .icon(IconName::Star)
                        .pressed(is_favorite)
                        .disabled(read_only)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_model_favorite(&fav_id, &fav_slug, !is_favorite, cx))),
                )
                .child(
                    Button::new(SharedString::from(format!("model-hide-{}-{}", row.instance_id, model.slug)))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::IconXs)
                        .icon(if is_hidden { IconName::EyeOff } else { IconName::Eye })
                        .disabled(read_only)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_model_hidden(&hide_id, &hide_slug, !is_hidden, cx))),
                )
        }));
        FlatSection::new(t!("Models"))
            .child(
                div()
                    .px(theme.sp(4.))
                    .py(theme.sp(3.))
                    .child(hint(
                        t!("Model preferences and custom models are saved on the selected environment and shared across connected devices."),
                        cx,
                    ))
                    .child(div().pt(theme.sp(3.)).child(if models.is_empty() {
                        hint(t!("No models"), cx)
                    } else {
                        list.into_any_element()
                    })),
            )
            .into_any_element()
    }

    fn render_usage_sources(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot();
        let sources = snapshot.settings.usage_limit_sources.clone().unwrap_or_default();
        let mut section = SettingsSection::new(t!("Usage providers")).header_action(
            Button::new("usage-add-hub")
                .variant(ButtonVariant::GhostMuted)
                .size(ButtonSize::Xs)
                .icon(IconName::Plus)
                .label(t!("Add hub"))
                .disabled(snapshot.read_only),
        );
        if sources.is_empty() {
            section.extend([SettingsRow::new(t!("No hubs configured.")).into_any_element()]);
        }
        for (id, source) in sources {
            let value = serde_json::to_value(&source).unwrap_or_default();
            let label = value.get("label").and_then(|v| v.as_str()).unwrap_or(&id).to_owned();
            let enabled = value.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            section.extend([SettingsRow::new(if enabled { label.into() } else { t!("{name} · Disabled", name = label) })
                .description(SharedString::from(value.get("baseUrl").and_then(|v| v.as_str()).unwrap_or_default().to_owned()))
                .control(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Remove")))
                .into_any_element()]);
        }
        section.into_any_element()
    }

    fn render_text_generators(&self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.model.read(cx).snapshot();
        let generators = snapshot.settings.text_generators.clone().unwrap_or_default();
        let mut section = SettingsSection::new(t!("Text generators")).header_action(
            Button::new("text-generator-add")
                .variant(ButtonVariant::GhostMuted)
                .size(ButtonSize::Xs)
                .icon(IconName::Plus)
                .label(t!("Add generator"))
                .disabled(snapshot.read_only),
        );
        if generators.is_empty() {
            section.extend([SettingsRow::new(t!("No text generators yet")).into_any_element()]);
        }
        for (id, config) in generators {
            let value = serde_json::to_value(&config).unwrap_or_default();
            let name = value.get("name").and_then(|v| v.as_str()).unwrap_or(&id).to_owned();
            let enabled = value.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let base = value.get("baseUrl").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
            section.extend([SettingsRow::new(SharedString::from(name.clone()))
                .description(SharedString::from(format!("{}{base}", if enabled { "" } else { "Off · " })))
                .control(Switch::new(SharedString::from(format!("text-generator-{id}")), enabled).disabled(snapshot.read_only))
                .into_any_element()]);
        }
        section.into_any_element()
    }

    fn render_advanced(&self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.model.read(cx).snapshot();
        let read_only = snapshot.read_only;
        let seconds = health_interval_seconds(&snapshot.settings);
        let idle = snapshot
            .settings
            .idle_compaction
            .map(|m| m.as_str())
            .unwrap_or("onReturn");
        let idle_options = [("onReturn", t!("When you return")), ("beforeCacheExpires", t!("Before the cache expires (beta)")), ("off", t!("Never", context = "idle thread compaction mode"))];
        let idle_ix = idle_options.iter().position(|(v, _)| *v == idle).unwrap_or(0);
        let this = cx.entity().downgrade();
        let this2 = cx.entity().downgrade();
        let row = SettingsRow::new(t!("Health check interval"))
            .description(t!("Refresh provider status, versions, and models in the background. Set to 0 to disable."))
            .control(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(seconds != DEFAULT_PROVIDER_HEALTH_SECONDS && !read_only, |d| {
                        let this = this.clone();
                        d.child(reset_button("provider-health-reset", t!("Reset provider health check interval to default"), move |_, cx| {
                            this.update(cx, |this, cx| this.set_health_interval(None, cx)).ok();
                        }))
                    })
                    .child(interval_seconds_field("provider-health", seconds, 0, PROVIDER_HEALTH_INTERVAL_STEP_SECONDS, read_only, move |s, _, cx| {
                        this2.update(cx, |this, cx| this.set_health_interval(Some(s), cx)).ok();
                    }, cx))
                    .child(hint(t!("seconds"), cx)),
            );
        let this = cx.entity().downgrade();
        let idle_row = select_row(
            "compact-idle-threads",
            t!("Compact idle threads"),
            Some(t!("When to compact an idle Claude thread over 200,000 tokens: after you return, just before its prompt cache expires, or never. Each Claude provider can override this.")),
            idle_options.iter().map(|(_, l)| l.clone()).collect(),
            idle_ix,
            read_only.then(|| t!("This connection lacks permission to change settings on the selected environment.")),
            move |ix, _, cx| {
                let value = ["onReturn", "beforeCacheExpires", "off"][ix];
                this.update(cx, |this, cx| {
                    let mut patch = ClientServerSettingsPatch::default();
                    patch.idle_compaction = t3code_contracts::IdleCompactionMode::from_wire(value);
                    this.update_settings("idle-compaction", patch, cx);
                })
                .ok();
            },
        );
        SettingsSection::new(t!("Advanced")).child(row).child(idle_row).into_any_element()
    }

    fn set_health_interval(&mut self, seconds: Option<u64>, cx: &mut Context<Self>) {
        let mut patch = ClientServerSettingsPatch::default();
        let mut overrides = serde_json::Map::new();
        overrides.insert(
            "providerHealthRefreshInterval".into(),
            seconds.map_or(serde_json::Value::Null, |s| serde_json::json!(s * 1000)),
        );
        patch.background_activity = serde_json::from_value(serde_json::Value::Object(overrides)).ok();
        self.update_settings("provider-health", patch, cx);
    }

    fn render_confirm(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.confirm.clone()?;
        let (title, consequence, action) = match &pending {
            PendingConfirm::Remove { name, .. } => (t!("Remove the {provider} provider instance?", provider = name.clone()), None, t!("Remove")),
            PendingConfirm::Reset { name, turns_off, .. } => (
                t!("Reset the {provider} provider to its defaults?", provider = name.clone()),
                turns_off.then(|| t!("It will also be turned off.")),
                t!("Reset"),
            ),
        };
        let description: SharedString = match consequence {
            Some(c) => format!("{} {c}", t!("Its saved settings, including any custom binary path, will be removed.")).into(),
            None => t!("Its saved settings, including any custom binary path, will be removed."),
        };
        let surface = DialogSurface::new(title)
            .description(description)
            .footer(Button::new("provider-confirm-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                this.confirm = None;
                cx.notify();
            })))
            .footer(Button::new("provider-confirm-ok").variant(ButtonVariant::Destructive).label(action).on_click(cx.listener(move |this, _, _, cx| {
                match this.confirm.take() {
                    Some(PendingConfirm::Remove { instance_id, .. }) => this.remove_instance(instance_id, false, cx),
                    Some(PendingConfirm::Reset { driver, .. }) => this.remove_instance(driver, true, cx),
                    None => {}
                }
                cx.notify();
            })));
        let this = cx.entity().downgrade();
        Some(
            Modal::dialog("provider-confirm", surface, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.confirm = None;
                    cx.notify();
                })
                .ok();
            })
            .into_any_element(),
        )
    }
}

/// A `SettingsSection` inside the provider detail pane. The web flattens
/// these (`[data-provider-detail] [data-slot=settings-group]` in `index.css`):
/// no border, fill or shadow, only the heading and the row dividers.
#[derive(IntoElement)]
pub struct FlatSection {
    title: SharedString,
    icon: Option<AnyElement>,
    header_action: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl FlatSection {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), icon: None, header_action: None, children: Vec::new() }
    }

    pub fn icon(mut self, icon: AnyElement) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn header_action(mut self, action: impl IntoElement) -> Self {
        self.header_action = Some(action.into_any_element());
        self
    }
}

impl ParentElement for FlatSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl gpui_kit::RenderOnce for FlatSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let divider = with_alpha(c.border, 0.5);
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.5))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(theme.sp(4.))
                    .min_h(theme.sp(7.))
                    .px(theme.sp(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .min_h(theme.sp(7.))
                            .ui_text(theme, TextSize::Base, weight::SEMIBOLD)
                            .text_color(c.foreground())
                            .children(self.icon)
                            .child(self.title),
                    )
                    .children(self.header_action.map(|action| {
                        div().flex().items_center().justify_end().min_h(theme.sp(7.)).min_w(theme.sp(7.)).child(action)
                    })),
            )
            .child(div().flex().flex_col().text_color(c.foreground()).children(self.children.into_iter().enumerate().map(|(ix, child)| {
                div().when(ix > 0, |d| d.border_t_1().border_color(divider)).child(child).into_any_element()
            })))
    }
}

fn detail_section(
    title: SharedString,
    icon: Option<AnyElement>,
    header_action: Option<AnyElement>,
    children: Vec<AnyElement>,
    _cx: &App,
) -> AnyElement {
    let mut section = FlatSection::new(title);
    if let Some(icon) = icon {
        section = section.icon(icon);
    }
    if let Some(action) = header_action {
        section = section.header_action(action);
    }
    section.extend(children);
    section.into_any_element()
}

/// The instance with variable `name` set to `value`; an empty value removes
/// a dedicated field's variable (`nextProviderEnvironmentWithFieldValue`).
fn with_environment_value(instance: &StoredInstance, name: &str, existing: Option<&str>, value: &str, sensitive: bool) -> StoredInstance {
    let mut next = instance.clone();
    let mut vars = next.environment.clone().unwrap_or_default();
    let target = existing.unwrap_or(name);
    match vars.iter_mut().find(|v| v.name == target) {
        Some(v) if existing.is_none() && value.is_empty() => {
            let _ = v;
            vars.retain(|v| v.name != target);
        }
        Some(v) => {
            v.value = Some(value.to_owned());
            v.value_redacted = None;
        }
        None if value.is_empty() => {}
        None => vars.push(t3code_contracts::ProviderInstanceEnvironmentInnerItem {
            name: name.to_owned(),
            value: Some(value.to_owned()),
            sensitive: Some(sensitive),
            secret: None,
            value_redacted: None,
        }),
    }
    next.environment = (!vars.is_empty()).then_some(vars);
    next
}

/// The provider health interval in effect, in seconds.
pub fn health_interval_seconds(settings: &t3code_contracts::ServerSettings) -> u64 {
    let value = settings
        .background_activity
        .as_ref()
        .and_then(|b| serde_json::to_value(b).ok())
        .and_then(|v| v.get("providerHealthRefreshInterval").cloned());
    match value {
        Some(serde_json::Value::Number(ms)) => ms.as_f64().map_or(DEFAULT_PROVIDER_HEALTH_SECONDS, |ms| (ms / 1000.).round() as u64),
        _ => DEFAULT_PROVIDER_HEALTH_SECONDS,
    }
}

fn advisory_icon(emphasis: AdvisoryEmphasis, cx: &App) -> AnyElement {
    let theme = &cx.app_theme().clone();
    let (icon, color) = match emphasis {
        AdvisoryEmphasis::Strong => (IconName::TriangleAlert, theme.colors.warning),
        AdvisoryEmphasis::Normal => (IconName::CircleArrowUp, theme.fixed.info_foreground),
    };
    Icon::new(icon).size(IconSize::Sm).color(color).into_any_element()
}

/// Shows an action's failure as an error toast.
pub fn toast_error(title: SharedString, error: RpcFailure, cx: &mut App) {
    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.message), cx);
}

/// A stored instance in the shape `ProviderInstanceMutation` carries.
pub fn stored_to_config(instance: &StoredInstance) -> ProviderInstanceConfig {
    serde_json::from_value(serde_json::to_value(instance).expect("instance encode")).expect("instance shape")
}

impl Render for ProviderSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let rows = self.rows(cx);
        if self.selected.as_ref().is_none_or(|s| !rows.iter().any(|r| &r.instance_id == s)) && self.selected.is_none() {
            self.selected = rows.first().map(|r| r.instance_id.clone());
        }
        let can_add = !snapshot.read_only && !allows_no_provider(snapshot.managed_policy.as_deref());
        let selected_row = rows.iter().find(|r| Some(&r.instance_id) == self.selected.as_ref()).cloned();
        // `@min-[48rem]/providers`: list and detail side by side once the page is
        // 48rem wide, otherwise the detail sits under the list.
        let wide = self.width >= f32::from(theme.rem(48.));
        let list_rows = div()
            .flex()
            .flex_col()
            .children(rows.iter().enumerate().map(|(ix, row)| {
                div().when(ix > 0, |d| d.border_t_1().border_color(with_alpha(c.border, 0.5))).child(self.render_list_row(row, cx))
            }))
            .when(can_add, |d| {
                d.child(
                    div()
                        .id("provider-list-add")
                        .flex()
                        .items_center()
                        .gap(theme.sp(3.))
                        .px(theme.sp(4.))
                        .py(theme.sp(3.))
                        .border_t_1()
                        .border_color(with_alpha(c.border, 0.5))
                        .cursor_pointer()
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .hover(|d| d.bg(with_alpha(c.muted, 0.25)).text_color(c.foreground()))
                        .on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(window, cx)))
                        .child(Icon::new(IconName::Plus).size(IconSize::Md).color(c.muted_foreground))
                        .child(t!("Add provider")),
                )
            });
        let editor = match &selected_row {
            Some(row) => self.render_editor(row, window, cx),
            None => div()
                .p(theme.sp(6.))
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(t!("No providers configured."))
                .into_any_element(),
        };
        let card = div()
            .overflow_hidden()
            .rounded(theme.radii.xl)
            .border_1()
            .border_color(with_alpha(c.border, 0.6))
            .bg(with_alpha(c.card(), 0.4))
            .shadow(theme.shadows.xs5.clone());
        let card = if wide {
            // `h-[min(44rem,calc(100dvh-11rem))] min-h-[32rem]`.
            let height = theme.rem(44.).min(window.viewport_size().height - theme.rem(11.)).max(theme.rem(32.));
            card.flex()
                .h(height)
                .child(
                    div()
                        .id("provider-list")
                        .flex_none()
                        .w(theme.rem(17.))
                        .h_full()
                        .overflow_y_scroll()
                        .border_r_1()
                        .border_color(with_alpha(c.border, 0.6))
                        .bg(with_alpha(c.muted, 0.1))
                        .child(list_rows),
                )
                .child(div().id("provider-detail").flex_1().min_w_0().h_full().overflow_y_scroll().child(editor))
        } else {
            card.flex()
                .flex_col()
                .child(div().border_b_1().border_color(with_alpha(c.border, 0.6)).bg(with_alpha(c.muted, 0.1)).child(list_rows))
                .child(div().min_w_0().child(editor))
        };
        let panel = cx.entity();
        let measure = gpui_kit::canvas(
            move |bounds, _, cx| {
                let width = f32::from(bounds.size.width);
                panel.update(cx, |panel, cx| {
                    if (panel.width - width).abs() > 0.5 {
                        panel.width = width;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let title_row = div()
            .flex()
            .items_center()
            .justify_between()
            .px(theme.sp(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.))
                    .ui_text(theme, TextSize::Base, weight::SEMIBOLD)
                    .child(t!("Providers"))
                    .when(can_add, |d| {
                        d.child(
                            div().id("providers-add-tip").tooltip(tooltip(TooltipSurface::new(t!("Add provider")))).child(
                                Button::new("providers-add")
                                    .variant(ButtonVariant::GhostMuted)
                                    .size(ButtonSize::IconXs)
                                    .icon(IconName::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(window, cx))),
                            ),
                        )
                    }),
            )
            .child(self.render_header_actions(cx));
        let read_only_note = snapshot.read_only.then(|| {
            SettingsSection::new(SharedString::default()).child(SettingsRow::new(t!("Limited permissions")).description(t!(
                "This session can view {environment}''s providers but can't change their settings.",
                environment = snapshot.environment_label.clone()
            )))
        });
        let mut page = page_container(cx)
            .gap(theme.sp(8.))
            .child(div().relative().flex().flex_col().gap(theme.sp(2.5)).child(measure).child(title_row).children(read_only_note).child(card))
            .child(self.render_usage_sources(cx));
        let allowed_drivers_policy = snapshot
            .managed_policy
            .as_ref()
            .and_then(|p| p.limits.as_ref()?.providers.as_ref()?.allowed_drivers.as_ref())
            .is_some();
        if !allowed_drivers_policy {
            page = page.child(self.render_text_generators(cx));
        }
        page = page.child(self.render_advanced(cx));
        let confirm = self.render_confirm(cx);
        div()
            .relative()
            .size_full()
            .child(page)
            .children(self.add_dialog.clone().map(|dialog| {
                let this = cx.entity().downgrade();
                Modal::dialog("add-provider-instance", dialog, move |_, cx| {
                    this.update(cx, |this, cx| {
                        this.add_dialog = None;
                        cx.notify();
                    })
                    .ok();
                })
            }))
            .children(confirm)
    }
}
