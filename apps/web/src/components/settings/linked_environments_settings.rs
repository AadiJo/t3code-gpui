//! "Linked environments" on Settings → Environments. Ports `LinkedEnvironmentsSettings.tsx`,
//! `LinkAccessPicker.tsx`, `LinkMachineSelect.tsx` and the hosts and machines of
//! `state/peerLinks.ts` / `linkRequest.ts`: one section per connected environment that offers
//! links and lets this session read access, listing its links (`peerLinks.list`) with their
//! health, Forget (`peerLinks.unlink`), and the "Link an environment" dialog (`peerLinks.link`)
//! that mints a pairing code on a machine this client is connected to, or takes a pasted one.
//! The page shows them with [`sections`].

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::input::InputState;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, Global, IntoElement, Render, SharedString, Task, TestSupportExt as _,
    Window, div, prelude::*, px,
};
use t3code_client_runtime::connection::catalog::TargetRoute;
use t3code_client_runtime::state::peer_pairing_code::mint_peer_pairing_code;
use t3code_contracts::rpc::methods::{PeerLinksLink, PeerLinksList, PeerLinksUnlink};
use t3code_contracts::{
    AuthEnvironmentScope, AuthMcpClientAccess, AuthSessionState, EnvironmentId, EnvironmentMachineKind, PeerLinkCreateInput, PeerLinkRemoveInput,
    PeerLinkStatus, PeerLinkSummary,
};

use super::connections_settings_access::{check_session, grants};
use super::settings_controls::{environment_machine_icon, environment_machine_kind};
use super::settings_layout::{SettingsRow, SettingsSection};
use super::settings_search::setting_title;
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::connection_status_dot::ConnectionStatusDot;
use crate::components::icons::{IconName, IconSize};
use crate::components::mcp::use_in_another_app_dialog::access_config;
use crate::components::server_update_action::confirm_copy;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::TextInput;
use crate::components::ui::select::Select;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{CommandError, ConnectionRuntime, EnvironmentState};
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::{t, t_rich};
use crate::live::rpc::RpcLink;
use crate::theme_color::with_alpha;

/// `AuthMcpClientAccess.literals`, in order.
const LEVELS: [AuthMcpClientAccess; 5] = [
    AuthMcpClientAccess::ReadOnly,
    AuthMcpClientAccess::ApprovalRequired,
    AuthMcpClientAccess::AutoAcceptEdits,
    AuthMcpClientAccess::Auto,
    AuthMcpClientAccess::FullAccess,
];

/// Links expiring within this many days warn.
const EXPIRY_WARNING_DAYS: i64 = 5;

/// One section per host, kept while the page is shown.
#[derive(Default)]
struct LinkedSections(HashMap<EnvironmentId, Entity<LinkedEnvironmentsSection>>);

impl Global for LinkedSections {}

/// `usePeerLinkHosts`: connected environments that offer links (`offersLinkedEnvironments`).
/// Whether this session may read their access is checked by each section.
fn link_hosts(cx: &App) -> Vec<EnvironmentId> {
    let state = ConnectionRuntime::state(cx);
    state
        .read(cx)
        .environments
        .iter()
        .filter(|env| env.is_connected())
        .filter(|env| {
            ServerSettingsStore::config(cx, env.id()).is_some_and(|config| {
                config.environment.capabilities.peer_links == Some(true) && config.settings.linked_environments_enabled == Some(true)
            })
        })
        .map(|env| env.id().clone())
        .collect()
}

