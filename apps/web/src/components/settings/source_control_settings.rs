//! Settings → Source control & issues. Ports `SourceControlSettings.tsx`:
//! the repository rows of `ProjectDefaultsSettings` (category
//! source-control), Version Control and Source control providers
//! (`DiscoveryItemRow`, with GitHub's `SourceControlAppSignInSettings` card in
//! its details), `IssueTrackerSettings.tsx`, and
//! `SourceControlWritingSettings.tsx` with its Advanced fold.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui_kit::{AnyElement, App, ClipboardItem, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{
    ClientServerSettingsPatch, ClientServerSettingsPatchSourceControlWritingStyle, IssueTrackerBindProjectInput,
    IssueTrackerConnectInput, IssueTrackerConnection, IssueTrackerConnectionList, SourceControlDiscoveryResult, IssueTrackerProjectBinding, PullRequestMergeMethod,
    SourceControlDiscoveryStatus, SourceControlProviderAuthStatus, SourceControlProviderDiscoveryItem,
    SourceControlSignInInput, SourceControlSignInPrompt, SourceControlWritingStyleMode, VcsDiscoveryItem,
};

use super::server_settings_controls::{
    Fields, advanced_fold, bool_setting, field_input, hint, lock_reason, save_patch, select_row, status_mark, tinted,
};
use super::server_settings_data::ServerPanelsModel;
use super::source_control_host_settings::{BITBUCKET_SETTINGS, GITCAFE_SETTINGS, SourceControlHostSettings};
use super::source_control_token_settings::{GITHUB_CREDENTIALS, SourceControlTokenSettings};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::threads_settings::model_select_row;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize, IconSource};
use crate::components::managed_policy::managed_by;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::switch::Switch;
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// A device-code sign-in in progress.
#[derive(Clone, Debug)]
struct SignIn {
    input: SourceControlSignInInput,
    prompt: Option<SourceControlSignInPrompt>,
    error: Option<SharedString>,
}

