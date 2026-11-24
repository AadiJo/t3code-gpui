//! The Publish repository dialog. Ports `PublishRepositoryDialog` of
//! `GitActionsControl.tsx` and the wizard parts of `ui/wizard.tsx`: three steps
//! (Provider, Repository, Summary) in a stepper, provider cards sorted ready
//! first, the repository path behind the host, visibility cards, an Advanced
//! disclosure with the remote name and protocol, and the published result.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, Hsla, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, TestSupportExt as _, Window, component::input::InputEvent,
    component::input::{Input, InputState}, div, prelude::*, px,
};
use t3code_contracts::{
    SourceControlCloneProtocol, SourceControlDiscoveryResult, SourceControlDiscoveryStatus,
    SourceControlProviderAuthAccount, SourceControlProviderAuthHost, SourceControlProviderAuthStatus,
    SourceControlProviderDiscoveryItem, SourceControlProviderKind, SourceControlPublishRepositoryInput,
    SourceControlPublishRepositoryResult, SourceControlPublishStatus, SourceControlRepositoryVisibility,
};

use super::git_actions_control_source::GitActionsSource;
use super::icons::{Icon, IconName, IconSize};
use super::ui::button::{Button, ButtonSize, ButtonVariant};
use super::ui::dialog::{Modal, dialog_glass};
use super::ui::input::TextInput;
use super::ui::spinner::{Spinner, SpinnerSize};
use super::ui::toggle::Toggle;
use super::ui::tooltip::tooltip;
use super::ui::TextStyleExt as _;
use crate::appearance_fonts::{TextSize, mono_font, weight};
use crate::i18n::{t, t_rich};
use crate::source_control_presentation::{SOURCE_CONTROL_DEFINITIONS, SourceControlDefinition, source_control_icon};
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

/// What the dialog asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum PublishRepositoryEvent {
    /// Closed, by Cancel, Done, Escape or a press outside.
    Close,
    /// "Setup Required": close and open Settings > Source control & issues.
    OpenSourceControlSettings,
    /// "Open on {provider}".
    OpenUrl(String),
    /// The repository was published; the checkout's status should refresh.
    Published,
}

/// Whether a host can publish now (`getPublishProviderReadiness`).
#[derive(Clone, Debug, PartialEq)]
struct Readiness {
    ready: bool,
    hint: Option<SharedString>,
}

fn readiness(kind: &str, discovered: &[SourceControlProviderDiscoveryItem]) -> Readiness {
    let Some(item) = discovered.iter().find(|p| p.kind.as_str() == kind) else {
        return Readiness {
            ready: false,
            hint: Some(t!("Provider status unavailable. Open Settings -> Source control & issues and rescan.")),
        };
    };
    if item.status != SourceControlDiscoveryStatus::Available {
        return Readiness { ready: false, hint: Some(item.install_hint.clone().into()) };
    }
    if item.auth.status == SourceControlProviderAuthStatus::Unauthenticated {
        let detail = match &item.auth.detail {
            t3code_contracts::SourceControlProviderAuthDetail::SomeType(d) => Some(SharedString::from(d.value.clone())),
            _ => None,
        };
        return Readiness {
            ready: false,
            hint: Some(detail.unwrap_or_else(|| {
                t!(
                    "{provider} is not authenticated. Open Settings -> Source control & issues for setup guidance.",
                    provider = item.label.as_str()
                )
            })),
        };
    }
    Readiness { ready: true, hint: None }
}

fn account(item: Option<&SourceControlProviderDiscoveryItem>) -> Option<String> {
    match &item?.auth.account {
        SourceControlProviderAuthAccount::SomeType(a) => Some(a.value.clone()),
        _ => None,
    }
}

fn signed_in_host(item: Option<&SourceControlProviderDiscoveryItem>) -> Option<String> {
    match &item?.auth.host {
        SourceControlProviderAuthHost::SomeType(h) => Some(h.value.clone()),
        _ => None,
    }
}

