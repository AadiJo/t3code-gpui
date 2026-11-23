//! Settings → Source control & issues. Ports `SourceControlSettings.tsx`
//! (discovered providers and their accounts), `SourceControlAppSignInSettings`
//! (GitHub device-code sign-in: a code to copy, a page to open, then wait),
//! `IssueTrackerSettings.tsx`, the repository rows of
//! `ProjectDefaultsSettings.tsx` (category source-control) and
//! `SourceControlWritingSettings.tsx`.

use gpui_kit::{ClipboardItem, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use t3code_contracts::{
    ClientServerSettingsPatch, ClientServerSettingsPatchSourceControlWritingStyle, PullRequestMergeMethod,
    SourceControlProviderAuthStatus, SourceControlSignInInput, SourceControlSignInPrompt, SourceControlWritingStyleMode,
};

use super::server_settings_controls::{Fields, bool_setting, field_input, hint, lock_reason, save_patch, select_row, tinted};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::threads_settings::model_select_row;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::weight;
use crate::components::icons::IconName;
use crate::components::managed_policy::managed_by;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// A device-code sign-in in progress, per host.
#[derive(Clone, Debug)]
struct SignIn {
    input: SourceControlSignInInput,
    prompt: Option<SourceControlSignInPrompt>,
    error: Option<SharedString>,
}

/// The Source control & issues page.
pub struct SourceControlSettingsPanel {
    model: Entity<ServerPanelsModel>,
    fields: Fields,
    sign_in: Option<SignIn>,
    _observe: Subscription,
}

fn json_str(value: &impl serde::Serialize, key: &str) -> Option<String> {
    let v = serde_json::to_value(value).ok()?;
    let v = if key.is_empty() { v } else { v.get(key)?.clone() };
    v.as_str().map(str::to_owned)
}

impl SourceControlSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        // Load discovery and trackers once; their replies land in the snapshot.
        model.update(cx, |m, cx| {
            m.run("discover-source-control", |s| s.discover_source_control(), |_, _| {}, cx);
            m.run("issue-trackers", |s| s.list_issue_trackers(), |_, _| {}, cx);
        });
        Self { model, fields: Fields::default(), sign_in: None, _observe: observe }
    }

    /// Starts a device-code sign-in (`sourceControl.signInStart`, then `signInWait`).
    pub fn start_sign_in(&mut self, input: SourceControlSignInInput, cx: &mut Context<Self>) {
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
                                            Ok(_) => this.sign_in = None,
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

    pub fn sign_in_code(&self) -> Option<&str> {
        self.sign_in.as_ref()?.prompt.as_ref().map(|p| p.user_code.as_str())
    }

    fn render_providers(&mut self, cx: &mut Context<Self>) -> SettingsSection {
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let snapshot = self.model.read(cx).snapshot().clone();
        let mut section = SettingsSection::new(t!("Source control"));
        let Some(discovery) = snapshot.source_control.clone() else {
            section.extend([SettingsRow::new(t!("Loading…")).into_any_element()]);
            return section;
        };
        for provider in &discovery.source_control_providers {
            let host = json_str(&provider.auth.host, "").or_else(|| json_str(&provider.auth.host, "value")).unwrap_or_else(|| "github.com".into());
            let account = json_str(&provider.auth.account, "").or_else(|| json_str(&provider.auth.account, "value"));
            let installed = provider.status.as_str() == "available";
            let status: SharedString = match (provider.auth.status, account) {
                (SourceControlProviderAuthStatus::Authenticated, Some(account)) => t!("Signed in as {account}", account = account),
                (SourceControlProviderAuthStatus::Authenticated, None) => t!("Signed in"),
                (SourceControlProviderAuthStatus::Unauthenticated, _) => t!("Not signed in"),
                _ if !installed => SharedString::from(provider.install_hint.clone()),
                _ => t!("Status unknown"),
            };
            let signing_in = self.sign_in.as_ref().filter(|s| s.input.provider == provider.kind && s.input.host == host).cloned();
            let mut row = SettingsRow::new(SharedString::from(provider.label.clone())).description(status);
            if provider.app_sign_in == Some(true) {
                let input = SourceControlSignInInput { provider: provider.kind.clone(), host: host.clone() };
                row = row.control(match &signing_in {
                    Some(_) => Button::new(SharedString::from(format!("sc-cancel-{host}")))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Sm)
                        .label(t!("Cancel"))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)))
                        .into_any_element(),
                    None => Button::new(SharedString::from(format!("sc-sign-in-{host}")))
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .label(if provider.auth.status == SourceControlProviderAuthStatus::Authenticated { t!("Sign out") } else { t!("Sign in with GitHub") })
                        .disabled(snapshot.read_only)
                        .on_click(cx.listener(move |this, _, _, cx| this.start_sign_in(input.clone(), cx)))
                        .into_any_element(),
                });
            }
            if let Some(state) = signing_in {
                let mut body = div().flex().flex_col().gap(theme.sp(2.)).pb(theme.sp(2.));
                body = body.child(hint(t!("Approve the app on GitHub to finish signing in."), cx));
                if let Some(prompt) = &state.prompt {
                    let (code, url) = (prompt.user_code.clone(), prompt.verification_uri.clone());
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(div().px(theme.sp(2.)).py(theme.sp(1.)).rounded(theme.radii.md).bg(with_alpha(c.muted, 0.6)).mono_text(px(16.), weight::SEMIBOLD).child(prompt.user_code.clone()))
                            .child(
                                Button::new("sc-copy-code")
                                    .variant(ButtonVariant::GhostMuted)
                                    .size(ButtonSize::IconSm)
                                    .icon(IconName::Copy)
                                    .on_click({
                                        let code = code.clone();
                                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                                    }),
                            )
                            .child(
                                Button::new("sc-copy-open")
                                    .variant(ButtonVariant::Outline)
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::ExternalLink)
                                    .label(t!("Copy code & open GitHub"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                        cx.open_url(&url);
                                    }),
                            ),
                    );
                }
                if let Some(error) = state.error {
                    body = body.child(tinted(error, c.destructive(), cx));
                }
                row.extend([body.into_any_element()]);
            }
            section.extend([row.into_any_element()]);
        }
        section
    }

    fn render_issue_trackers(&mut self, cx: &mut Context<Self>) -> SettingsSection {
        let snapshot = self.model.read(cx).snapshot().clone();
        let mut section = SettingsSection::new(t!("Issue trackers"));
        let Some(list) = snapshot.issue_trackers.clone() else {
            section.extend([SettingsRow::new(t!("Loading…")).into_any_element()]);
            return section;
        };
        for tracker in &list.trackers {
            let detail: SharedString = match (&tracker.account, tracker.connected) {
                (Some(account), true) => t!("Signed in as {account}", account = account.clone()),
                (None, true) => t!("Connected"),
                _ => SharedString::from(tracker.detail.clone().unwrap_or_default()),
            };
            section.extend([SettingsRow::new(SharedString::from(tracker.display_name.clone()))
                .description(detail)
                .control(
                    Badge::new(if tracker.connected { t!("Connected") } else { t!("Not connected") })
                        .variant(if tracker.connected { BadgeVariant::Success } else { BadgeVariant::Outline })
                        .size(BadgeSize::Sm),
                )
                .into_any_element()]);
        }
        if list.trackers.is_empty() {
            section.extend([SettingsRow::new(t!("No issue trackers")).into_any_element()]);
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
            .child(bool_setting(&model, "defaultAutoPull", t!("Automatically pull"), Some(t!("Pull the default branch before new threads start in its checkout.")), s.default_auto_pull.unwrap_or(false), lock(&["defaultAutoPull"]), |v| ClientServerSettingsPatch { default_auto_pull: Some(v), ..Default::default() }))
            .child(bool_setting(&model, "removeAgentCreditsOnMerge", t!("Remove agent credits when merging"), Some(t!("Remove recognized agent credit lines from merge and squash messages on hosts that support it, keeping human co-authors. Includes auto-merge. Excludes merge queues, stack merges, and existing commits.")), s.remove_agent_credits_on_merge.unwrap_or(false), lock(&["removeAgentCreditsOnMerge"]), |v| ClientServerSettingsPatch { remove_agent_credits_on_merge: Some(v), ..Default::default() }))
            .child(select_row("pullRequestMergeMethod", t!("Default merge method"), Some(t!("Pull requests start with this method.")), methods.iter().map(|m| merge_method_label(*m)).collect(), method_ix, lock(&["pullRequestMergeMethod"]), move |ix, _, cx| {
                let patch: ClientServerSettingsPatch = match methods[ix] {
                    Some(m) => ClientServerSettingsPatch { pull_request_merge_method: Some(m), ..Default::default() },
                    None => ClientServerSettingsPatch::default(),
                };
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
        let guide_instructions = self.fields.get("reviewGuideInstructions", s.review_guide_instructions.clone().unwrap_or_default(), "", false, window, cx);
        let guide_read = guide_instructions.clone();
        let guide_model = model.clone();
        let writing = SettingsSection::new(t!("Commits & pull requests"))
            .child(select_row("sourceControlWritingStyle", t!("Commit and pull request writing"), Some(modes[mode_ix].2.clone()), modes.iter().map(|(_, l, _)| l.clone()).collect(), mode_ix, lock(&["sourceControlWritingStyle"]), move |ix, _, cx| {
                let patch = ClientServerSettingsPatch { source_control_writing_style: Some(ClientServerSettingsPatchSourceControlWritingStyle { mode: Some(modes_static(ix)), ..Default::default() }), ..Default::default() };
                save_patch(&style_model, "sourceControlWritingStyle", patch, cx);
            }))
            .child(super::server_settings_controls::switch_row("followChangeRequestTemplates", t!("Follow pull request templates"), Some(t!("Use the repository's pull request template when it has one.")), follow, lock(&["sourceControlWritingStyle.followChangeRequestTemplates"]), move |v, _, cx| {
                let patch = ClientServerSettingsPatch { source_control_writing_style: Some(ClientServerSettingsPatchSourceControlWritingStyle { follow_change_request_templates: Some(v), ..Default::default() }), ..Default::default() };
                save_patch(&follow_model, "followChangeRequestTemplates", patch, cx);
            }))
            .child(model_select_row(&model, "sourceControlWriterModelSelection", t!("Source control writer model"), Some(t!("Model for commit messages, pull request text, and branch or bookmark names. When disabled, use the text generation model configured for this environment.")), s.source_control_writer_model_selection.as_ref(), Some(t!("Off")), lock(&["sourceControlWriterModelSelection"]), |v| ClientServerSettingsPatch { source_control_writer_model_selection: v, ..Default::default() }, cx))
            .child(bool_setting(&model, "reviewGuidesEnabled", t!("Review guides (beta)"), Some(t!("Add a Guide tab to pull requests. An agent explains the changes and points out decisions that need review.")), s.review_guides_enabled.unwrap_or(false), lock(&["reviewGuidesEnabled"]), |v| ClientServerSettingsPatch { review_guides_enabled: Some(v), ..Default::default() }))
            .child(model_select_row(&model, "reviewGuideModelSelection", t!("Review guide model"), Some(t!("Must be a coding agent: it reads the pull request over many steps. Off uses the writer model, or Codex or Claude if that model is a text generator.")), s.review_guide_model_selection.as_ref(), Some(t!("Off")), lock(&["reviewGuideModelSelection"]), |v| ClientServerSettingsPatch { review_guide_model_selection: v, ..Default::default() }, cx))
            .child(SettingsRow::new(t!("Review guide instructions")).description(t!("Your preferences for review guides: what to show first, dwell on, or skip. The guide's format and coverage stay the same.")).control(
                div().flex().items_center().gap(theme.sp(2.)).child(div().w(theme.sp(64.)).child(field_input(&guide_instructions, false, false, ro))).child(
                    Button::new("review-guide-save").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Save")).disabled(ro).on_click(move |_, _, cx| {
                        let value = guide_read.read(cx).value().trim().to_owned();
                        save_patch(&guide_model, "reviewGuideInstructions", ClientServerSettingsPatch { review_guide_instructions: Some(value), ..Default::default() }, cx);
                    }),
                ),
            ));
        let providers = self.render_providers(cx);
        let trackers = self.render_issue_trackers(cx);
        page_container(cx).child(providers).child(trackers).child(repositories).child(writing)
    }
}

fn modes_static(ix: usize) -> SourceControlWritingStyleMode {
    [SourceControlWritingStyleMode::RepoConventions, SourceControlWritingStyleMode::ConventionalCommits, SourceControlWritingStyleMode::Custom][ix]
}

