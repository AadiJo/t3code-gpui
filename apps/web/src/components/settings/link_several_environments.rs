//! "Link several environments" (`LinkSeveralEnvironments.tsx`, `linkBatch.ts`): links the
//! machines this app already reaches in one go. Each pairing code is minted on its machine with
//! this client's session there and passed straight to the linking environment; none is held,
//! shown or logged. Four links run at once; a failure stops only its own machine, and what
//! failed stays picked so linking again retries only those.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::StreamExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, EventEmitter, IntoElement, Render, SharedString, Task, TestSupportExt as _, Window, div,
    prelude::*,
};
use t3code_client_runtime::state::peer_pairing_code::mint_peer_pairing_code;
use t3code_contracts::rpc::methods::{PeerLinksLink, PeerLinksList};
use t3code_contracts::{
    AuthEnvironmentScope, AuthMcpClientAccess, AuthSessionState, EnvironmentId, PeerLinkCreateInput, PeerLinkErrorReason, PeerLinkSummary,
    PeerLinksLinkError,
};

use super::connections_settings_access::{check_session, grants};
use super::linked_environments_settings::{LinkableMachine, access_picker, linkable_machines};
use super::settings_controls::environment_machine_icon;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::{CheckState, Checkbox};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::connection::ConnectionRuntime;
use crate::i18n::{t, t_rich};
use crate::live::rpc::RpcLink;
use crate::theme_color::with_alpha;

/// `LINK_BATCH_CONCURRENCY`: pairings a batch opens at once.
const CONCURRENCY: usize = 4;

/// What the dialog asks its section to do.
pub enum LinkSeveralEvent {
    /// Closed; `linked` when a link was made, so the section lists again.
    Close { linked: bool },
}

/// `LinkBatchBlock`: why a machine cannot be linked from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkBatchBlock {
    LinksUnknown,
    Linked,
    NotConnected,
    NoAccess,
    NoAddress,
}

/// `LinkBatchFailure`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkBatchFailure {
    Unreachable,
    CodeRefused,
    WrongEnvironment,
    NoCode,
    Other,
}

/// `LinkBatchOutcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkBatchOutcome {
    Linked,
    Failed { failure: LinkBatchFailure, detail: Option<String> },
}

/// `LinkListState`: whether the linking environment's own links are known yet.
#[derive(Clone)]
enum Links {
    Loading,
    Failed(SharedString),
    Loaded(Arc<[PeerLinkSummary]>),
}

fn block_label(block: LinkBatchBlock) -> SharedString {
    match block {
        LinkBatchBlock::LinksUnknown => t!("Not known yet whether it is linked"),
        LinkBatchBlock::Linked => t!("Already linked"),
        LinkBatchBlock::NotConnected => t!("This app is not connected to it"),
        LinkBatchBlock::NoAccess => t!("Your session there cannot create pairing codes"),
        LinkBatchBlock::NoAddress => t!("No address another machine can reach"),
    }
}

fn failure_label(failure: LinkBatchFailure) -> SharedString {
    match failure {
        LinkBatchFailure::Unreachable => t!("Unreachable"),
        LinkBatchFailure::CodeRefused => t!("Pairing code refused"),
        LinkBatchFailure::WrongEnvironment => t!("Another environment answered"),
        LinkBatchFailure::NoCode => t!("Could not get a pairing code there"),
        LinkBatchFailure::Other => t!("Could not link"),
    }
}

/// `linkFailure`: only the linking environment's own error passes its message through; anything
/// else may carry the request, and with it the pairing code.
fn link_failure(error: &t3code_client_runtime::rpc::client::RpcError<PeerLinksLinkError>) -> LinkBatchOutcome {
    use t3code_client_runtime::rpc::client::RpcError;
    match error {
        RpcError::Failure(PeerLinksLinkError::Link(error)) => {
            let failure = if error.answered_as.is_some() {
                LinkBatchFailure::WrongEnvironment
            } else {
                match error.reason {
                    PeerLinkErrorReason::Unreachable | PeerLinkErrorReason::NotAT3Environment => LinkBatchFailure::Unreachable,
                    PeerLinkErrorReason::PairingRejected => LinkBatchFailure::CodeRefused,
                    _ => LinkBatchFailure::Other,
                }
            };
            LinkBatchOutcome::Failed { failure, detail: Some(error.message.clone()) }
        }
        _ => LinkBatchOutcome::Failed { failure: LinkBatchFailure::Other, detail: None },
    }
}