/// The dialog's state between opens of one checkout.
pub struct PublishRepositoryDialog {
    source: Arc<dyn GitActionsSource>,
    cwd: SharedString,
    /// `offeredSourceControlProviders`: hosts the server offers; unset offers GitHub only.
    offered: Option<std::collections::BTreeMap<String, bool>>,
    discovery: Option<SourceControlDiscoveryResult>,
    selected: Option<&'static str>,
    repository: Entity<InputState>,
    /// The path was edited; until then it follows the account prefill.
    repository_edited: bool,
    visibility: SourceControlRepositoryVisibility,
    remote_name: Entity<InputState>,
    protocol: SourceControlCloneProtocol,
    step: usize,
    advanced_open: bool,
    pending: bool,
    error: Option<SharedString>,
    result: Option<SourceControlPublishRepositoryResult>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PublishRepositoryEvent> for PublishRepositoryDialog {}

impl PublishRepositoryDialog {
    pub fn new(
        source: Arc<dyn GitActionsSource>,
        cwd: SharedString,
        offered: Option<std::collections::BTreeMap<String, bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repository = cx.new(|cx| InputState::new(window, cx));
        let remote_name = cx.new(|cx| InputState::new(window, cx).placeholder("origin").default_value("origin"));
        let subscriptions = vec![
            cx.subscribe_in(&repository, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.repository_edited = true;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                _ => {}
            }),
            cx.observe(&remote_name, |_, _, cx| cx.notify()),
        ];
        let request = source.source_control_discovery();
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                // A failed discovery leaves every host "Provider status unavailable".
                this.discovery = Some(result.unwrap_or(SourceControlDiscoveryResult {
                    version_control_systems: Vec::new(),
                    source_control_providers: Vec::new(),
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            source,
            cwd,
            offered,
            discovery: None,
            selected: None,
            repository,
            repository_edited: false,
            visibility: SourceControlRepositoryVisibility::Private,
            remote_name,
            protocol: SourceControlCloneProtocol::Ssh,
            step: 0,
            advanced_open: false,
            pending: false,
            error: None,
            result: None,
            _subscriptions: subscriptions,
        }
    }

    fn discovered(&self) -> &[SourceControlProviderDiscoveryItem] {
        self.discovery.as_ref().map_or(&[], |d| d.source_control_providers.as_slice())
    }

    /// `isSourceControlProviderOffered`.
    fn offered(&self, kind: &str) -> bool {
        self.offered.as_ref().and_then(|o| o.get(kind).copied()).unwrap_or(kind == SOURCE_CONTROL_DEFINITIONS[0].kind)
    }

    /// Offered hosts, ready ones first, then by label.
    fn options(&self) -> Vec<(SourceControlDefinition, Readiness)> {
        let mut options: Vec<_> = SOURCE_CONTROL_DEFINITIONS
            .iter()
            .filter(|d| self.offered(d.kind))
            .map(|d| (*d, readiness(d.kind, self.discovered())))
            .collect();
        options.sort_by(|a, b| b.1.ready.cmp(&a.1.ready).then_with(|| a.0.picker_label.cmp(b.0.picker_label)));
        options
    }

    /// The host the dialog publishes to: the pick if it is ready, else the first ready host.
    fn provider(&self) -> (SourceControlDefinition, Readiness) {
        let options = self.options();
        let selected = self.selected.and_then(|kind| options.iter().find(|(d, _)| d.kind == kind));
        selected
            .filter(|(_, r)| r.ready)
            .or_else(|| options.iter().find(|(_, r)| r.ready))
            .or(selected)
            .cloned()
            .unwrap_or_else(|| (SOURCE_CONTROL_DEFINITIONS[0], readiness(SOURCE_CONTROL_DEFINITIONS[0].kind, self.discovered())))
    }

    fn discovered_item(&self, kind: &str) -> Option<&SourceControlProviderDiscoveryItem> {
        self.discovered().iter().find(|p| p.kind.as_str() == kind)
    }

    /// The path as typed, or `account/` until the field is edited.
    fn repository(&self, cx: &gpui_kit::App) -> String {
        if self.repository_edited {
            return self.repository.read(cx).value().to_string();
        }
        let (definition, _) = self.provider();
        // Bitbucket and Azure DevOps accounts do not name a workspace or project.
        let owner_from_account = !matches!(definition.kind, "bitbucket" | "azure-devops");
        account(self.discovered_item(definition.kind)).filter(|_| owner_from_account).map(|a| format!("{a}/")).unwrap_or_default()
    }

    fn can_submit(&self, cx: &gpui_kit::App) -> bool {
        let (_, ready) = self.provider();
        let path = self.repository(cx);
        let path = path.trim();
        let (owner, name) = path.split_once('/').unwrap_or((path, ""));
        ready.ready && !self.pending && !owner.trim().is_empty() && !name.trim().is_empty()
    }

    /// Puts the account prefill and the host's path hint in the repository field.
    fn sync_repository_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hint = self.provider().0.path_hint;
        let prefill = self.repository(cx);
        let edited = self.repository_edited;
        self.repository.update(cx, |input, cx| {
            input.set_placeholder(hint, window, cx);
            if !edited {
                input.set_value(prefill, window, cx);
            }
        });
        self.repository_edited = edited;
    }