/// The page's "Linked environments" sections, in host order.
pub fn sections(cx: &mut App) -> Vec<AnyElement> {
    if !ConnectionRuntime::is_running(cx) {
        return Vec::new();
    }
    let hosts = link_hosts(cx);
    let many = hosts.len() > 1;
    let mut known = cx.try_global::<LinkedSections>().map(|s| s.0.clone()).unwrap_or_default();
    known.retain(|id, _| hosts.contains(id));
    let mut out = Vec::new();
    for (index, id) in hosts.iter().enumerate() {
        let section = known.entry(id.clone()).or_insert_with(|| cx.new(|cx| LinkedEnvironmentsSection::new(id.clone(), cx))).clone();
        section.update(cx, |section, _| {
            section.show_label = many;
            section.search_target = index == 0;
        });
        out.push(AnyView::from(section).into_any_element());
    }
    cx.set_global(LinkedSections(known));
    out
}

/// The section of one host, for scripted checks.
pub fn section(id: &EnvironmentId, cx: &App) -> Option<Entity<LinkedEnvironmentsSection>> {
    cx.try_global::<LinkedSections>()?.0.get(id).cloned()
}

#[derive(Clone)]
enum ListState {
    Loading,
    Failed(SharedString),
    Loaded(Arc<[PeerLinkSummary]>),
}

/// `LinkedEnvironmentsSettings` for one host.
pub struct LinkedEnvironmentsSection {
    environment_id: EnvironmentId,
    show_label: bool,
    search_target: bool,
    session: Option<AuthSessionState>,
    list: ListState,
    forgetting: Option<EnvironmentId>,
    confirm_forget: Option<PeerLinkSummary>,
    dialog: Option<Entity<LinkEnvironmentDialog>>,
    _tasks: Vec<Task<()>>,
}