/// The dialog for one linking environment.
pub struct LinkSeveralEnvironments {
    host: EnvironmentId,
    links: Links,
    /// Each machine's session, once read (`useEnvironmentsWithScope`).
    sessions: HashMap<EnvironmentId, Option<AuthSessionState>>,
    selected: HashSet<EnvironmentId>,
    access: AuthMcpClientAccess,
    outcomes: Vec<(EnvironmentId, LinkBatchOutcome)>,
    linking: HashSet<EnvironmentId>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<LinkSeveralEvent> for LinkSeveralEnvironments {}

impl LinkSeveralEnvironments {
    /// Opens the dialog: lists the host's links and reads each machine's session.
    pub fn new(host: EnvironmentId, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = ConnectionRuntime::state(cx);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        if state.read(cx).cloud.as_ref().is_some_and(|c| c.discovery.environments.is_empty() && !c.discovery.refreshing) {
            ConnectionRuntime::global(cx).refresh_relay(cx);
        }
        let mut this = Self {
            host,
            links: Links::Loading,
            sessions: HashMap::new(),
            selected: HashSet::new(),
            access: AuthMcpClientAccess::ApprovalRequired,
            outcomes: Vec::new(),
            linking: HashSet::new(),
            _tasks: Vec::new(),
        };
        this.refresh_links(cx);
        let ids: Vec<EnvironmentId> = this.machines(cx).into_iter().map(|m| m.environment_id).collect();
        for id in ids {
            if let Some(task) = check_session(&id, cx) {
                this._tasks.push(cx.spawn(async move |this, cx| {
                    let session = task.await.ok();
                    _ = this.update(cx, |this, cx| {
                        this.sessions.insert(id, session);
                        cx.notify();
                    });
                }));
            }
        }
        this
    }