    fn select(&mut self, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(kind);
        self.repository_edited = false;
        self.sync_repository_field(window, cx);
        cx.notify();
    }

    fn set_step(&mut self, step: usize, window: &mut Window, cx: &mut Context<Self>) {
        if step == 1 {
            self.sync_repository_field(window, cx);
        }
        self.step = step;
        cx.notify();
    }

    fn submit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        self.error = None;
        self.pending = true;
        let (definition, _) = self.provider();
        let remote = self.remote_name.read(cx).value().trim().to_string();
        let request = self.source.publish_repository(SourceControlPublishRepositoryInput {
            cwd: self.cwd.to_string(),
            provider: SourceControlProviderKind::from(definition.kind),
            repository: self.repository(cx).trim().to_string(),
            visibility: self.visibility,
            remote_name: Some(if remote.is_empty() { "origin".into() } else { remote }),
            protocol: Some(self.protocol),
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.pending = false;
                match result {
                    Ok(result) => {
                        this.result = Some(result);
                        this.step = 2;
                        cx.emit(PublishRepositoryEvent::Published);
                    }
                    Err(error) => {
                        this.error = Some(if error.0.is_empty() { t!("An error occurred.") } else { error.0 });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PublishRepositoryEvent::Close);
    }

    /// `WizardSteps`: a segmented row of numbered steps; done steps show a check
    /// and their summary, and only earlier steps are clickable.
    fn render_steps(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let dark = theme.is_dark();
        let steps = [t!("Provider"), t!("Repository"), t!("Summary")];
        let summaries: [Option<SharedString>; 3] = [
            Some(self.provider().0.picker_label.into()),
            self.result.as_ref().map(|r| r.repository.name_with_owner.clone().into()),
            None,
        ];
        let card = c.card();
        let ring = if dark { gpui_kit::hsla(0., 0., 1., 0.05) } else { gpui_kit::hsla(0., 0., 0., 0.05) };
        div()
            .flex()
            .gap(theme.rem(0.25))
            .p(theme.rem(0.25))
            .rounded(theme.radii.xl)
            .bg(if dark { gpui_kit::hsla(0., 0., 1., 0.04) } else { c.background() })
            .border_1()
            .border_color(ring)
            .children(steps.into_iter().enumerate().map(|(index, step)| {
                let current = index == self.step;
                let done = index < self.step;
                let disabled = self.step == 2 || index >= 2 || index > self.step;
                let badge_fill: Hsla = if done {
                    c.primary()
                } else if current {
                    with_alpha(c.primary(), 0.1)
                } else if dark {
                    gpui_kit::hsla(0., 0., 1., 0.05)
                } else {
                    card
                };
                let badge_ring: Hsla = if done {
                    c.primary()
                } else if current {
                    with_alpha(c.primary(), 0.3)
                } else if dark {
                    gpui_kit::hsla(0., 0., 1., 0.1)
                } else {
                    gpui_kit::hsla(0., 0., 0., 0.1)
                };
                let badge_text = if done { c.primary_foreground() } else if current { c.primary() } else { c.muted_foreground };
                let label: SharedString = match (&summaries[index], done) {
                    (Some(summary), true) => format!("{step}: {summary}").into(),
                    _ => step,
                };
                div()
                    .id(("publish-step", index))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.rem(0.5))
                    .px(theme.rem(0.625))
                    .py(theme.rem(0.5))
                    .rounded(theme.radii.lg)
                    .when(current, |s| s.bg(card).border_1().border_color(ring))
                    .when(!disabled && !current, |s| s.cursor_pointer().hover(move |s| s.bg(card)))
                    .when(!disabled, |s| s.on_click(cx.listener(move |this, _, window, cx| this.set_step(index, window, cx))))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .size(theme.rem(1.25))
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(badge_fill)
                            .border_1()
                            .border_color(badge_ring)
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(badge_text)
                            .map(|b| {
                                if done {
                                    b.child(Icon::new(IconName::Check).size(IconSize::Custom(theme.rem(1.))).color(badge_text))
                                } else {
                                    b.child((index + 1).to_string())
                                }
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(if current { c.foreground() } else { c.muted_foreground })
                            .child(label),
                    )
            }))
            .into_any_element()
    }

    /// A radio card: selected cards get the primary ring, others a faint fill.
    fn card(theme: &AppTheme, id: impl Into<gpui_kit::ElementId>, selected: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
        let c = &theme.colors;
        let dark = theme.is_dark();
        let rest = if dark { gpui_kit::hsla(0., 0., 1., 0.035) } else { c.background() };
        let hover = if dark { c.accent } else { with_alpha(c.muted, 0.5) };
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(theme.rem(0.75))
            .px(theme.rem(0.75))
            .rounded(theme.radii.lg)
            .border_1()
            .map(|card| {
                if selected {
                    if dark {
                        card.border_color(with_alpha(c.primary(), 0.3)).bg(with_alpha(c.primary(), 0.1))
                    } else {
                        card.border_color(c.primary()).bg(c.background())
                    }
                } else {
                    card.border_color(if dark { gpui_kit::hsla(0., 0., 0., 0.) } else { c.border })
                        .bg(rest)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                }
            })
    }

    fn render_provider_step(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let (current, _) = self.provider();
        let cards = self.options().into_iter().map(|(definition, readiness)| {
            let label = div().min_w_0().flex_1().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(definition.picker_label);
            let icon = Icon::new(source_control_icon(definition)).size(IconSize::Custom(theme.rem(1.25)));
            if !readiness.ready {
                let hint = readiness.hint.unwrap_or_else(|| t!("Open Settings -> Source control & issues to configure this provider."));
                return Self::card(theme, ("publish-provider", definition.kind.len()), false)
                    .cursor_default()
                    .opacity(0.64)
                    .py(theme.rem(0.75))
                    .child(icon.color(c.muted_foreground))
                    .child(label)
                    .child(
                        div().id(("publish-setup", definition.kind.len())).tooltip(tooltip(hint)).child(
                            Button::new(("publish-setup-button", definition.kind.len()))
                                .variant(ButtonVariant::WarningOutline)
                                .size(ButtonSize::Micro)
                                .label(t!("Setup Required"))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(PublishRepositoryEvent::OpenSourceControlSettings))),
                        ),
                    )
                    .into_any_element();
            }
            let kind = definition.kind;
            Self::card(theme, ("publish-provider", definition.kind.len()), current.kind == kind)
                .py(theme.rem(0.75))
                .on_click(cx.listener(move |this, _, window, cx| this.select(kind, window, cx)))
                .child(icon)
                .child(label)
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.rem(0.5))
            .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(t!("Provider")))
            .child(div().grid().grid_cols(2).gap(theme.rem(0.5)).children(cards))
            .into_any_element()
    }