impl LinkedEnvironmentsSection {
    fn new(environment_id: EnvironmentId, cx: &mut Context<Self>) -> Self {
        let mut this =
            Self { environment_id, show_label: false, search_target: false, session: None, list: ListState::Loading, forgetting: None, confirm_forget: None, dialog: None, _tasks: Vec::new() };
        if let Some(task) = check_session(&this.environment_id, cx) {
            this._tasks.push(cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    this.session = result.ok();
                    cx.notify();
                })
                .ok();
            }));
        }
        // Showing the panel asks again: links made elsewhere change nothing this client reads.
        this.refresh(cx);
        this
    }

    /// `peerLinks.list` again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(link) = RpcLink::new(&self.environment_id, cx) else { return };
        let call = link.call::<PeerLinksList>(Default::default());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                this.list = match result {
                    Ok(success) => ListState::Loaded(success.links.into()),
                    Err(error) => ListState::Failed(error),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// The links as last listed.
    pub fn links(&self) -> Option<Arc<[PeerLinkSummary]>> {
        match &self.list {
            ListState::Loaded(links) => Some(links.clone()),
            _ => None,
        }
    }

    /// The open "Link an environment" dialog.
    pub fn dialog(&self) -> Option<Entity<LinkEnvironmentDialog>> {
        self.dialog.clone()
    }

    fn may(&self, scope: AuthEnvironmentScope) -> bool {
        self.session.as_ref().is_some_and(|s| grants(s, scope))
    }

    /// Opens "Link an environment".
    pub fn open_link_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.environment_id.clone();
        let links = self.links().unwrap_or_else(|| Arc::from([]));
        let section = cx.entity().downgrade();
        let dialog = cx.new(|cx| LinkEnvironmentDialog::new(host, links, section, window, cx));
        self.dialog = Some(dialog);
        cx.notify();
    }

    fn close_dialog(&mut self, linked: bool, cx: &mut Context<Self>) {
        self.dialog = None;
        if linked {
            self.refresh(cx);
        }
        cx.notify();
    }

    /// "Forget": unlinks after the confirm.
    pub fn forget(&mut self, link: PeerLinkSummary, cx: &mut Context<Self>) {
        let Some(rpc) = RpcLink::new(&self.environment_id, cx) else { return };
        self.confirm_forget = None;
        self.forgetting = Some(link.environment_id.clone());
        cx.notify();
        let call = rpc.call::<PeerLinksUnlink>(PeerLinkRemoveInput { environment_id: link.environment_id.clone() });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                this.forgetting = None;
                if let Err(error) = result {
                    Toaster::push(ToastData::new(ToastKind::Error, t!("Could not forget the link")).description(error), cx);
                }
                this.refresh(cx);
            })
            .ok();
        }));
    }

    /// Asks before forgetting (`requestConfirmDialog`, destructive).
    pub fn ask_forget(&mut self, link: PeerLinkSummary, cx: &mut Context<Self>) {
        self.confirm_forget = Some(link);
        cx.notify();
    }

    fn render_row(&self, link: &PeerLinkSummary, can_unlink: bool, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let health = peer_link_health(link);
        let status: SharedString = match &health {
            Health::Expired => t!("Expired. Link it again with a new pairing code."),
            Health::Unreachable(detail) => detail.clone().map(SharedString::from).unwrap_or_else(|| t!("Not answering right now.")),
            Health::Failing(detail) => detail.clone().into(),
            Health::Reachable(Some(days)) => t!(
                "{days, plural, one {Expires in {days} day. Link it again to renew.} other {Expires in {days} days. Link it again to renew.}}",
                days = *days
            ),
            Health::Reachable(None) => t!("Reachable"),
        };
        let dot = match &health {
            Health::Reachable(None) => Some(theme.fixed.success),
            Health::Reachable(Some(_)) => Some(c.warning),
            Health::Expired | Health::Failing(_) => Some(c.destructive()),
            Health::Unreachable(_) => Some(with_alpha(c.muted_foreground, 0.3)),
        };
        let busy = self.forgetting.as_ref() == Some(&link.environment_id);
        let key = link.environment_id.as_str().to_owned();
        let forget_link = link.clone();
        div()
            .px(theme.sp(4.))
            .py(theme.sp(3.))
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.sp(3.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(1.))
                    .child(
                        div()
                            .flex()
                            .min_h(theme.sp(5.))
                            .items_center()
                            .gap(theme.sp(1.5))
                            .child(ConnectionStatusDot::new(dot, Some(status.clone()), SharedString::from(format!("linked-dot-{key}"))))
                            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(SharedString::from(link.label.clone()))),
                    )
                    .child(
                        div()
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(format!("{} · {status}", access_config(link.access).0)),
                    )
                    .child(
                        div()
                            .truncate()
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(with_alpha(c.muted_foreground, 0.8))
                            .child(link.urls.join(", ")),
                    ),
            )
            .when(can_unlink, |d| {
                d.child(
                    div().flex_none().child(
                        Button::new(SharedString::from(format!("linked-forget-{key}")))
                            .size(ButtonSize::Xs)
                            .variant(ButtonVariant::DestructiveOutline)
                            .label(if busy { t!("Forgetting…") } else { t!("Forget") })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.ask_forget(forget_link.clone(), cx))),
                    ),
                )
            })
            .into_any_element()
    }
}

/// `peerLinkHealth`.
enum Health {
    Reachable(Option<i64>),
    Unreachable(Option<String>),
    Failing(String),
    Expired,
}

fn peer_link_health(link: &PeerLinkSummary) -> Health {
    let now = jiff::Timestamp::now();
    let expires: Option<jiff::Timestamp> = link.expires_at.parse().ok();
    if link.status == PeerLinkStatus::Expired || expires.is_some_and(|at| at <= now) {
        return Health::Expired;
    }
    if link.status == PeerLinkStatus::Unreachable {
        return Health::Unreachable(link.last_error.clone());
    }
    if let Some(error) = &link.last_error {
        return Health::Failing(error.clone());
    }
    let days = expires.map(|at| (at.duration_since(now).as_secs_f64() / 86_400.).ceil() as i64);
    Health::Reachable(days.filter(|d| *d <= EXPIRY_WARNING_DAYS))
}