/// The Source control & issues page.
pub struct SourceControlSettingsPanel {
    /// The last `discoverSourceControl` and `issueTrackers.list` replies. A live source does not
    /// keep them in its snapshot, so the panel holds what it asked for.
    discovery: Option<Arc<SourceControlDiscoveryResult>>,
    trackers: Option<Arc<IssueTrackerConnectionList>>,
    /// GitHub's token form, kept so a draft survives re-renders.
    github_token: Entity<SourceControlTokenSettings>,
    /// The settings forms of hosts without token credentials (Bitbucket, GitCafe).
    host_forms: Vec<(&'static str, Entity<SourceControlHostSettings>)>,
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    sign_in: Option<SignIn>,
    /// Discovery rows and trackers whose details are open, by kind.
    expanded: BTreeSet<String>,
    advanced_open: bool,
    tracker_error: Option<(String, SharedString)>,
    _observe: Subscription,
}

/// An `Option` encoded as `{ _tag: "Some", value }` / `{ _tag: "None" }`, or a plain string.
fn opt_str(value: &impl serde::Serialize) -> Option<String> {
    let v = serde_json::to_value(value).ok()?;
    v.as_str().map(str::to_owned).or_else(|| v.get("value")?.as_str().map(str::to_owned))
}

fn host_icon(kind: &str) -> Option<IconSource> {
    Some(
        match kind {
            "github" => BrandIcon::GitHub,
            "gitlab" => BrandIcon::GitLab,
            "bitbucket" => BrandIcon::Bitbucket,
            "forgejo" => BrandIcon::Forgejo,
            "gitcafe" => BrandIcon::GitCafe,
            "cursor-origin" | "cursorOrigin" | "origin" => BrandIcon::CursorOrigin,
            "azure-devops" | "azureDevOps" => BrandIcon::AzureDevOps,
            "git" => BrandIcon::Git,
            "jj" | "jujutsu" => BrandIcon::Jujutsu,
            "linear" => BrandIcon::Linear,
            _ => return None,
        }
        .into(),
    )
}

impl SourceControlSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let github_token = cx.new(|cx| SourceControlTokenSettings::new(model.clone(), &GITHUB_CREDENTIALS, window, cx));
        let host_forms = [&BITBUCKET_SETTINGS, &GITCAFE_SETTINGS]
            .into_iter()
            .map(|definition| (definition.kind, cx.new(|cx| SourceControlHostSettings::new(model.clone(), definition, window, cx))))
            .collect();
        let mut this = Self {
            host_forms,
            discovery: None,
            trackers: None,
            github_token,
            model,
            fields: Fields::default(),
            sign_in: None,
            expanded: BTreeSet::new(),
            advanced_open: false,
            tracker_error: None,
            _observe: observe,
        };
        this.scan(cx);
        this.list_trackers(cx);
        this
    }

    /// `sourceControl.discovery`, again: the Rescan button and after a sign-in.
    fn scan(&mut self, cx: &mut Context<Self>) {
        let panel = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("discover-source-control", |s| s.discover_source_control(), move |result, cx| {
                if let Ok(result) = result {
                    _ = panel.update(cx, |this, cx| {
                        this.discovery = Some(Arc::new(result));
                        cx.notify();
                    });
                }
            }, cx)
        });
    }

    /// `issueTrackers.list`, kept for the Issue trackers section.
    fn list_trackers(&mut self, cx: &mut Context<Self>) {
        let panel = cx.entity().downgrade();
        self.model.update(cx, |m, cx| {
            m.run("issue-trackers", |s| s.list_issue_trackers(), move |result, cx| {
                if let Ok(result) = result {
                    _ = panel.update(cx, |this, cx| {
                        this.trackers = Some(Arc::new(result));
                        cx.notify();
                    });
                }
            }, cx)
        });
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    /// Starts a device-code sign-in (`sourceControl.signInStart`, then `signInWait`).
    pub fn start_sign_in(&mut self, input: SourceControlSignInInput, cx: &mut Context<Self>) {
        self.expanded.insert(input.provider.as_str().to_owned());
        self.sign_in = Some(SignIn { input: input.clone(), prompt: None, error: None });
        let this = cx.entity().downgrade();
        let wait_input = input.clone();
        self.model.update(cx, |m, cx| {
            m.run("sc-sign-in-start", move |s| s.source_control_sign_in_start(input), move |result, cx| {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(prompt) => {
                            if let Some(state) = &mut this.sign_in {
                                state.prompt = Some(prompt);
                            }
                            let done = cx.entity().downgrade();
                            this.model.update(cx, |m, cx| {
                                m.run("sc-sign-in-wait", move |s| s.source_control_sign_in_wait(wait_input), move |result, cx| {
                                    done.update(cx, |this, cx| {
                                        match result {
                                            Ok(_) => {
                                                this.sign_in = None;
                                                this.scan(cx);
                                            }
                                            Err(e) => {
                                                if let Some(state) = &mut this.sign_in {
                                                    state.prompt = None;
                                                    state.error = Some(e.message);
                                                }
                                            }
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                                }, cx)
                            });
                        }
                        Err(e) => {
                            if let Some(state) = &mut this.sign_in {
                                state.error = Some(e.message);
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

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.sign_in.take() {
            let input = state.input;
            self.model.update(cx, |m, cx| m.run("sc-sign-in-cancel", move |s| s.source_control_sign_in_cancel(input), |_, _| {}, cx));
        }
        cx.notify();
    }

    /// Sign out: the host's saved app sign-in becomes empty (`sourceControlHosts.<kind>.signIns`).
    fn sign_out(&mut self, kind: &str, host: &str, cx: &mut Context<Self>) {
        let snapshot = self.model.read(cx).snapshot();
        let mut sign_ins = snapshot
            .settings
            .source_control_hosts
            .as_ref()
            .and_then(|h| h.get(kind))
            .and_then(|h| h.get("signIns"))
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        sign_ins.insert(host.to_owned(), serde_json::Value::String(String::new()));
        let mut entry = serde_json::Map::new();
        entry.insert("signIns".into(), serde_json::Value::Object(sign_ins));
        let patch = ClientServerSettingsPatch {
            source_control_hosts: Some([(kind.to_owned(), entry)].into_iter().collect()),
            ..Default::default()
        };
        let this = cx.entity().downgrade();
        let input = t3code_contracts::ServerUpdateSettingsPayload { patch, provider_instance_mutation: None };
        self.model.update(cx, |m, cx| {
            m.run("sc-sign-out", move |s| s.update_settings(input), move |result, cx| {
                match result {
                    Ok(_) => {
                        this.update(cx, |this, cx| this.scan(cx)).ok();
                    }
                    Err(error) => super::provider_settings_panel::toast_error(t!("Could not save settings"), error, cx),
                }
            }, cx)
        });
    }

    pub fn sign_in_code(&self) -> Option<&str> {
        self.sign_in.as_ref()?.prompt.as_ref().map(|p| p.user_code.as_str())
    }

    /// `DiscoveryItemRow`: mark, label, version, badges, summary; a details
    /// chevron when it has details; the offered switch (providers) or a
    /// read-only availability switch (version control).
    #[allow(clippy::too_many_arguments)]
    fn discovery_row(
        &self,
        key: String,
        icon: Option<IconSource>,
        dot: gpui_kit::Hsla,
        label: String,
        version: Option<String>,
        badges: Vec<AnyElement>,
        summary: AnyElement,
        dimmed: bool,
        control: Option<AnyElement>,
        details: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let open = self.expanded.contains(&key);
        let has_details = details.is_some();
        let toggle_key = key.clone();
        div()
            .flex()
            .flex_col()
            .hover(|d| d.bg(with_alpha(c.muted, 0.2)))
            .when(dimmed, |d| d.opacity(0.8))
            .child(
                div().px(theme.sp(4.)).py(theme.sp(3.)).child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.sp(3.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(theme.sp(1.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(theme.sp(2.))
                                        .min_w_0()
                                        .child(status_mark(icon, dot, cx))
                                        .child(div().truncate().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label))
                                        .children(version.map(|v| div().mono_text(px(TextSize::Xs.size(theme.fonts.interface)), weight::NORMAL).text_color(c.muted_foreground).child(v)))
                                        .children(badges),
                                )
                                .child(
                                    div()
                                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                        .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.5))
                                        .text_color(with_alpha(c.muted_foreground, 0.8))
                                        .child(summary),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(theme.sp(2.))
                                .when(has_details, |d| {
                                    d.child(
                                        Button::new(SharedString::from(format!("sc-details-{key}")))
                                            .variant(ButtonVariant::GhostMuted)
                                            .size(ButtonSize::IconXs)
                                            .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
                                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx))),
                                    )
                                })
                                .children(control),
                        ),
                ),
            )
            .when(open, |d| d.children(details.map(|details| div().px(theme.sp(4.)).pb(theme.sp(4.)).pt(theme.sp(1.)).child(details))))
            .into_any_element()
    }

    fn render_version_control(&mut self, items: &[VcsDiscoveryItem], cx: &mut Context<Self>) -> SettingsSection {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let refreshing = self.model.read(cx).is_pending("discover-source-control");
        let mut section = SettingsSection::new(t!("Version Control")).header_action(
            div().id("sc-rescan-tip").tooltip(tooltip(TooltipSurface::new(t!("Rescan Git and hosting integrations")))).child(
                Button::new("sc-rescan")
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::IconXs)
                    .icon(IconName::RefreshCw)
                    .disabled(refreshing)
                    .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
            ),
        );
        for item in items {
            let not_ready = !item.implemented;
            let available = item.status == SourceControlDiscoveryStatus::Available;
            let dot = if not_ready {
                with_alpha(c.muted_foreground, 0.35)
            } else if available {
                theme.fixed.success
            } else {
                c.warning
            };
            let summary: SharedString = if not_ready {
                t!("Support for {label} is coming soon.", label = item.label.clone())
            } else if !available {
                t!("Not available on this server: {hint}", hint = item.install_hint.clone())
            } else {
                t!("Available")
            };
            let badges = if not_ready {
                vec![Badge::new(t!("Coming Soon")).variant(BadgeVariant::Warning).size(BadgeSize::Sm).into_any_element()]
            } else {
                Vec::new()
            };
            let control = (!not_ready).then(|| Switch::new(SharedString::from(format!("vcs-{}", item.kind.as_str())), available).disabled(true).into_any_element());
            let row = self.discovery_row(
                format!("vcs:{}", item.kind.as_str()),
                host_icon(item.kind.as_str()),
                dot,
                item.label.clone(),
                opt_str(&item.version),
                badges,
                div().child(summary).into_any_element(),
                not_ready,
                control,
                None,
                cx,
            );
            section.extend([row]);
        }
        section
    }

    fn render_providers(&mut self, items: &[SourceControlProviderDiscoveryItem], cx: &mut Context<Self>) -> SettingsSection {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let offered = snapshot.settings.offered_source_control_providers.clone().unwrap_or_default();
        let mut section = SettingsSection::new(t!("Source control providers"));
        for item in items {
            let kind = item.kind.as_str().to_owned();
            let available = item.status == SourceControlDiscoveryStatus::Available;
            let authenticated = item.auth.status == SourceControlProviderAuthStatus::Authenticated;
            let account = opt_str(&item.auth.account);
            let detail = opt_str(&item.auth.detail);
            let dot = if available && authenticated { theme.fixed.success } else { with_alpha(c.muted_foreground, 0.35) };
            let mut badges = Vec::new();
            if item.auth.status == SourceControlProviderAuthStatus::Unauthenticated {
                badges.push(Badge::new(t!("Not authenticated")).variant(BadgeVariant::Secondary).size(BadgeSize::Sm).into_any_element());
            }
            let summary: AnyElement = if !available {
                div().child(t!("Not available on this server: {hint}", hint = item.install_hint.clone())).into_any_element()
            } else if authenticated {
                let head = match &account {
                    Some(account) => t!("Authenticated as {account}", account = account.clone()),
                    None => t!("Authenticated"),
                };
                div().child(match detail {
                    Some(d) => SharedString::from(format!("{head} · {d}")),
                    None => head,
                })
                .into_any_element()
            } else if item.executable.is_none() && item.auth.status == SourceControlProviderAuthStatus::Unauthenticated {
                div().child(t!("Available. {hint}", hint = item.install_hint.clone())).into_any_element()
            } else if item.auth.status == SourceControlProviderAuthStatus::Unauthenticated
                && item.auth.accounts.as_ref().is_some_and(|a| a.iter().any(|e| e.authenticated))
            {
                div().child(detail.map(SharedString::from).unwrap_or_else(|| t!("Every {label} host is turned off.", label = item.label.clone()))).into_any_element()
            } else if item.auth.status == SourceControlProviderAuthStatus::Unauthenticated {
                let exe = item.executable.clone().unwrap_or_default();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(theme.sp(1.))
                    .child(t!("{label} is not authenticated on this server. Sign in or configure credentials using the", label = item.label.clone()))
                    .child(div().rounded(theme.radii.sm).bg(c.muted).px(theme.sp(1.)).mono_text(px(TextSize::Xs2.size(theme.fonts.interface)), weight::NORMAL).child(exe))
                    .child(t!("tool on the server host to enable change request features."))
                    .into_any_element()
            } else {
                div().child(t!("Could not verify {label}. {detail}", label = item.label.clone(), detail = detail.unwrap_or_else(|| item.install_hint.clone()))).into_any_element()
            };
            let checked = offered.get(&kind).copied().unwrap_or(kind == "github");
            let offer_model = self.model.clone();
            let offer_kind = kind.clone();
            let control = div()
                .id(SharedString::from(format!("sc-offer-tip-{kind}")))
                .tooltip(tooltip(TooltipSurface::new(t!("Offer when adding or publishing a project"))))
                .child(Switch::new(SharedString::from(format!("sc-offer-{kind}")), checked).disabled(snapshot.read_only).on_change(move |v, _, cx| {
                    let patch = ClientServerSettingsPatch {
                        offered_source_control_providers: Some([(offer_kind.clone(), v)].into_iter().collect()),
                        ..Default::default()
                    };
                    save_patch(&offer_model, "offeredSourceControlProviders", patch, cx);
                }))
                .into_any_element();
            let details = self.host_details(item, cx);
            let row = self.discovery_row(
                kind.clone(),
                host_icon(&kind),
                dot,
                item.label.clone(),
                opt_str(&item.version),
                badges,
                summary,
                false,
                Some(control),
                details,
                cx,
            );
            section.extend([row]);
        }
        section
    }

    /// `hostSettingsPanel`: a host with credentials shows its app sign-in (when offered), then
    /// the token form; a host with a settings schema shows its form; others have no details.
    fn host_details(&mut self, item: &SourceControlProviderDiscoveryItem, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some((_, form)) = self.host_forms.iter().find(|(kind, _)| *kind == item.kind.as_str()) {
            return Some(form.clone().into_any_element());
        }
        if item.kind.as_str() != GITHUB_CREDENTIALS.kind {
            return (item.app_sign_in == Some(true)).then(|| self.render_app_sign_in(item, cx));
        }
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let app = item.app_sign_in == Some(true);
        let rule = || div().h(px(1.)).flex_1().bg(c.border);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(6.))
                .when(app, |d| {
                    d.child(self.render_app_sign_in(item, cx)).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(3.))
                            .ui_text(&theme, TextSize::Xs2, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(rule())
                            .child(t!("Or use your own credentials"))
                            .child(rule()),
                    )
                })
                .child(self.github_token.clone())
                .into_any_element(),
        )
    }

    /// `SourceControlAppSignInSettings`: the GitHub app card.
    fn render_app_sign_in(&mut self, item: &SourceControlProviderDiscoveryItem, cx: &mut Context<Self>) -> AnyElement {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let kind = item.kind.as_str().to_owned();
        let host = opt_str(&item.auth.host).unwrap_or_else(|| "github.com".into());
        let signed_in = snapshot
            .settings
            .source_control_hosts
            .as_ref()
            .and_then(|h| h.get(&kind))
            .and_then(|h| h.get("signIns"))
            .and_then(|s| s.get(&host))
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        let state = self.sign_in.clone().filter(|s| s.input.provider.as_str() == kind);
        let waiting = state.as_ref().and_then(|s| s.prompt.clone()).filter(|_| !signed_in);
        let failed = state.as_ref().and_then(|s| s.error.clone()).filter(|_| !signed_in);
        let starting = state.as_ref().is_some_and(|s| s.prompt.is_none() && s.error.is_none());
        let account = opt_str(&item.auth.account);
        let (subtitle, action): (SharedString, AnyElement) = if signed_in {
            let (k, h) = (kind.clone(), host.clone());
            (
                account.map(|a| t!("Signed in as {account}", account = a)).unwrap_or_else(|| t!("Signed in")),
                Button::new("sc-sign-out")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Xs)
                    .label(t!("Sign out"))
                    .disabled(snapshot.read_only)
                    .on_click(cx.listener(move |this, _, _, cx| this.sign_out(&k, &h, cx)))
                    .into_any_element(),
            )
        } else if waiting.is_some() {
            (
                t!("Approve the app on GitHub to finish signing in."),
                Button::new("sc-cancel")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Xs)
                    .label(t!("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)))
                    .into_any_element(),
            )
        } else {
            let input = SourceControlSignInInput { provider: item.kind.clone(), host: host.clone() };
            (
                t!("A far higher GitHub API rate limit than a personal token or gh login."),
                Button::new("sc-sign-in")
                    .size(ButtonSize::Xs)
                    .label(if failed.is_some() { t!("Try again") } else { t!("Sign in with GitHub") })
                    .disabled(snapshot.read_only || starting)
                    .on_click(cx.listener(move |this, _, _, cx| this.start_sign_in(input.clone(), cx)))
                    .into_any_element(),
            )
        };
        let mark = status_mark(Some(BrandIcon::GitHub.into()), if signed_in { theme.fixed.success } else { with_alpha(c.muted_foreground, 0.35) }, cx);
        div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme.radii.xl)
            .border_1()
            .border_color(if failed.is_some() { with_alpha(c.destructive(), 0.4) } else { c.border })
            .bg(c.card())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(3.))
                    .p(theme.sp(3.))
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(0.5))
                            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child("T3 Code for GitHub"))
                            .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(subtitle)),
                    )
                    .child(div().flex_none().child(action)),
            )
            .children(waiting.map(|prompt| {
                let (code, url) = (prompt.user_code.clone(), prompt.verification_uri.clone());
                let copy = code.clone();
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(theme.sp(4.))
                    .border_t_1()
                    .border_color(c.border)
                    .bg(with_alpha(c.muted, 0.4))
                    .px(theme.sp(3.))
                    .py(theme.sp(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(div().flex().items_center().gap(theme.sp(1.)).children(prompt.user_code.chars().map(|ch| {
                                if ch == '-' {
                                    div().px(theme.sp(0.5)).text_color(c.muted_foreground).mono_text(px(18.), weight::MEDIUM).child("–")
                                } else {
                                    div()
                                        .h(theme.sp(10.))
                                        .w(theme.sp(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(theme.radii.md)
                                        .border_1()
                                        .border_color(c.border)
                                        .bg(c.background())
                                        .mono_text(px(18.), weight::MEDIUM)
                                        .child(ch.to_string())
                                }
                            })))
                            .child(
                                Button::new("sc-copy-code")
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::IconXs)
                                    .icon(IconName::Copy)
                                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(theme.sp(2.))
                            .child(
                                Button::new("sc-copy-open")
                                    .size(ButtonSize::Sm)
                                    .label(t!("Copy code & open GitHub"))
                                    .trailing_icon(IconName::ExternalLink)
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                        cx.open_url(&url);
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.sp(1.5))
                                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                    .text_color(c.muted_foreground)
                                    .child(div().size(px(6.)).rounded_full().bg(c.warning))
                                    .child(t!("Waiting for approval")),
                            ),
                    )
            }))
            .when(signed_in, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(theme.sp(3.))
                        .gap_y(theme.sp(1.))
                        .border_t_1()
                        .border_color(c.border)
                        .bg(with_alpha(c.muted, 0.4))
                        .px(theme.sp(3.))
                        .py(theme.sp(2.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(div().flex_1().min_w_0().child(t!("Reaches repositories in accounts and organizations that installed the app."))),
                )
            })
            .children(failed.map(|message| {
                div()
                    .border_t_1()
                    .border_color(with_alpha(c.destructive(), 0.2))
                    .bg(with_alpha(c.destructive(), 0.05))
                    .px(theme.sp(3.))
                    .py(theme.sp(2.))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.destructive())
                    .child(message)
            }))
            .into_any_element()
    }

    fn connect_tracker(&mut self, tracker: &IssueTrackerConnection, token: Option<String>, cx: &mut Context<Self>) {
        let input = IssueTrackerConnectInput { tracker: tracker.tracker.clone(), token };
        let key = tracker.tracker.as_str().to_owned();
        let this = cx.entity().downgrade();
        self.tracker_error = None;
        self.model.update(cx, |m, cx| {
            m.run(format!("tracker-connect:{key}"), move |s| s.connect_issue_tracker(input), move |result, cx| {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(_) => this.list_trackers(cx),
                        Err(e) => this.tracker_error = Some((key.clone(), e.message)),
                    }
                    cx.notify();
                })
                .ok();
            }, cx)
        });
    }

    fn render_issue_trackers(&mut self, window: &mut Window, cx: &mut Context<Self>) -> SettingsSection {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let mut section = SettingsSection::new(t!("Issue trackers"));
        let Some(list) = self.trackers.clone().or_else(|| snapshot.issue_trackers.clone()) else {
            section.extend([SettingsRow::new(t!("Loading…")).into_any_element()]);
            return section;
        };
        for tracker in &list.trackers {
            let key = tracker.tracker.as_str().to_owned();
            let usable = tracker.connected && tracker.account.is_some();
            let linked = list.bindings.values().filter(|b| b.tracker == tracker.tracker).count();
            let summary: SharedString = if !tracker.connected {
                t!("Not connected. Add a {credential} to list its issues.", credential = tracker.credential.label.to_lowercase())
            } else if tracker.account.is_none() {
                tracker.detail.clone().map(SharedString::from).unwrap_or_else(|| t!("The saved credential was rejected."))
            } else if linked == 0 {
                t!(
                    "Connected as {account} · no projects linked yet. Pick a {scope} for a project to list its issues.",
                    account = tracker.account.clone().unwrap_or_default(),
                    scope = tracker.scope_noun.clone()
                )
            } else if linked == 1 {
                t!("Connected as {account} · {count} project linked", account = tracker.account.clone().unwrap_or_default(), count = linked)
            } else {
                t!("Connected as {account} · {count} projects linked", account = tracker.account.clone().unwrap_or_default(), count = linked)
            };
            let badges = if tracker.connected && tracker.account.is_none() {
                vec![Badge::new(t!("Credential rejected")).variant(BadgeVariant::Destructive).size(BadgeSize::Sm).into_any_element()]
            } else {
                Vec::new()
            };
            // Details: the credential (saved on blur) and per-project bindings.
            let connect_tracker = tracker.clone();
            let this = cx.entity().downgrade();
            let field = self.fields.draft(
                format!("tracker:{key}"),
                "",
                if tracker.connected { t!("Saved") } else { SharedString::default() },
                true,
                window,
                cx,
                move |text, _, cx| {
                    let token = text.trim().to_owned();
                    if !token.is_empty() {
                        this.update(cx, |this, cx| this.connect_tracker(&connect_tracker, Some(token), cx)).ok();
                    }
                },
            );
            let disconnect_tracker = tracker.clone();
            let error = self.tracker_error.as_ref().filter(|(k, _)| *k == key).map(|(_, e)| e.clone());
            let mut description = div().flex().flex_col().child(t!("Stored on the server."));
            if let Some(url) = tracker.credential.help_url.clone() {
                description = description.child(
                    div()
                        .id(SharedString::from(format!("tracker-help-{key}")))
                        .flex()
                        .items_center()
                        .gap(theme.sp(1.))
                        .cursor_pointer()
                        .text_color(c.foreground())
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(t!("Create one"))
                        .child(Icon::new(IconName::ExternalLink).size(IconSize::Xs)),
                );
            }
            if let Some(error) = error {
                description = description.child(tinted(error, c.destructive(), cx));
            }
            let credential_row = SettingsRow::new(SharedString::from(tracker.credential.label.clone()))
                .status(description)
                .control(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.sp(2.))
                        .w(theme.sp(56.))
                        .child(div().flex_1().min_w_0().child(field_input(&field, false, false, snapshot.read_only)))
                        .when(tracker.connected, |d| {
                            d.child(
                                Button::new(SharedString::from(format!("tracker-disconnect-{key}")))
                                    .variant(ButtonVariant::GhostDestructive)
                                    .size(ButtonSize::IconSm)
                                    .icon(IconName::X)
                                    .disabled(snapshot.read_only)
                                    .on_click(cx.listener(move |this, _, _, cx| this.connect_tracker(&disconnect_tracker, None, cx))),
                            )
                        }),
                );
            let mut details = div().flex().flex_col().child(credential_row);
            if usable && !tracker.scopes.is_empty() && !snapshot.projects.is_empty() {
                details = details.child(SettingsRow::new(t!("Projects")).description(t!(
                    "Pick the {tracker} {scope} whose issues each project lists beside its repository's own.",
                    tracker = tracker.display_name.clone(),
                    scope = tracker.scope_noun.clone()
                )));
                for project in snapshot.projects.iter() {
                    let bound = list.bindings.get(project.id.as_str()).filter(|b| b.tracker == tracker.tracker);
                    let mut options = vec![t!("Not bound")];
                    options.extend(tracker.scopes.iter().map(|s| SharedString::from(format!("{} ({})", s.name, s.key))));
                    let selected = bound.and_then(|b| tracker.scopes.iter().position(|s| s.key == b.scope)).map_or(0, |ix| ix + 1);
                    let (scopes, project_id, kind, model) = (tracker.scopes.clone(), project.id.clone(), tracker.tracker.clone(), self.model.clone());
                    details = details.child(select_row(
                        format!("tracker-bind-{key}-{}", project.id.as_str()),
                        SharedString::from(project.title.clone()),
                        None,
                        options,
                        selected,
                        None,
                        move |ix, _, cx| {
                            let binding = ix.checked_sub(1).and_then(|i| scopes.get(i)).map(|s| IssueTrackerProjectBinding { tracker: kind.clone(), scope: s.key.clone() });
                            let input = IssueTrackerBindProjectInput { project_id: project_id.clone(), binding };
                            model.update(cx, |m, cx| {
                                m.run("tracker-bind", move |s| s.bind_issue_tracker_project(input), |_, _| {}, cx);
                            });
                        },
                    ));
                }
            }
            let row = self.discovery_row(
                format!("tracker:{key}"),
                host_icon(&key),
                if usable { theme.fixed.success } else { with_alpha(c.muted_foreground, 0.35) },
                tracker.display_name.clone(),
                None,
                badges,
                div().child(summary).into_any_element(),
                false,
                None,
                Some(details.into_any_element()),
                cx,
            );
            section.extend([row]);
        }
        section
    }
}