    fn render_repository_step(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let (definition, _) = self.provider();
        let host: SharedString = definition
            .publish_host
            .map(str::to_owned)
            .or_else(|| signed_in_host(self.discovered_item(definition.kind)))
            .unwrap_or_else(|| "your server".into())
            .into();
        let field_label = |text: SharedString| div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(text);
        let visibility = [
            (SourceControlRepositoryVisibility::Private, t!("Private"), t!("Only invited people"), IconName::Lock),
            (SourceControlRepositoryVisibility::Public, t!("Public"), t!("Anyone on the web"), IconName::Globe),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (value, label, description, icon))| {
            Self::card(theme, ("publish-visibility", index), self.visibility == value)
                .py(theme.rem(0.625))
                .when(!self.pending, |card| {
                    card.on_click(cx.listener(move |this, _, _, cx| {
                        this.visibility = value;
                        cx.notify();
                    }))
                })
                .child(Icon::new(icon).size(IconSize::Custom(theme.rem(1.))).color(c.muted_foreground))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label))
                        .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(description)),
                )
        });
        let advanced = self.advanced_open.then(|| {
            let protocol = self.protocol;
            div()
                .mt(theme.rem(0.75))
                .grid()
                .grid_cols(2)
                .gap(theme.rem(0.75))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.rem(0.375))
                        .child(field_label(t!("Remote", context = "git remote name")))
                        .child(TextInput::new(&self.remote_name).disabled(self.pending)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.rem(0.375))
                        .child(field_label(t!("Protocol")))
                        .child(
                            div()
                                .flex()
                                .child(Toggle::new("publish-ssh").label("SSH").pressed(protocol == SourceControlCloneProtocol::Ssh).disabled(self.pending).on_toggle({
                                    let this = cx.weak_entity();
                                    move |_, _, cx| {
                                        this.update(cx, |this, cx| {
                                            this.protocol = SourceControlCloneProtocol::Ssh;
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                }))
                                .child(Toggle::new("publish-https").label("HTTPS").pressed(protocol == SourceControlCloneProtocol::Https).disabled(self.pending).on_toggle({
                                    let this = cx.weak_entity();
                                    move |_, _, cx| {
                                        this.update(cx, |this, cx| {
                                            this.protocol = SourceControlCloneProtocol::Https;
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                })),
                        ),
                )
        });
        let soft = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.035) } else { with_alpha(c.muted, 0.4) };
        let status = self.pending.then(|| {
            div()
                .flex()
                .items_center()
                .gap(theme.rem(0.5))
                .px(theme.rem(0.75))
                .py(theme.rem(0.5))
                .rounded(theme.radii.md)
                .bg(soft)
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(Spinner::new(SpinnerSize::Sm))
                .child(t_rich!("Publishing repository to {provider}...", provider = definition.picker_label).plain())
        });
        let error = self.error.clone().filter(|_| !self.pending).map(|error| {
            let destructive = c.destructive();
            div()
                .px(theme.rem(0.75))
                .py(theme.rem(0.5))
                .rounded(theme.radii.md)
                .border_1()
                .border_color(with_alpha(destructive, 0.4))
                .bg(with_alpha(destructive, 0.1))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(destructive)
                .child(div().font_weight(weight::MEDIUM).child(t!("Publish failed")))
                .child(div().mt(theme.rem(0.125)).text_color(with_alpha(destructive, 0.9)).child(error))
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.rem(1.25))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.5))
                    .child(field_label(t!("Repository")))
                    .child(
                        div()
                            .flex()
                            .items_stretch()
                            .overflow_hidden()
                            .rounded(theme.radii.md)
                            .border_1()
                            .border_color(c.input)
                            .bg(c.background())
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap(theme.rem(0.375))
                                    .px(theme.rem(0.625))
                                    .border_r_1()
                                    .border_color(c.input)
                                    .bg(with_alpha(c.muted, 0.5))
                                    .font(mono_font(weight::NORMAL))
                                    .text_size(px(TextSize::Xs.size(theme.fonts.interface)))
                                    .text_color(c.muted_foreground)
                                    .child(Icon::new(source_control_icon(definition)).size(IconSize::Custom(theme.rem(0.875))))
                                    .child(format!("{host}/")),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Styled::h(Input::new(&self.repository).appearance(false).disabled(self.pending), theme.rem(2.25))
                                        .px(theme.rem(0.75))
                                        .w_full()
                                        .mono_text(px(TextSize::Sm.size(theme.fonts.interface)), weight::NORMAL),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.5))
                    .child(field_label(t!("Visibility")))
                    .child(div().grid().grid_cols(2).gap(theme.rem(0.5)).children(visibility)),
            )
            .child(
                div()
                    .child(
                        div()
                            .id("publish-advanced")
                            .flex()
                            .items_center()
                            .gap(theme.rem(0.25))
                            .cursor_pointer()
                            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                            .text_color(c.muted_foreground)
                            .hover(|s| s.text_color(c.foreground()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.advanced_open = !this.advanced_open;
                                cx.notify();
                            }))
                            .child(
                                Icon::new(if self.advanced_open { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .size(IconSize::Custom(theme.rem(0.875))),
                            )
                            .child(t!("Advanced", context = "publish dialog section of advanced options")),
                    )
                    .children(advanced),
            )
            .children(status)
            .children(error)
            .into_any_element()
    }

    fn render_summary_step(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let soft = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.035) } else { with_alpha(c.muted, 0.4) };
        let (definition, _) = self.provider();
        let Some(result) = &self.result else {
            return div()
                .px(theme.rem(0.75))
                .py(theme.rem(0.5))
                .rounded(theme.radii.md)
                .bg(soft)
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(t!("Publish result unavailable."))
                .into_any_element();
        };
        let pushed = result.status == SourceControlPublishStatus::Pushed;
        let url = result.repository.url.clone();
        div()
            .flex()
            .flex_col()
            .gap(theme.rem(1.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(theme.rem(0.5))
                    .py(theme.rem(0.25))
                    .child(
                        div()
                            .flex()
                            .size(theme.rem(2.))
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(with_alpha(theme.fixed.success, 0.15))
                            .child(Icon::new(IconName::Check).size(IconSize::Custom(theme.rem(1.))).color(theme.fixed.success)),
                    )
                    .child(
                        div().ui_text(theme, TextSize::Sm, weight::SEMIBOLD).text_color(c.foreground()).child(if pushed {
                            t!("Repository published")
                        } else {
                            t!("Repository created")
                        }),
                    )
                    .child(
                        div()
                            .max_w(theme.rem(20.))
                            .text_center()
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(if pushed {
                                t!("{branch} is now live on {provider}.", branch = result.branch.as_str(), provider = definition.picker_label)
                            } else {
                                t!(
                                    "Remote \"{remote}\" is set up. Make a commit and push it to share your code.",
                                    remote = result.remote_name.as_str()
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.rem(0.5))
                    .px(theme.rem(0.75))
                    .py(theme.rem(0.5))
                    .rounded(theme.radii.lg)
                    .bg(soft)
                    .child(Icon::new(source_control_icon(definition)).size(IconSize::Custom(theme.rem(0.875))).color(c.muted_foreground))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .font(mono_font(weight::NORMAL))
                            .text_size(px(TextSize::Xs.size(theme.fonts.interface)))
                            .text_color(c.foreground())
                            .child(result.repository.name_with_owner.clone()),
                    ),
            )
            .child(
                Button::new("publish-open")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t_rich!("Open on {provider}", provider = definition.picker_label).plain())
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PublishRepositoryEvent::OpenUrl(url.clone())))),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut footer = div().flex().justify_end().gap(px(8.));
        if self.step == 2 {
            footer = footer.child(
                Button::new("publish-done")
                    .label(t!("Done", context = "button, closes the finished publish dialog"))
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            );
            return footer.into_any_element();
        }
        let (_, readiness) = self.provider();
        let any_ready = self.options().iter().any(|(_, r)| r.ready);
        footer = footer.child(
            Button::new("publish-back")
                .variant(ButtonVariant::Outline)
                .disabled(self.pending)
                .label(if self.step == 0 { t!("Cancel") } else { t!("Back") })
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.step == 0 {
                        this.close(cx);
                    } else {
                        let step = this.step - 1;
                        this.set_step(step, window, cx);
                    }
                })),
        );
        footer = if self.step < 1 {
            footer.child(
                Button::new("publish-next")
                    .disabled(!any_ready || !readiness.ready)
                    .label(t!("Next"))
                    .on_click(cx.listener(|this, _, window, cx| this.set_step(1, window, cx))),
            )
        } else {
            footer.child(
                Button::new("publish-submit")
                    .disabled(!self.can_submit(cx))
                    .label(if self.pending { t!("Publishing...") } else { t!("Publish", context = "verb") })
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
        };
        footer.into_any_element()
    }
}