impl Render for LinkedEnvironmentsSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        // Hosts whose access this session may not read are left out (`useEnvironmentsWithScope`).
        if !self.may(AuthEnvironmentScope::AccessRead) {
            return div().into_any_element();
        }
        let label = ConnectionRuntime::state(cx).read(cx).environment(&self.environment_id).map(|e| e.entry.label.clone());
        let title = setting_title("linked-environments");
        let title: SharedString = match (&label, self.show_label) {
            (Some(label), true) => format!("{title} · {label}").into(),
            _ => title,
        };
        let can_link = self.may(AuthEnvironmentScope::AccessWrite);
        let mut section = SettingsSection::new(title);
        if can_link {
            section = section.header_action(
                Button::new(SharedString::from(format!("linked-link-{}", self.environment_id.as_str())))
                    .size(ButtonSize::Xs)
                    .icon(IconName::Plus)
                    .icon_size(IconSize::Custom(theme.rem(0.75)))
                    .label(t!("Link", context = "verb, links an environment"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_link_dialog(window, cx))),
            );
        }
        section = match &self.list {
            ListState::Failed(error) => section.child(SettingsRow::new(t!("Could not load linked environments")).description(error.clone())),
            ListState::Loading => section.child(SettingsRow::new(t!("Loading linked environments…"))),
            ListState::Loaded(links) if links.is_empty() => section.child(SettingsRow::new(t!("No linked environments")).description(t!(
                "Link another machine running T3 Code, such as a VPS, so agents here can launch and drive threads there."
            ))),
            ListState::Loaded(links) => {
                let links = links.clone();
                let mut section = section;
                for (ix, link) in links.iter().enumerate() {
                    let row = self.render_row(link, can_link, &theme, cx);
                    section = section.child(if ix > 0 { div().border_t_1().border_color(with_alpha(theme.colors.border, 0.5)).child(row).into_any_element() } else { row });
                }
                section
            }
        };
        let confirm = self.confirm_forget.clone().map(|link| {
            let (title, description) = confirm_copy(&t!(
                "Forget {label}? Agents here stop working there. Revoke \"T3 Code · …\" in {label}''s Sharing & access to end the session there too.",
                label = link.label.as_str()
            ));
            let cancel = cx.listener(|this, _, _, cx| {
                this.confirm_forget = None;
                cx.notify();
            });
            let forget = link.clone();
            let mut surface = DialogSurface::new(title)
                .footer(Button::new("linked-forget-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).on_click(cancel))
                .footer(
                    Button::new("linked-forget-confirm")
                        .variant(ButtonVariant::Destructive)
                        .label(t!("Confirm"))
                        .on_click(cx.listener(move |this, _, _, cx| this.forget(forget.clone(), cx))),
                );
            if let Some(description) = description {
                surface = surface.description(description);
            }
            let this = cx.entity().downgrade();
            Modal::dialog("linked-forget-dialog", surface, move |_, cx| {
                this.update(cx, |this, cx| {
                    this.confirm_forget = None;
                    cx.notify();
                })
                .ok();
            })
        });
        div()
            .id(if self.search_target { SharedString::from("linked-environments") } else { format!("linked-environments-{}", self.environment_id.as_str()).into() })
            .child(section)
            .children(confirm)
            .children(self.dialog.clone())
            .into_any_element()
    }
}

/// A machine to link to (`LinkableMachine`).
#[derive(Clone, Debug)]
pub struct LinkableMachine {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub machine: EnvironmentMachineKind,
    pub connected: bool,
    pub connect_only: bool,
    pub linked: bool,
    pub urls: Vec<String>,
}