fn merge_method_label(method: Option<PullRequestMergeMethod>) -> SharedString {
    match method {
        None => t!("Last selected"),
        Some(PullRequestMergeMethod::Merge) => t!("Merge commit"),
        Some(PullRequestMergeMethod::Squash) => t!("Squash and merge"),
        Some(PullRequestMergeMethod::Rebase) => t!("Rebase and merge"),
        Some(PullRequestMergeMethod::FastForward) => t!("Fast-forward"),
    }
}

impl Render for SourceControlSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &cx.app_theme().clone();
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let lock = |paths: &[&str]| lock_reason(managed_by(policy, paths), ro);
        let model = self.model.clone();

        let methods = [None, Some(PullRequestMergeMethod::Squash), Some(PullRequestMergeMethod::Merge), Some(PullRequestMergeMethod::Rebase), Some(PullRequestMergeMethod::FastForward)];
        let method_ix = methods.iter().position(|m| *m == s.pull_request_merge_method).unwrap_or(0);
        let method_model = model.clone();
        let repositories = SettingsSection::new(t!("Repositories"))
            .child(bool_setting(&model, "defaultAutoPull", t!("Automatically pull"), Some(t!("Keeps the default branch current when the checkout has no local changes or commits. Projects can override it.")), s.default_auto_pull.unwrap_or(false), lock(&["defaultAutoPull"]), |v| ClientServerSettingsPatch { default_auto_pull: Some(v), ..Default::default() }))
            .child(bool_setting(&model, "removeAgentCreditsOnMerge", t!("Remove agent credits when merging"), Some(t!("Remove recognized agent credit lines from merge and squash messages on hosts that support it, keeping human co-authors. Includes auto-merge. Excludes merge queues, stack merges, and existing commits.")), s.remove_agent_credits_on_merge.unwrap_or(false), lock(&["removeAgentCreditsOnMerge"]), |v| ClientServerSettingsPatch { remove_agent_credits_on_merge: Some(v), ..Default::default() }))
            .child(select_row("pullRequestMergeMethod", t!("Default merge method"), Some(t!("Pull requests start with this method. Last selected reuses whatever you chose most recently on this device.")), methods.iter().map(|m| merge_method_label(*m)).collect(), method_ix, lock(&["pullRequestMergeMethod"]), move |ix, _, cx| {
                // Last selected clears the setting with null.
                let patch = ClientServerSettingsPatch { pull_request_merge_method: Some(methods[ix]), ..Default::default() };
                save_patch(&method_model, "pullRequestMergeMethod", patch, cx);
            }));

        let style = s.source_control_writing_style.as_ref().and_then(|w| serde_json::to_value(w).ok());
        let mode = style.as_ref().and_then(|v| v.get("mode")).and_then(|m| m.as_str()).and_then(SourceControlWritingStyleMode::from_wire).unwrap_or(SourceControlWritingStyleMode::RepoConventions);
        let follow = style.as_ref().and_then(|v| v.get("followChangeRequestTemplates")).and_then(|f| f.as_bool()).unwrap_or(true);
        let modes = [
            (SourceControlWritingStyleMode::RepoConventions, t!("Repository conventions"), t!("Commit messages and pull request titles and descriptions match each project's history.")),
            (SourceControlWritingStyleMode::ConventionalCommits, t!("Conventional Commits"), t!("Conventional Commit prefixes, with short pull request titles and descriptions.")),
            (SourceControlWritingStyleMode::Custom, t!("Custom instructions"), t!("Your instructions shape commit messages and pull request titles and descriptions.")),
        ];
        let mode_ix = modes.iter().position(|(m, _, _)| *m == mode).unwrap_or(0);
        let style_model = model.clone();
        let follow_model = model.clone();
        let guide_model = model.clone();
        let guide_instructions = self.fields.draft("reviewGuideInstructions", s.review_guide_instructions.clone().unwrap_or_default(), "", false, window, cx, move |text, _, cx| {
            save_patch(&guide_model, "reviewGuideInstructions", ClientServerSettingsPatch { review_guide_instructions: Some(text.trim().to_owned()), ..Default::default() }, cx);
        });
        let guides_on = s.review_guides_enabled.unwrap_or(false);
        let mut writing = SettingsSection::new(t!("Commits & pull requests"))
            .child(select_row("sourceControlWritingStyle", t!("Commit and pull request writing"), Some(modes[mode_ix].2.clone()), modes.iter().map(|(_, l, _)| l.clone()).collect(), mode_ix, lock(&["sourceControlWritingStyle"]), move |ix, _, cx| {
                let patch = ClientServerSettingsPatch { source_control_writing_style: Some(ClientServerSettingsPatchSourceControlWritingStyle { mode: Some(modes_static(ix)), ..Default::default() }), ..Default::default() };
                save_patch(&style_model, "sourceControlWritingStyle", patch, cx);
            }))
            .child(bool_setting(&model, "reviewGuidesEnabled", t!("Review guides (beta)"), Some(t!("Add a Guide tab to pull requests. An agent explains the changes and points out decisions that need review.")), guides_on, lock(&["reviewGuidesEnabled"]), |v| ClientServerSettingsPatch { review_guides_enabled: Some(v), ..Default::default() }));
        if guides_on {
            writing = writing
                .child(model_select_row(&model, "reviewGuideModelSelection", t!("Review guide model"), Some(t!("Must be a coding agent: it reads the pull request over many steps. Off uses the writer model, or Codex or Claude if that model is a text generator.")), s.review_guide_model_selection.as_ref(), Some(t!("Off")), lock(&["reviewGuideModelSelection"]), |v| ClientServerSettingsPatch { review_guide_model_selection: Some(v), ..Default::default() }, cx))
                .child(SettingsRow::new(t!("Review guide instructions")).description(t!("Your preferences for review guides: what to show first, dwell on, or skip. The guide's format and coverage stay the same.")).control(div().w(theme.sp(64.)).child(field_input(&guide_instructions, false, false, ro))));
        }
        let entity = cx.entity().downgrade();
        let advanced = advanced_fold(
            "sc-advanced",
            self.advanced_open,
            move |_, cx| {
                entity.update(cx, |this, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                })
                .ok();
            },
            vec![
                super::server_settings_controls::switch_row("followChangeRequestTemplates", t!("Follow pull request templates"), Some(t!("Use the repository's pull request template when it has one.")), follow, lock(&["sourceControlWritingStyle.followChangeRequestTemplates"]), move |v, _, cx| {
                    let patch = ClientServerSettingsPatch { source_control_writing_style: Some(ClientServerSettingsPatchSourceControlWritingStyle { follow_change_request_templates: Some(v), ..Default::default() }), ..Default::default() };
                    save_patch(&follow_model, "followChangeRequestTemplates", patch, cx);
                })
                .into_any_element(),
                model_select_row(&model, "sourceControlWriterModelSelection", t!("Source control writer model"), Some(t!("Model for commit messages, pull request text, and branch or bookmark names. When disabled, use the text generation model configured for this environment.")), s.source_control_writer_model_selection.as_ref(), Some(t!("Off")), lock(&["sourceControlWriterModelSelection"]), |v| ClientServerSettingsPatch { source_control_writer_model_selection: Some(v), ..Default::default() }, cx)
                    .into_any_element(),
            ],
            cx,
        );
        writing = writing.child(advanced);

        let mut page = page_container(cx).child(repositories);
        match self.discovery.clone().or_else(|| snapshot.source_control.clone()) {
            Some(discovery) if !discovery.version_control_systems.is_empty() || !discovery.source_control_providers.is_empty() => {
                if !discovery.version_control_systems.is_empty() {
                    page = page.child(self.render_version_control(&discovery.version_control_systems, cx));
                }
                if !discovery.source_control_providers.is_empty() {
                    page = page.child(self.render_providers(&discovery.source_control_providers, cx));
                }
            }
            Some(_) => {
                page = page.child(SettingsSection::new(t!("Version Control")).child(SettingsRow::new(t!("No version control tools found")).control(
                    Button::new("sc-scan-empty").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Scan again")).on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
                )));
            }
            None => {
                page = page.child(SettingsSection::new(t!("Version Control")).child(div().px(theme.sp(4.)).py(theme.sp(3.)).child(hint(t!("Loading…"), cx))));
            }
        }
        page.child(self.render_issue_trackers(window, cx)).child(writing)
    }
}

fn modes_static(ix: usize) -> SourceControlWritingStyleMode {
    [SourceControlWritingStyleMode::RepoConventions, SourceControlWritingStyleMode::ConventionalCommits, SourceControlWritingStyleMode::Custom][ix]
}