impl Render for PublishRepositoryDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let panel = match self.step {
            0 => self.render_provider_step(&theme, cx),
            1 => self.render_repository_step(&theme, cx),
            _ => self.render_summary_step(&theme, cx),
        };
        let steps = self.render_steps(&theme, cx);
        let footer = self.render_footer(cx);
        let ring = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.05) } else { gpui_kit::hsla(0., 0., 0., 0.05) };
        let card = dialog_glass(&theme)
            .id("publish-repository-dialog")
            .w_full()
            .max_w(theme.rem(36.))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .p(sp(6.))
                    .pb(sp(3.))
                    .child(
                        div()
                            .ui_text(&theme, TextSize::Xl, weight::SEMIBOLD)
                            .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                            .child(t!("Publish repository")),
                    )
                    .child(
                        div()
                            .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(t!("Pick where to host it, then point us at a repo to push to.")),
                    )
                    .child(steps),
            )
            .child(
                div()
                    .px(sp(6.))
                    .py(theme.rem(1.25))
                    .bg(if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.02) } else { with_alpha(c.background(), 0.8) })
                    .border_t_1()
                    .border_b_1()
                    .border_color(ring)
                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                    .child(panel),
            )
            .child(div().px(sp(6.)).py(sp(4.)).child(footer))
            .test_support();
        let this = cx.weak_entity();
        Modal::dialog("publish-repository-modal", card, move |_, cx| {
            this.update(cx, |this, cx| this.close(cx)).ok();
        })
    }
}