fn is_loopback(url: &str) -> bool {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).is_some_and(|host| {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host == "localhost" || host.ends_with(".localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    })
}

/// `linkableMachines`: the saved machines other than the host, with every address this client
/// knows for them that another machine could reach, then those only T3 Connect lists.
pub fn linkable_machines(host: &EnvironmentId, links: &[PeerLinkSummary], cx: &App) -> Vec<LinkableMachine> {
    let linked: Vec<&EnvironmentId> = links.iter().filter(|l| l.status != PeerLinkStatus::Expired).map(|l| &l.environment_id).collect();
    let state = ConnectionRuntime::state(cx);
    let state = state.read(cx);
    let discovered = state.cloud.as_ref().map(|c| c.discovery.environments.as_slice()).unwrap_or_default();
    let saved: Vec<LinkableMachine> = state
        .environments
        .iter()
        .filter(|env| env.id() != host)
        .map(|env: &EnvironmentState| {
            let config = ServerSettingsStore::config(cx, env.id());
            let reported = config.iter().flat_map(|c| c.direct_endpoints.iter().flatten().map(|e| e.http_base_url.clone()));
            let routes = env.entry.routes.iter().filter(|r| r.target.route != TargetRoute::Relay).filter_map(|r| r.bearer_profile().map(|p| p.http_base_url.to_string()));
            let connect = discovered.iter().find(|d| &d.record.environment_id == env.id()).map(|d| d.record.endpoint.http_base_url.to_string());
            let mut urls: Vec<String> = Vec::new();
            for url in reported.chain(routes).chain(connect) {
                if !urls.contains(&url) && !is_loopback(&url) {
                    urls.push(url);
                }
            }
            LinkableMachine {
                environment_id: env.id().clone(),
                label: env.entry.label.clone(),
                machine: environment_machine_kind(env.id(), cx),
                connected: env.is_connected(),
                connect_only: false,
                linked: linked.contains(&env.id()),
                urls,
            }
        })
        .collect();
    let connect_only = discovered
        .iter()
        .filter(|d| &d.record.environment_id != host && !saved.iter().any(|m| m.environment_id == d.record.environment_id))
        .map(|d| LinkableMachine {
            environment_id: d.record.environment_id.clone(),
            label: d.record.label.clone(),
            machine: t3code_contracts::server::resolve_environment_machine_kind(None, None),
            connected: false,
            connect_only: true,
            linked: linked.contains(&&d.record.environment_id),
            urls: Some(d.record.endpoint.http_base_url.to_string()).filter(|u| !is_loopback(u)).into_iter().collect(),
        })
        .collect::<Vec<_>>();
    saved.into_iter().chain(connect_only).collect()
}

/// The machine choice: a listed machine, or an address typed in (`ANOTHER_ADDRESS`).
#[derive(Clone, Debug, PartialEq)]
enum MachineChoice {
    Machine(EnvironmentId),
    Address,
}

/// "Link an environment" (`LinkEnvironmentAction`'s dialog).
pub struct LinkEnvironmentDialog {
    host: EnvironmentId,
    links: Arc<[PeerLinkSummary]>,
    section: gpui_kit::WeakEntity<LinkedEnvironmentsSection>,
    choice: Option<MachineChoice>,
    address: Entity<InputState>,
    pairing_code: Entity<InputState>,
    access: AuthMcpClientAccess,
    busy: bool,
    mint_failed: Vec<EnvironmentId>,
    sessions: HashMap<EnvironmentId, Option<AuthSessionState>>,
    _tasks: Vec<Task<()>>,
}

impl LinkEnvironmentDialog {
    fn new(
        host: EnvironmentId,
        links: Arc<[PeerLinkSummary]>,
        section: gpui_kit::WeakEntity<LinkedEnvironmentsSection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("https://box.example.ts.net"));
        let pairing_code = cx.new(|cx| InputState::new(window, cx));
        cx.observe(&address, |_, _, cx| cx.notify()).detach();
        cx.observe(&pairing_code, |_, _, cx| cx.notify()).detach();
        // Connect hosts are listed only once something asked for them, so ask on open.
        let state = ConnectionRuntime::state(cx);
        if state.read(cx).cloud.as_ref().is_some_and(|c| c.discovery.environments.is_empty() && !c.discovery.refreshing) {
            ConnectionRuntime::global(cx).refresh_relay(cx);
        }
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            host,
            links,
            section,
            choice: None,
            address,
            pairing_code,
            access: AuthMcpClientAccess::ApprovalRequired,
            busy: false,
            mint_failed: Vec::new(),
            sessions: HashMap::new(),
            _tasks: Vec::new(),
        }
    }

    /// The address field, for scripted checks.
    pub fn address(&self) -> &Entity<InputState> {
        &self.address
    }

    /// The pairing code field, for scripted checks.
    pub fn pairing_code(&self) -> &Entity<InputState> {
        &self.pairing_code
    }

    fn machines(&self, cx: &App) -> Vec<LinkableMachine> {
        linkable_machines(&self.host, &self.links, cx)
    }

    /// Picks a machine by id, or the address field with `None`.
    pub fn choose(&mut self, machine: Option<EnvironmentId>, cx: &mut Context<Self>) {
        self.choice = Some(machine.clone().map_or(MachineChoice::Address, MachineChoice::Machine));
        if let Some(id) = machine
            && !self.sessions.contains_key(&id)
        {
            self.sessions.insert(id.clone(), None);
            if let Some(task) = check_session(&id, cx) {
                self._tasks.push(cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.sessions.insert(id, result.ok());
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }

    pub fn set_access(&mut self, access: AuthMcpClientAccess, cx: &mut Context<Self>) {
        self.access = access;
        cx.notify();
    }

    /// What the form resolves to now: the picked machine, whether it needs a typed address,
    /// whether this client can mint the code, and whether Link can run.
    fn resolve(&self, cx: &App) -> (Option<MachineChoice>, Option<LinkableMachine>, bool, bool, bool) {
        let machines = self.machines(cx);
        let selected = self.choice.clone().or_else(|| machines.is_empty().then_some(MachineChoice::Address));
        let machine = match &selected {
            Some(MachineChoice::Machine(id)) => machines.iter().find(|m| &m.environment_id == id).cloned(),
            _ => None,
        };
        let needs_address = selected.is_some() && machine.as_ref().is_none_or(|m| m.urls.is_empty());
        // This client mints a code there when connected with a session that may create pairing
        // links, and only for its own addresses: anything can answer at a typed one.
        let can_mint = machine.as_ref().is_some_and(|m| {
            !needs_address
                && ConnectionRuntime::state(cx).read(cx).environment(&m.environment_id).is_some_and(|e| e.is_connected())
                && self.sessions.get(&m.environment_id).cloned().flatten().is_some_and(|s| grants(&s, AuthEnvironmentScope::AccessWrite))
                && !self.mint_failed.contains(&m.environment_id)
        });
        let address = self.address.read(cx).value().trim().to_owned();
        let code = self.pairing_code.read(cx).value().trim().to_owned();
        let ready = selected.is_some() && (!needs_address || !address.is_empty()) && (can_mint || !code.is_empty());
        (selected, machine, needs_address, can_mint, ready)
    }

    /// Link.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let (_, machine, needs_address, can_mint, ready) = self.resolve(cx);
        if self.busy || !ready {
            return;
        }
        let urls: Vec<String> = match (&machine, needs_address) {
            (Some(machine), false) => machine.urls.clone(),
            _ => vec![self.address.read(cx).value().trim().to_owned()],
        };
        let Some((url, alternates)) = urls.split_first().map(|(u, rest)| (u.clone(), rest.to_vec())) else { return };
        let Some(host) = RpcLink::new(&self.host, cx) else { return };
        let mint_link = machine.as_ref().filter(|_| can_mint).and_then(|m| ConnectionRuntime::global(cx).environment_link(&m.environment_id));
        let typed_code = self.pairing_code.read(cx).value().trim().to_owned();
        let access = self.access;
        // A typed address links alone: only this app's own addresses name a machine.
        let expected = machine.as_ref().filter(|_| !needs_address).map(|m| (m.environment_id.clone(), m.label.clone()));
        let target = machine.as_ref().map(|m| m.environment_id.clone());
        self.busy = true;
        cx.notify();
        let minted = ConnectionRuntime::global(cx).spawn(
            async move {
                match mint_link {
                    Some(link) => mint_peer_pairing_code(&link, access).await.map_err(CommandError::from),
                    None => Ok(typed_code),
                }
            },
            cx,
        );
        self._tasks.push(cx.spawn(async move |this, cx| {
            let code = match minted.await {
                Ok(code) => code,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        if let Some(target) = target {
                            this.mint_failed.push(target);
                        }
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not link the environment")).description(error.0), cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let input = PeerLinkCreateInput {
                url,
                alternate_urls: (!alternates.is_empty()).then_some(alternates),
                pairing_code: code,
                access,
                expected_environment_id: expected.as_ref().map(|(id, _)| id.clone()),
                expected_label: expected.map(|(_, label)| label),
            };
            let result = host.call::<PeerLinksLink>(input).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(link) => {
                        Toaster::push(ToastData::new(ToastKind::Success, t!("Linked {label}", label = link.label.as_str())), cx);
                        this.section.update(cx, |section, cx| section.close_dialog(true, cx)).ok();
                    }
                    Err(error) => {
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not link the environment")).description(error), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.section.update(cx, |section, cx| section.close_dialog(false, cx)).ok();
    }
}

fn field_label(theme: &AppTheme, label: SharedString) -> AnyElement {
    div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.foreground()).child(label).into_any_element()
}

fn hint(theme: &AppTheme, text: impl Into<SharedString>) -> AnyElement {
    div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(text.into()).into_any_element()
}

impl Render for LinkEnvironmentDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let machines = self.machines(cx);
        let (selected, machine, needs_address, can_mint, ready) = self.resolve(cx);
        let mut body: Vec<AnyElement> = Vec::new();
        if !machines.is_empty() {
            // `LinkMachineSelect`: the machines, then "Another address…".
            let mut options: Vec<SharedString> = machines
                .iter()
                .map(|m| {
                    let badge = if m.linked {
                        format!(" · {}", t!("Linked"))
                    } else if m.connected {
                        String::new()
                    } else if m.connect_only {
                        " · T3 Connect".into()
                    } else {
                        format!(" · {}", t!("Not connected"))
                    };
                    SharedString::from(format!("{}{badge}", m.label))
                })
                .collect();
            options.push(t!("Another address…"));
            let index = match &selected {
                Some(MachineChoice::Machine(id)) => machines.iter().position(|m| &m.environment_id == id).unwrap_or(0),
                Some(MachineChoice::Address) => machines.len(),
                None => usize::MAX,
            };
            let value = match (&selected, &machine) {
                (Some(MachineChoice::Address), _) => t!("Another address…"),
                (Some(_), Some(m)) => m.label.clone().into(),
                _ => t!("Choose a machine"),
            };
            let ids: Vec<EnvironmentId> = machines.iter().map(|m| m.environment_id.clone()).collect();
            let this = cx.entity().downgrade();
            let mut select = Select::new("link-machine", options, index.min(machines.len()), move |ix, _, cx| {
                let pick = ids.get(ix).cloned();
                this.update(cx, |this, cx| this.choose(pick, cx)).ok();
            })
            .value_label(value);
            if let Some(m) = &machine {
                select = select.leading(environment_machine_icon(m.machine, theme.rem(1.), with_alpha(c.foreground(), 0.8)));
            }
            body.push(div().flex().flex_col().gap(theme.sp(1.5)).child(field_label(&theme, t!("Machine"))).child(select).into_any_element());
        }
        if machine.as_ref().is_some_and(|m| m.linked) {
            body.push(hint(&theme, t!("Already linked. Linking again renews it with the access you pick.")));
        }
        if needs_address {
            let mut field = div().flex().flex_col().gap(theme.sp(1.5)).child(field_label(&theme, t!("Address"))).child(
                div().id("link-address").child(TextInput::new(&self.address).disabled(self.busy)).test_support(),
            );
            if let Some(m) = &machine {
                field = field.child(hint(
                    &theme,
                    t_rich!(
                        "This app has no address for {label} that another machine can reach. Enter its https or Tailscale address.",
                        label = m.label.as_str()
                    )
                    .plain(),
                ));
            }
            body.push(field.into_any_element());
            let address = self.address.read(cx).value().trim().to_owned();
            if !address.is_empty() {
                // `LinkAddressWarning`.
                let destination = url::Url::parse(&address).ok().map(|u| u.origin().ascii_serialization()).filter(|o| o != "null").unwrap_or(address);
                body.push(
                    div()
                        .rounded(theme.radii.xl)
                        .border_1()
                        .border_color(with_alpha(c.warning, 0.32))
                        .bg(c.warning_surface)
                        .px(theme.sp(3.5) - px(1.))
                        .py(theme.sp(3.) - px(1.))
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.warning_foreground)
                        .child(div().font_weight(weight::MEDIUM).child(t!("Check this address")))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .text_color(with_alpha(c.warning_foreground, 0.8))
                                .child(t_rich!("This app cannot confirm it is one of your machines. The pairing code you paste is sent there.").plain())
                                .child(destination),
                        )
                        .into_any_element(),
                );
            }
        }
        if selected.is_some() && !can_mint {
            let mut field = div().flex().flex_col().gap(theme.sp(1.5)).child(field_label(&theme, t!("Pairing code"))).child(
                div().id("link-pairing-code").child(TextInput::new(&self.pairing_code).disabled(self.busy)).test_support(),
            );
            if let Some(m) = &machine {
                field = field.child(hint(
                    &theme,
                    t_rich!(
                        "Create a link on {label}: Settings → Sharing & access → Authorized clients → Create link, then paste its code here.",
                        label = m.label.as_str()
                    )
                    .plain(),
                ));
            }
            body.push(field.into_any_element());
        }
        // `LinkAccessPicker`.
        let (label, description, icon) = access_config(self.access);
        let options: Vec<SharedString> = LEVELS.iter().map(|level| access_config(*level).0).collect();
        let selected_ix = LEVELS.iter().position(|l| *l == self.access).unwrap_or(1);
        let this = cx.entity().downgrade();
        body.push(
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(1.5))
                .child(field_label(&theme, t!("What agents here may do there")))
                .child(
                    Select::new("link-access", options, selected_ix, move |ix, _, cx| {
                        this.update(cx, |this, cx| this.set_access(LEVELS[ix], cx)).ok();
                    })
                    .value_label(label)
                    .leading(crate::components::icons::Icon::new(icon).size(IconSize::Md)),
                )
                .child(hint(
                    &theme,
                    format!(
                        "{description} {}",
                        t_rich!("An agent never gets more access there than its own mode here. Below full access, it can change only threads this link started, not that machine's own threads, projects or settings.").plain()
                    ),
                ))
                .into_any_element(),
        );
        let surface = DialogSurface::new(t!("Link an environment"))
            .max_width_rem(28.)
            .bare_footer()
            .description(
                t_rich!("Agents here can then launch, message and wait on threads there. It appears under Authorized clients in that environment's Sharing & access, where it can be revoked.").plain(),
            )
            .children(body)
            .footer(Button::new("link-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.cancel(cx))))
            .footer(
                Button::new("link-submit")
                    .label(if self.busy { t!("Linking…") } else { t!("Link", context = "verb, links an environment") })
                    .disabled(self.busy || !ready)
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            )
            .on_close({
                let this = cx.entity().downgrade();
                move |_, cx| {
                    this.update(cx, |this, cx| this.cancel(cx)).ok();
                }
            });
        let this = cx.entity().downgrade();
        Modal::dialog("link-environment-dialog", div().id("link-environment").occlude().w_full().max_w(theme.rem(28.)).child(surface), move |_, cx| {
            this.update(cx, |this, cx| this.cancel(cx)).ok();
        })
    }
}