    /// `links.refresh`.
    pub fn refresh_links(&mut self, cx: &mut Context<Self>) {
        let Some(link) = RpcLink::new(&self.host, cx) else { return };
        let call = link.call::<PeerLinksList>(Default::default());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = call.await;
            _ = this.update(cx, |this, cx| {
                this.links = match result {
                    Ok(list) => Links::Loaded(list.links.into()),
                    Err(error) => Links::Failed(error),
                };
                cx.notify();
            });
        }));
    }

    fn machines(&self, cx: &App) -> Vec<LinkableMachine> {
        let links = match &self.links {
            Links::Loaded(links) => links.clone(),
            _ => Arc::from([]),
        };
        linkable_machines(&self.host, &links, cx)
    }

    /// `linkBatchCandidates`: until the host's links load, none is offered, since linking one
    /// already linked would replace its working token.
    pub fn candidates(&self, cx: &App) -> Vec<(LinkableMachine, Option<LinkBatchBlock>)> {
        let loaded = matches!(self.links, Links::Loaded(_));
        self.machines(cx)
            .into_iter()
            .map(|machine| {
                let writable =
                    self.sessions.get(&machine.environment_id).cloned().flatten().is_some_and(|s| grants(&s, AuthEnvironmentScope::AccessWrite));
                let blocked = if !loaded {
                    Some(LinkBatchBlock::LinksUnknown)
                } else if machine.linked {
                    Some(LinkBatchBlock::Linked)
                } else if !machine.connected {
                    Some(LinkBatchBlock::NotConnected)
                } else if !writable {
                    Some(LinkBatchBlock::NoAccess)
                } else if machine.urls.is_empty() {
                    Some(LinkBatchBlock::NoAddress)
                } else {
                    None
                };
                (machine, blocked)
            })
            .collect()
    }

    fn outcome(&self, id: &EnvironmentId) -> Option<&LinkBatchOutcome> {
        self.outcomes.iter().rev().find(|(known, _)| known == id).map(|(_, outcome)| outcome)
    }

    fn selectable(&self, cx: &App) -> Vec<LinkableMachine> {
        self.candidates(cx)
            .into_iter()
            .filter(|(m, blocked)| blocked.is_none() && self.outcome(&m.environment_id) != Some(&LinkBatchOutcome::Linked))
            .map(|(m, _)| m)
            .collect()
    }

    fn chosen(&self, cx: &App) -> Vec<LinkableMachine> {
        self.selectable(cx).into_iter().filter(|m| self.selected.contains(&m.environment_id)).collect()
    }

    /// The outcome of each machine of the last batch.
    pub fn outcomes(&self) -> &[(EnvironmentId, LinkBatchOutcome)] {
        &self.outcomes
    }

    /// Ticks or unticks a machine.
    pub fn toggle(&mut self, id: EnvironmentId, checked: bool, cx: &mut Context<Self>) {
        if checked {
            self.selected.insert(id);
        } else {
            self.selected.remove(&id);
        }
        cx.notify();
    }

    /// "Select all", or "Select none" when all are picked.
    pub fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let selectable = self.selectable(cx);
        if self.chosen(cx).len() == selectable.len() {
            self.selected.clear();
        } else {
            self.selected = selectable.into_iter().map(|m| m.environment_id).collect();
        }
        cx.notify();
    }

    pub fn set_access(&mut self, access: AuthMcpClientAccess, cx: &mut Context<Self>) {
        self.access = access;
        cx.notify();
    }

    fn busy(&self) -> bool {
        !self.linking.is_empty()
    }

    /// `submit` (`runLinkBatch`): mints a code on each chosen machine and links it from the host.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let chosen = self.chosen(cx);
        if self.busy() || chosen.is_empty() {
            return;
        }
        let Some(host) = RpcLink::new(&self.host, cx) else { return };
        let runtime = ConnectionRuntime::global(cx);
        let jobs: Vec<_> = chosen
            .iter()
            .map(|m| (m.environment_id.clone(), m.label.clone(), m.urls.clone(), runtime.environment_link(&m.environment_id)))
            .collect();
        self.linking = chosen.iter().map(|m| m.environment_id.clone()).collect();
        let access = self.access;
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<(EnvironmentId, LinkBatchOutcome)>();
        let session_link = host.clone();
        let batch = host.run(async move {
            let session = session_link.session().await?;
            futures::stream::iter(jobs)
                .for_each_concurrent(CONCURRENCY, |(id, label, urls, mint_link)| {
                    let (session, sender) = (session.clone(), sender.clone());
                    async move {
                        let outcome = async {
                            let Some((url, alternates)) = urls.split_first() else {
                                return LinkBatchOutcome::Failed { failure: LinkBatchFailure::Unreachable, detail: None };
                            };
                            let code = match mint_link {
                                Some(link) => mint_peer_pairing_code(&link, access).await.ok(),
                                None => None,
                            };
                            let Some(code) = code else { return LinkBatchOutcome::Failed { failure: LinkBatchFailure::NoCode, detail: None } };
                            let input = PeerLinkCreateInput {
                                url: url.clone(),
                                alternate_urls: (!alternates.is_empty()).then(|| alternates.to_vec()),
                                pairing_code: code,
                                access,
                                expected_environment_id: Some(id.clone()),
                                expected_label: Some(label),
                            };
                            match session.client.call::<PeerLinksLink>(input).await {
                                Ok(_) => LinkBatchOutcome::Linked,
                                Err(error) => link_failure(&error),
                            }
                        }
                        .await;
                        _ = sender.unbounded_send((id, outcome));
                    }
                })
                .await;
            Ok(())
        });
        cx.notify();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let outcomes = cx.spawn({
                let this = this.clone();
                async move |cx| {
                    while let Some((id, outcome)) = receiver.next().await {
                        _ = this.update(cx, |this, cx| {
                            this.linking.remove(&id);
                            this.outcomes.retain(|(known, _)| *known != id);
                            this.outcomes.push((id, outcome));
                            cx.notify();
                        });
                    }
                }
            });
            let result = batch.await;
            outcomes.await;
            _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    // The host session never came: every pick failed the same way.
                    for id in std::mem::take(&mut this.linking) {
                        this.outcomes.push((id, LinkBatchOutcome::Failed { failure: LinkBatchFailure::Other, detail: Some(error.to_string()) }));
                    }
                }
                this.linking.clear();
                if this.outcomes.iter().any(|(_, o)| *o == LinkBatchOutcome::Linked) {
                    this.refresh_links(cx);
                }
                // What failed stays picked, so linking again retries only those.
                this.selected = this.outcomes.iter().filter(|(_, o)| *o != LinkBatchOutcome::Linked).map(|(id, _)| id.clone()).collect();
                cx.notify();
            });
        }));
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let linked = self.outcomes.iter().any(|(_, o)| *o == LinkBatchOutcome::Linked);
        cx.emit(LinkSeveralEvent::Close { linked });
    }
}

impl Render for LinkSeveralEnvironments {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let candidates = self.candidates(cx);
        let selectable = self.selectable(cx);
        let chosen = self.chosen(cx);
        let busy = self.busy();
        let mut body: Vec<AnyElement> = Vec::new();
        if candidates.is_empty() {
            body.push(
                div()
                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .child(t_rich!("This app knows no other machines. Add them under Environments first.").plain())
                    .into_any_element(),
            );
        } else {
            let all = chosen.len() == selectable.len() && !selectable.is_empty();
            let header = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.sp(2.))
                .child(div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(t_rich!("Machines").plain()))
                .child(
                    Button::new("link-several-all")
                        .size(ButtonSize::Xs)
                        .variant(ButtonVariant::GhostMuted)
                        .label(if all { t!("Select none") } else { t!("Select all") })
                        .disabled(busy || selectable.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_all(cx))),
                );
            let status: Option<AnyElement> = match &self.links {
                Links::Loaded(_) => None,
                Links::Loading => Some(
                    div()
                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(t_rich!("Checking which machines are already linked…").plain())
                        .into_any_element(),
                ),
                Links::Failed(error) => Some(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.sp(2.))
                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                        .child(
                            div().text_color(c.destructive()).child(t!(
                                "Could not check which machines are already linked, so none can be picked: {error}",
                                error = error.clone()
                            )),
                        )
                        .child(
                            Button::new("link-several-retry")
                                .size(ButtonSize::Xs)
                                .variant(ButtonVariant::Outline)
                                .label(t_rich!("Retry").plain())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.links = Links::Loading;
                                    this.refresh_links(cx);
                                })),
                        )
                        .into_any_element(),
                ),
            };
            let border = c.border;
            let rows = candidates.iter().enumerate().map(|(ix, (machine, blocked))| {
                let id = machine.environment_id.clone();
                let outcome = self.outcome(&id).cloned();
                let available = blocked.is_none() && outcome != Some(LinkBatchOutcome::Linked);
                let failed = matches!(outcome, Some(LinkBatchOutcome::Failed { .. }));
                let status: Option<SharedString> = if self.linking.contains(&id) {
                    Some(t!("Linking…"))
                } else {
                    match &outcome {
                        Some(LinkBatchOutcome::Linked) => Some(t!("Linked", context = "status of a machine that was just linked")),
                        Some(LinkBatchOutcome::Failed { failure, .. }) => Some(failure_label(*failure)),
                        None => blocked.map(block_label),
                    }
                };
                let detail = match &outcome {
                    Some(LinkBatchOutcome::Failed { detail: Some(detail), .. }) => Some(detail.clone()),
                    _ => None,
                };
                let checked = available && self.selected.contains(&id);
                let toggle_id = id.clone();
                div()
                    .id(SharedString::from(format!("link-several-{}", id.as_str())))
                    .when(ix > 0, |d| d.border_t_1().border_color(border))
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.5))
                    .px(theme.sp(3.))
                    .py(theme.sp(2.))
                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                    .when(available && !busy, |d| d.cursor_pointer())
                    .child(Checkbox::new(SharedString::from(format!("link-several-check-{}", id.as_str())), if checked { CheckState::Checked } else { CheckState::Unchecked }).disabled(busy || !available))
                    .child(div().flex_none().child(environment_machine_icon(machine.machine, theme.rem(1.), c.muted_foreground)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().font_weight(weight::MEDIUM).child(machine.label.clone()))
                            .children(detail.map(|d| div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(d))),
                    )
                    .children(status.map(|status| {
                        div()
                            .flex_none()
                            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                            .text_color(if failed { c.destructive() } else { c.muted_foreground })
                            .child(status)
                    }))
                    .when(available && !busy, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            let on = !this.selected.contains(&toggle_id);
                            this.toggle(toggle_id.clone(), on, cx);
                        }))
                    })
                    .test_support()
                    .into_any_element()
            });
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(1.5))
                    .child(header)
                    .children(status)
                    .child(
                        div()
                            .id("link-several-list")
                            .max_h(theme.rem(18.))
                            .overflow_y_scroll()
                            .rounded(theme.radii.lg)
                            .border_1()
                            .border_color(border)
                            .flex()
                            .flex_col()
                            .children(rows),
                    )
                    .into_any_element(),
            );
        }
        if !self.outcomes.is_empty() && !busy {
            let failed = self.outcomes.iter().filter(|(_, o)| *o != LinkBatchOutcome::Linked).count();
            let linked = self.outcomes.len() - failed;
            let summary = if failed == 0 {
                t!("{linked, plural, one {Linked # machine.} other {Linked # machines.}}", linked = linked as i64)
            } else {
                t!(
                    "Linked {linked} of {total}. The ones that failed stay selected, so you can try them again.",
                    linked = linked as i64,
                    total = self.outcomes.len() as i64
                )
            };
            body.push(
                div()
                    .id("link-several-summary")
                    .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .child(summary)
                    .test_support()
                    .into_any_element(),
            );
        }
        let this = cx.entity().downgrade();
        body.push(access_picker(&theme, "link-several-access", self.access, move |level, cx| {
            if this.read_with(cx, |this, _| this.busy()).unwrap_or(true) {
                return;
            }
            this.update(cx, |this, cx| this.set_access(level, cx)).ok();
        }));
        let mut surface = DialogSurface::new(t_rich!("Link several environments").plain())
            .max_width_rem(28.)
            .bare_footer()
            .description(
                t_rich!("Agents here can then launch, message and wait on threads on each machine you pick. Each appears under Authorized clients in that machine's Sharing & access, where it can be revoked.").plain(),
            )
            .children(body)
            .footer(
                Button::new("link-several-close")
                    .variant(ButtonVariant::Outline)
                    .label(if self.outcomes.is_empty() { t!("Cancel") } else { t!("Done") })
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            );
        if !(!self.outcomes.is_empty() && !busy && chosen.is_empty()) {
            surface = surface.footer(
                Button::new("link-several-submit")
                    .label(if busy {
                        t!("Linking…")
                    } else {
                        t!("{count, plural, one {Link # machine} other {Link # machines}}", count = chosen.len() as i64)
                    })
                    .disabled(busy || chosen.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            );
        }
        let surface = surface.on_close({
            let this = cx.entity().downgrade();
            move |_, cx| {
                this.update(cx, |this, cx| this.close(cx)).ok();
            }
        });
        let this = cx.entity().downgrade();
        Modal::dialog("link-several-dialog", div().id("link-several").occlude().w_full().max_w(theme.rem(28.)).child(surface).test_support(), move |_, cx| {
            this.update(cx, |this, cx| this.close(cx)).ok();
        })
    }
}
