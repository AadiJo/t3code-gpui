//! "Use T3 Code in another app (MCP)". Ports `UseInAnotherAppDialog.tsx` and `ui/wizard.tsx`:
//! a gate that waits for T3 Connect sign-in, the environment's tunnel and a reachability check,
//! then three steps (App, Access, Connect) that hand the environment's MCP URL to an app and
//! mint a one-use code that approves the app's sign-in at the chosen level.

use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, TestSupportExt as _, Window, div, img, prelude::*, px,
};
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_client_runtime::relay::CloudAccountState;
use t3code_client_runtime::state::cloud_link::{after_failed_check, check_reachability, checked_at, read_link_state};
use t3code_client_runtime::state::peer_pairing_code::mint_mcp_approval_code;
use t3code_contracts::rpc::methods::PeerLinksList;
use t3code_contracts::{
    AuthEnvironmentScope, AuthMcpClientAccess, AuthSessionState, EnvironmentCloudLinkStateResult, EnvironmentConnectReachability,
    EnvironmentId, PeerLinkStatus, PeerLinkSummary, RuntimeMode, ServerConfig,
};

use super::mcp_apps::{MCP_APPS, McpApp, McpAppHandoff, mcp_handoff_action, other_mcp_app_steps};
use super::mcp_gate::{McpGate, McpGateInput, ReachabilityInput, RelayListing, RelayProblem, resolve_mcp_gate};
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, mono_font, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::managed_policy::{ManagedCapability, managed_capability_denial, managed_runtime_mode_denial};
use crate::components::settings::connections_settings_access::{check_session, grants};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{Modal, dialog_glass};
use crate::components::ui::scroll_area::scroll_area;
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{CommandError, ConnectionRuntime, ConnectionsState};
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::live::rpc::RpcLink;
use crate::theme_color::with_alpha;

/// What the dialog asks its host to do.
#[derive(Clone, Debug)]
pub enum UseInAnotherAppEvent {
    Close,
    /// "Sign in to T3 Connect": the sign-in is its own modal, so this dialog closes first.
    SignIn,
    /// "Manage access": Sharing & access on this environment.
    ManageAccess(EnvironmentId),
}

/// The app grid's choice.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Choice {
    App(usize),
    Other,
}

/// `check`: the gate opens on a check it ran, or on a newer one the link state reports. A failed
/// run counts only server checks newer than `since`; `None` waits for a retry.
#[derive(Clone, Debug)]
enum Check {
    Idle,
    Running,
    Done { result: Option<EnvironmentConnectReachability>, since: Option<String> },
}

#[derive(Clone, Debug)]
enum LinkStateLoad {
    Loading,
    Loaded(Box<EnvironmentCloudLinkStateResult>),
    Failed(SharedString),
}

/// A minted approval code.
#[derive(Clone, Debug)]
struct ApprovalCode {
    credential: SharedString,
    expires_at: jiff::Timestamp,
}

/// The wizard for one environment, or a gate message when there is none.
pub struct UseInAnotherAppDialog {
    requested: Option<EnvironmentId>,
    environment: Option<EnvironmentId>,
    step: usize,
    choice: Option<Choice>,
    picked: Option<AuthMcpClientAccess>,
    session: Option<AuthSessionState>,
    link_state: LinkStateLoad,
    check: Check,
    connecting: bool,
    discovery_asked: bool,
    another_way: bool,
    code: Option<ApprovalCode>,
    code_pending: bool,
    code_error: Option<SharedString>,
    copied: Option<SharedString>,
    links: Option<Vec<PeerLinkSummary>>,
    opened: Option<SharedString>,
    _poll: Option<Task<()>>,
    _clock: Option<Task<()>>,
    _copied_reset: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
    _observe: Subscription,
}

impl EventEmitter<UseInAnotherAppEvent> for UseInAnotherAppDialog {}

/// `DEFAULT_LEVELS`: the level the Access step starts at, Auto else the next one down.
const DEFAULT_LEVELS: [AuthMcpClientAccess; 4] =
    [AuthMcpClientAccess::Auto, AuthMcpClientAccess::AutoAcceptEdits, AuthMcpClientAccess::ApprovalRequired, AuthMcpClientAccess::ReadOnly];

/// `AuthMcpClientAccess.literals`, in order.
const LEVELS: [AuthMcpClientAccess; 5] = [
    AuthMcpClientAccess::ReadOnly,
    AuthMcpClientAccess::ApprovalRequired,
    AuthMcpClientAccess::AutoAcceptEdits,
    AuthMcpClientAccess::Auto,
    AuthMcpClientAccess::FullAccess,
];

/// `accessConfig`: each level's label, description and icon.
pub fn access_config(level: AuthMcpClientAccess) -> (SharedString, SharedString, IconName) {
    use AuthMcpClientAccess as A;
    match level {
        A::ReadOnly => (t!("Read only"), t!("Read projects and threads. Cannot start, message or change anything."), IconName::Eye),
        A::ApprovalRequired => (
            t!("Supervised", context = "permission mode, agent asks before acting"),
            t!("Ask before commands and file changes."),
            IconName::Lock,
        ),
        A::AutoAcceptEdits => (t!("Auto-accept edits"), t!("Auto-approve edits, ask before other actions."), IconName::PenLine),
        A::Auto => (
            t!("Auto", context = "permission mode, provider approves routine actions"),
            t!("Supported providers approve routine actions; others still ask."),
            IconName::ShieldCheck,
        ),
        A::FullAccess => (t!("Full access"), t!("Allow commands and edits without prompts."), IconName::ShieldOff),
    }
}

/// `mcpClientScopes`: the scopes an agent session gets at `level`.
fn mcp_client_scopes(level: AuthMcpClientAccess) -> &'static [AuthEnvironmentScope] {
    use AuthEnvironmentScope as S;
    match level {
        AuthMcpClientAccess::ReadOnly => &[S::OrchestrationRead],
        AuthMcpClientAccess::FullAccess => &[S::OrchestrationRead, S::OrchestrationOperate, S::SettingsWrite, S::SourceControlWrite],
        _ => &[S::OrchestrationRead, S::OrchestrationOperate],
    }
}

fn runtime_mode(level: AuthMcpClientAccess) -> Option<RuntimeMode> {
    Some(match level {
        AuthMcpClientAccess::ApprovalRequired => RuntimeMode::ApprovalRequired,
        AuthMcpClientAccess::AutoAcceptEdits => RuntimeMode::AutoAcceptEdits,
        AuthMcpClientAccess::Auto => RuntimeMode::Auto,
        AuthMcpClientAccess::FullAccess => RuntimeMode::FullAccess,
        _ => return None,
    })
}

fn level_rank(level: AuthMcpClientAccess) -> usize {
    LEVELS.iter().position(|l| *l == level).unwrap_or(0)
}

/// `useAccessEnvironment`: the environment `requested` resolves to among the enabled ones, and
/// its name (with an address when two share a label).
pub fn access_environment(requested: Option<&EnvironmentId>, state: &ConnectionsState) -> Option<(EnvironmentId, SharedString)> {
    let candidates: Vec<_> = state.environments.iter().filter(|e| Some(e.id()) == requested || e.entry.enabled).collect();
    let chosen = requested
        .and_then(|id| candidates.iter().find(|e| e.id() == id))
        .or_else(|| candidates.iter().find(|e| e.is_connected()))
        .or_else(|| candidates.first())?;
    let label = chosen.entry.label.clone();
    let duplicate = candidates.iter().any(|e| e.id() != chosen.id() && e.entry.label == label);
    let label = if duplicate { format!("{label} · {}", chosen.id().as_str()) } else { label };
    Some((chosen.id().clone(), label.into()))
}

impl UseInAnotherAppDialog {
    pub fn new(requested: Option<EnvironmentId>, cx: &mut Context<Self>) -> Self {
        let state = ConnectionRuntime::state(cx);
        let observe = cx.observe(&state, |this, _, cx| this.on_state(cx));
        let mut this = Self {
            requested,
            environment: None,
            step: 0,
            choice: None,
            picked: None,
            session: None,
            link_state: LinkStateLoad::Loading,
            check: Check::Idle,
            connecting: false,
            discovery_asked: false,
            another_way: false,
            code: None,
            code_pending: false,
            code_error: None,
            copied: None,
            links: None,
            opened: None,
            _poll: None,
            _clock: None,
            _copied_reset: None,
            _tasks: Vec::new(),
            _observe: observe,
        };
        this.on_state(cx);
        this
    }

    fn link(&self, cx: &App) -> Option<EnvironmentLink> {
        let id = self.environment.as_ref()?;
        ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(id)).flatten()
    }

    fn config(&self, cx: &App) -> Option<std::sync::Arc<ServerConfig>> {
        ServerSettingsStore::config(cx, self.environment.as_ref()?)
    }

    fn signed_in(cx: &App) -> Option<bool> {
        let state = ConnectionRuntime::state(cx);
        let cloud = state.read(cx).cloud.as_ref()?;
        match cloud.account {
            CloudAccountState::Loading => None,
            CloudAccountState::SignedOut => Some(false),
            CloudAccountState::SignedIn(_) => Some(true),
        }
    }

    fn connected(&self, cx: &App) -> bool {
        let Some(id) = &self.environment else { return false };
        ConnectionRuntime::state(cx).read(cx).environment(id).is_some_and(|e| e.is_connected())
    }

    /// Follows the environment list: resolves the environment, and once it is connected and
    /// signed in, loads what the gate reads.
    fn on_state(&mut self, cx: &mut Context<Self>) {
        if !ConnectionRuntime::is_running(cx) {
            return;
        }
        let state = ConnectionRuntime::state(cx);
        let resolved = access_environment(self.requested.as_ref(), state.read(cx)).map(|(id, _)| id);
        if resolved != self.environment {
            self.environment = resolved;
            self.session = None;
            self.link_state = LinkStateLoad::Loading;
            self.check = Check::Idle;
        }
        // T3 Connect lists machines only once something asks, and the gate reads that list.
        if Self::signed_in(cx) == Some(true) && !self.discovery_asked {
            self.discovery_asked = true;
            ConnectionRuntime::global(cx).refresh_relay(cx);
        }
        if self.connected(cx) && self.session.is_none() && self._tasks.is_empty() {
            self.load(cx);
        }
        self.sync_polling(cx);
        cx.notify();
    }

    /// Reads the session's grants, the link state and the environment's links.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.environment.clone() else { return };
        if let Some(task) = check_session(&id, cx) {
            self._tasks.push(cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    if let Ok(session) = result {
                        this.session = Some(session);
                    }
                    this.after_session(cx);
                    cx.notify();
                })
                .ok();
            }));
        }
        let offered = self.config(cx).is_some_and(|config| {
            config.environment.capabilities.peer_links == Some(true) && config.settings.linked_environments_enabled == Some(true)
        });
        if offered && let Some(link) = RpcLink::new(&id, cx) {
            let call = link.call::<PeerLinksList>(Default::default());
            self._tasks.push(cx.spawn(async move |this, cx| {
                let result = call.await;
                this.update(cx, |this, cx| {
                    this.links = result.ok().map(|success| success.links);
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    fn can_read_relay(&self) -> bool {
        self.session.as_ref().is_some_and(|s| grants(s, AuthEnvironmentScope::RelayRead))
    }

    fn after_session(&mut self, cx: &mut Context<Self>) {
        if self.can_read_relay() {
            self.refresh_link_state(cx);
        }
    }

    fn refresh_link_state(&mut self, cx: &mut Context<Self>) {
        let Some(link) = self.link(cx) else { return };
        let task = ConnectionRuntime::global(cx).spawn(async move { read_link_state(&link).await.map_err(CommandError::from) }, cx);
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.link_state = match result {
                    Ok(state) => LinkStateLoad::Loaded(Box::new(state)),
                    // A refresh that fails keeps what was read before.
                    Err(error) => match &this.link_state {
                        LinkStateLoad::Loaded(_) => return,
                        _ => LinkStateLoad::Failed(error.0),
                    },
                };
                this.maybe_check(cx);
                this.sync_polling(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn link_state(&self) -> Option<&EnvironmentCloudLinkStateResult> {
        match &self.link_state {
            LinkStateLoad::Loaded(state) => Some(state),
            _ => None,
        }
    }

    /// `canCheck`: asks the server to check its tunnel once one exists, once.
    fn maybe_check(&mut self, cx: &mut Context<Self>) {
        let can_check = self.connected(cx)
            && self.link_state().is_some_and(|state| t3code_client_runtime::state::cloud_link::is_managed_tunnel_active(state, None));
        if can_check && matches!(self.check, Check::Idle) {
            self.run_check(cx);
        }
    }

    /// `runReachabilityCheck`.
    fn run_check(&mut self, cx: &mut Context<Self>) {
        let Some(link) = self.link(cx) else { return };
        self.check = Check::Running;
        // A saved machine's address comes from T3 Connect's list, which can be stale.
        ConnectionRuntime::global(cx).refresh_relay(cx);
        let prior = self.link_state().cloned();
        let task = ConnectionRuntime::global(cx).spawn(
            async move {
                match check_reachability(&link).await {
                    Ok(result) => Ok(Ok(result)),
                    Err(_) => Ok(Err(read_link_state(&link).await.ok())),
                }
            },
            cx,
        );
        cx.notify();
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.check = match result {
                    Ok(Ok(result)) => {
                        let since = checked_at(&result).map(str::to_owned);
                        Check::Done { result: Some(result), since }
                    }
                    Ok(Err(latest)) => {
                        let (result, since) = after_failed_check(prior.as_ref(), latest.as_ref());
                        if let Some(latest) = latest {
                            this.link_state = LinkStateLoad::Loaded(Box::new(latest));
                        }
                        Check::Done { result, since }
                    }
                    Err(_) => Check::Done { result: None, since: None },
                };
                this.refresh_link_state(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn relay_listing<'a>(&self, state: &'a ConnectionsState) -> RelayListing<'a> {
        let Some(cloud) = &state.cloud else { return RelayListing::Loading };
        let discovery = &cloud.discovery;
        if !discovery.refreshing && discovery.error.is_some() {
            return RelayListing::Failed;
        }
        if !discovery.listed {
            return RelayListing::Loading;
        }
        let entry = self.environment.as_ref().and_then(|id| discovery.environments.iter().find(|e| &e.record.environment_id == id));
        RelayListing::Listed { endpoint: entry.map(|e| &e.record.endpoint), availability: entry.map(|e| e.availability) }
    }

    /// The gate as it stands now.
    pub fn gate(&self, cx: &App) -> McpGate {
        let state = ConnectionRuntime::state(cx);
        let state = state.read(cx);
        let config = self.config(cx);
        let reported = self.link_state().and_then(|s| s.reachability.clone());
        let reachability = match &self.check {
            Check::Idle | Check::Running => ReachabilityInput::Checking,
            Check::Done { result, since } => match (reported, since) {
                (Some(reported), Some(since)) if checked_at(&reported).is_some_and(|at| at > since.as_str()) => {
                    ReachabilityInput::Answer(Some(reported))
                }
                _ => ReachabilityInput::Answer(result.clone()),
            },
        };
        resolve_mcp_gate(McpGateInput {
            signed_in: Self::signed_in(cx),
            connected: self.connected(cx),
            approval_codes: config.as_ref().map(|c| c.environment.capabilities.mcp_approval_codes == Some(true)),
            // Until the session check answers, the gate waits on the link state below.
            can_read_relay: self.session.is_none() || self.can_read_relay(),
            link_state: self.link_state(),
            link_state_error: match &self.link_state {
                LinkStateLoad::Failed(error) => Some(error.clone()),
                _ => None,
            },
            relay: self.relay_listing(state),
            reachability,
        })
    }

    /// Polls the link state every 5 s while the tunnel starts or is offline; the server has no
    /// push for it.
    fn sync_polling(&mut self, cx: &mut Context<Self>) {
        let waiting = matches!(self.gate(cx), McpGate::Offline { .. }) || (self.connecting && matches!(self.gate(cx), McpGate::TurnOn));
        if !waiting {
            self._poll = None;
            return;
        }
        if self._poll.is_some() {
            return;
        }
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |this, cx| this.refresh_link_state(cx)).is_err() {
                    return;
                }
            }
        }));
    }

    /// `useGrantableLevels`: why each level cannot be granted here; absent when it can.
    fn grantable(&self, level: AuthMcpClientAccess, label: &str, cx: &App) -> Option<SharedString> {
        let config = self.config(cx);
        let policy = config.as_ref().and_then(|c| c.managed_policy.as_ref());
        // The server refuses codes for any level when its policy keeps remote clients out.
        if let Some(denial) = managed_capability_denial(policy, ManagedCapability::RemoteClients) {
            return Some(denial.into());
        }
        let Some(session) = &self.session else {
            return Some(t!("This device needs the Manage access permission on {environment}.", environment = label));
        };
        if !grants(session, AuthEnvironmentScope::AccessWrite) {
            return Some(t!("This device needs the Manage access permission on {environment}.", environment = label));
        }
        if !mcp_client_scopes(level).iter().all(|scope| grants(session, *scope)) {
            return Some(t!("This device cannot grant this on {environment}.", environment = label));
        }
        runtime_mode(level).and_then(|mode| managed_runtime_mode_denial(policy, mode)).map(SharedString::from)
    }

    fn access(&self, label: &str, cx: &App) -> Option<AuthMcpClientAccess> {
        self.picked.or_else(|| DEFAULT_LEVELS.iter().copied().find(|level| self.grantable(*level, label, cx).is_none()))
    }

    fn hosted_approval(&self, cx: &App) -> bool {
        self.config(cx).is_some_and(|c| c.environment.capabilities.mcp_hosted_approval == Some(true))
    }

    fn set_step(&mut self, step: usize, cx: &mut Context<Self>) {
        if step == 2 && self.step != 2 {
            self.another_way = self.choice == Some(Choice::Other);
        }
        self.step = step;
        cx.notify();
    }

    fn copy(&mut self, label: SharedString, value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = Some(label);
        self._copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// "Get code": mints on click; the code is never copied or sent anywhere by itself.
    fn mint(&mut self, access: AuthMcpClientAccess, name: SharedString, cx: &mut Context<Self>) {
        let Some(link) = self.link(cx) else { return };
        self.code_pending = true;
        self.code_error = None;
        cx.notify();
        let task = ConnectionRuntime::global(cx).spawn(async move { mint_mcp_approval_code(&link, access).await.map_err(CommandError::from) }, cx);
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.code_pending = false;
                match result.ok().and_then(|minted| {
                    let expires_at: jiff::Timestamp = minted.expires_at.parse().ok()?;
                    Some(ApprovalCode { credential: minted.credential.into(), expires_at })
                }) {
                    Some(code) => {
                        this.code = Some(code);
                        this.start_clock(cx);
                    }
                    None => this.code_error = Some(t!("Could not get a code from {environment}. Try again.", environment = name)),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// `useNowSecond`: redraws every second while a code counts down.
    fn start_clock(&mut self, cx: &mut Context<Self>) {
        self._clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let live = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.code.as_ref().is_some_and(|code| jiff::Timestamp::now() < code.expires_at)
                    })
                    .unwrap_or(false);
                if !live {
                    return;
                }
            }
        }));
    }

    /// The primary action's hand-off: copy first (so the app never gets an older clipboard),
    /// then open the installed app or the web page.
    fn hand_off(&mut self, app: McpApp, mcp_url: &str, cx: &mut Context<Self>) {
        let Some(action) = mcp_handoff_action(&app, mcp_url) else { return };
        if let Some(copy) = &action.copy {
            cx.write_to_clipboard(ClipboardItem::new_string(copy.text.clone()));
            let mut toast = ToastData::new(ToastKind::Success, copy.title.clone());
            // No platform reliably reports an app link nothing handles, so this keeps a way through.
            if action.app_link.is_some() {
                let link = action.link.clone();
                toast = toast.action(t!("Get {app}", app = app.name), move |_, cx| cx.open_url(&link));
            }
            Toaster::push(toast, cx);
        }
        let url = action.app_link.unwrap_or(action.link);
        cx.open_url(&url);
        self.opened = Some(url.into());
    }

    /// The step shown (0 App, 1 Access, 2 Connect).
    pub fn step(&self) -> usize {
        self.step
    }

    /// The level the Access step has selected.
    pub fn selected_access(&self, cx: &App) -> Option<AuthMcpClientAccess> {
        let (_, label) = access_environment(self.requested.as_ref(), ConnectionRuntime::state(cx).read(cx))?;
        self.access(&label, cx)
    }

    /// The approval code on screen.
    pub fn approval_code(&self) -> Option<SharedString> {
        self.code.as_ref().map(|code| code.credential.clone())
    }

    /// The last link the primary action opened.
    pub fn opened(&self) -> Option<SharedString> {
        self.opened.clone()
    }
}

impl Render for UseInAnotherAppDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let state = ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::state(cx));
        let resolved = state.as_ref().and_then(|s| access_environment(self.requested.as_ref(), s.read(cx)));
        let has_cloud = state.as_ref().is_some_and(|s| s.read(cx).cloud.is_some());
        let label = resolved.as_ref().map(|(_, label)| label.clone());
        let title: SharedString =
            format!("{} · {}", t!("Use T3 Code in another app (MCP)"), label.clone().unwrap_or_else(|| "T3 Code".into())).into();
        let body = match (&resolved, has_cloud) {
            (Some((id, label)), true) => self.render_wizard(&theme, title, id.clone(), label.clone(), cx),
            _ => {
                let message = if resolved.is_none() { t!("Connect to an environment first.") } else { t!("T3 Connect is not available in this build.") };
                vec![header(&theme, title, None), panel(&theme, true, vec![gate_message(&theme, message, Vec::new(), Vec::new())])]
            }
        };
        let content = dialog_glass(&theme)
            .id("use-in-another-app")
            .occlude()
            .w_full()
            .max_w(theme.rem(36.))
            .max_h(theme.rem(48.))
            .child(scroll_area("use-in-another-app-scroll").flex().flex_col().children(body))
            .child(
                div().absolute().top(theme.sp(2.)).right(theme.sp(2.)).child(
                    Button::new("use-in-another-app-close")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Icon)
                        .icon(IconName::X)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(UseInAnotherAppEvent::Close))),
                ),
            );
        let this = cx.entity().downgrade();
        Modal::dialog("use-in-another-app-modal", content, move |_, cx| {
            this.update(cx, |_, cx| cx.emit(UseInAnotherAppEvent::Close)).ok();
        })
    }
}

impl UseInAnotherAppDialog {
    fn render_wizard(&mut self, theme: &AppTheme, title: SharedString, id: EnvironmentId, name: SharedString, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let gate = self.gate(cx);
        let hosted = self.hosted_approval(cx);
        let steps: Vec<SharedString> = if hosted {
            vec![t!("App", context = "wizard step: choose the app"), t!("Connect", context = "wizard step: connect the app")]
        } else {
            vec![
                t!("App", context = "wizard step: choose the app"),
                t!("Access", context = "wizard step: choose the access level"),
                t!("Connect", context = "wizard step: connect the app"),
            ]
        };
        let shown = if hosted && self.step == 2 { 1 } else { self.step };
        let McpGate::Open { mcp_url } = gate.clone() else {
            return vec![header(theme, title, None), panel(theme, true, vec![self.render_gate(theme, &gate, &id, &name, cx)])];
        };
        let stepper = self.render_steps(theme, &steps, shown, hosted, cx);
        let access = self.access(&name, cx);
        let content = match self.step {
            0 => self.render_app_step(theme, cx),
            1 => self.render_access_step(theme, access, &name, cx),
            _ => match self.choice {
                Some(choice) if hosted || access.is_some() => {
                    self.render_connect_step(theme, &id, &name, &mcp_url, choice, if hosted { None } else { access }, cx)
                }
                _ => div().into_any_element(),
            },
        };
        let mut footer = div().flex().gap(theme.sp(2.)).px(theme.sp(6.)).py(theme.sp(4.));
        let buttons = div().flex().gap(theme.sp(2.));
        let buttons = match self.step {
            1 => buttons
                .child(
                    Button::new("mcp-back")
                        .variant(ButtonVariant::Outline)
                        .label(t!("Back"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_step(0, cx))),
                )
                .child(
                    Button::new("mcp-next")
                        .label(t!("Next"))
                        .disabled(access.is_none())
                        .on_click(cx.listener(|this, _, _, cx| this.set_step(2, cx))),
                ),
            2 => buttons.child(Button::new("mcp-done").label(t!("Done")).on_click(cx.listener(|_, _, _, cx| cx.emit(UseInAnotherAppEvent::Close)))),
            _ => buttons.child(
                Button::new("mcp-next").label(t!("Next")).disabled(self.choice.is_none()).on_click(cx.listener(move |this, _, _, cx| {
                    this.set_step(if hosted { 2 } else { 1 }, cx)
                })),
            ),
        };
        footer = if self.step == 2 {
            footer.justify_between().child(manage_access_button(id.clone(), None, cx)).child(buttons)
        } else {
            footer.justify_end().child(buttons)
        };
        vec![header(theme, title, Some(stepper)), panel(theme, false, vec![content]), footer.into_any_element()]
    }

    /// `WizardSteps`: equal cells in a tray; the current cell is a card. Later steps are disabled.
    fn render_steps(&self, theme: &AppTheme, steps: &[SharedString], current: usize, hosted: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let dark = theme.appearance.is_dark();
        let tint = |alpha: f32| if dark { with_alpha(gpui_kit::white(), alpha) } else { with_alpha(gpui_kit::black(), alpha) };
        div()
            .grid()
            .grid_cols(steps.len() as u16)
            .gap(theme.sp(1.))
            .p(theme.sp(1.))
            .rounded(theme.radii.xl)
            .bg(if dark { tint(0.04) } else { c.background() })
            .shadow(vec![ring(tint(0.05))])
            .children(steps.iter().enumerate().map(|(ix, label)| {
                let active = ix == current;
                let done = ix < current;
                let disabled = ix > current;
                div()
                    .id(SharedString::from(format!("mcp-step-{ix}")))
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .min_w_0()
                    .px(theme.sp(2.5))
                    .py(theme.sp(2.))
                    .rounded(theme.radii.lg)
                    .when(active, |d| d.bg(c.card()).shadow(vec![ring(tint(0.05))]))
                    .when(!disabled, |d| {
                        d.cursor_pointer().hover(|d| d.bg(c.card())).on_click(cx.listener(move |this, _, _, cx| {
                            this.set_step(if hosted && ix == 1 { 2 } else { ix }, cx)
                        }))
                    })
                    .child(
                        div()
                            .size(theme.sp(5.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .map(|d| {
                                if done {
                                    d.bg(c.primary()).shadow(vec![ring(c.primary())]).text_color(c.primary_foreground())
                                } else if active {
                                    d.bg(with_alpha(c.primary(), 0.1)).shadow(vec![ring(with_alpha(c.primary(), 0.3))]).text_color(c.primary())
                                } else {
                                    d.bg(if dark { tint(0.05) } else { c.card() }).shadow(vec![ring(tint(0.1))]).text_color(c.muted_foreground)
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
                    .test_support()
            }))
            .into_any_element()
    }

    fn render_gate(&self, theme: &AppTheme, gate: &McpGate, id: &EnvironmentId, name: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let retry = |id: &'static str, cx: &mut Context<Self>, on: fn(&mut Self, &mut Context<Self>)| {
            div().flex().child(Button::new(id).variant(ButtonVariant::Outline).label(t!("Check again")).on_click(cx.listener(move |this, _, _, cx| on(this, cx)))).into_any_element()
        };
        let offline_title = t!("{environment}''s T3 Connect is offline", environment = name);
        let cannot_check = t!("Can't check T3 Connect on {environment}", environment = name);
        let check_again = |cx: &mut Context<Self>| retry("mcp-check-again", cx, |this, cx| this.run_check(cx));
        match gate {
            McpGate::Checking => div()
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(Spinner::new(SpinnerSize::Sm).muted())
                .child(t!("Checking T3 Connect…"))
                .into_any_element(),
            McpGate::SignIn => gate_message(
                theme,
                t!("Sign in to T3 Connect"),
                vec![
                    t!("Apps like Poke and Claude run in the cloud. They need a public address to reach {environment}.", environment = name),
                    t!("T3 Connect gives {environment} that address. Each app still needs your approval.", environment = name),
                ],
                vec![
                    div()
                        .flex()
                        .child(
                            Button::new("mcp-sign-in")
                                .label(t!("Sign in to T3 Connect"))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(UseInAnotherAppEvent::SignIn))),
                        )
                        .into_any_element(),
                ],
            ),
            McpGate::Disconnected { relay: None } => gate_message(theme, t!("Connect to {environment} first.", environment = name), Vec::new(), Vec::new()),
            McpGate::Disconnected { relay: Some(problem) } => gate_message(
                theme,
                if *problem == RelayProblem::Offline { offline_title } else { cannot_check },
                Vec::new(),
                vec![retry("mcp-refresh-list", cx, |_, cx| ConnectionRuntime::global(cx).refresh_relay(cx))],
            ),
            McpGate::Update => gate_message(theme, t!("Update T3 Code on {environment}", environment = name), Vec::new(), Vec::new()),
            McpGate::NoRelayRead => gate_message(
                theme,
                cannot_check,
                vec![t!("This device needs the View relay permission on {environment}.", environment = name)],
                Vec::new(),
            ),
            McpGate::Error { message } => gate_message(theme, cannot_check, vec![message.clone()], vec![check_again(cx)]),
            McpGate::Offline { reachability } if !self.connecting => {
                let lines = reachability
                    .as_ref()
                    .map(|r| vec![SharedString::from(r.reason.clone()), t!("Fix: {fix}", fix = r.fix.as_str())])
                    .unwrap_or_default();
                gate_message(theme, if reachability.is_some() { offline_title } else { cannot_check }, lines, vec![check_again(cx)])
            }
            McpGate::TurnOn | McpGate::Offline { .. } => gate_message(
                theme,
                t!("Turn on T3 Connect for {environment}", environment = name),
                vec![
                    t!("{environment} gets a public https address, with no router setup.", environment = name),
                    t!("Only apps you approve can use it. You can turn it off in Settings."),
                ],
                vec![self.render_turn_on(theme, id, name, cx)],
            ),
            McpGate::Open { .. } => div().into_any_element(),
        }
    }

    /// `useT3ConnectTurnOn`: on a remote environment this client cannot run the tunnel's link
    /// flow, so it shows the policy reason or the commands to run on the host.
    fn render_turn_on(&self, theme: &AppTheme, _id: &EnvironmentId, name: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let config = self.config(cx);
        if let Some(reason) = managed_capability_denial(config.as_ref().and_then(|c| c.managed_policy.as_ref()), ManagedCapability::Connect) {
            return div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(reason).into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.))
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .child(self.command_block(theme, "t3 connect link".into(), "mcp-turn-on-command", cx))
            .child(div().child(crate::i18n::t_rich!("Restart T3 Code on {environment}.", environment = name.as_ref()).plain()))
            .child(div().flex().child(
                Button::new("mcp-turn-on-check").variant(ButtonVariant::Outline).label(t!("Check again")).on_click(cx.listener(|this, _, _, cx| {
                    this.refresh_link_state(cx);
                    this.run_check(cx);
                })),
            ))
            .into_any_element()
    }

    fn render_app_step(&self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let pressed_bg = with_alpha(c.primary(), 0.08);
        let logo_ring = if theme.appearance.is_dark() { with_alpha(gpui_kit::white(), 0.12) } else { with_alpha(gpui_kit::black(), 0.1) };
        let tiles = MCP_APPS.iter().enumerate().filter(|(_, app)| !app.hidden).map(|(ix, app)| {
            let pressed = self.choice == Some(Choice::App(ix));
            div()
                .id(SharedString::from(format!("mcp-app-{}", app.id)))
                .flex()
                .flex_col()
                .items_center()
                .gap(theme.sp(2.))
                .min_w_0()
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(if pressed { c.primary() } else { gpui_kit::transparent_black() })
                .px(theme.sp(1.) - px(1.))
                .py(theme.sp(3.) - px(1.))
                .cursor_pointer()
                .when(pressed, |d| d.bg(pressed_bg))
                .when(!pressed, |d| d.hover(|d| d.bg(c.highlight())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choice = Some(Choice::App(ix));
                    cx.notify();
                }))
                .child(img(app.logo).size(theme.sp(10.)).flex_none().rounded(theme.radii.lg).shadow(vec![ring(logo_ring)]))
                .child(div().max_w_full().truncate().ui_text(theme, TextSize::Xs, weight::MEDIUM).child(app.name))
                .test_support()
        });
        let other_pressed = self.choice == Some(Choice::Other);
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(3.))
            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(t!("Which app?")))
            .child(div().grid().grid_cols(4).gap(theme.sp(2.)).children(tiles))
            .child(
                div()
                    .id("mcp-app-other")
                    .w_full()
                    .flex()
                    .justify_center()
                    .rounded(theme.radii.lg)
                    .border_1()
                    .border_color(if other_pressed { c.primary() } else { c.border })
                    .px(theme.sp(3.) - px(1.))
                    .py(theme.sp(2.5) - px(1.))
                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                    .cursor_pointer()
                    .when(other_pressed, |d| d.bg(pressed_bg))
                    .when(!other_pressed, |d| d.hover(|d| d.bg(c.highlight())))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.choice = Some(Choice::Other);
                        cx.notify();
                    }))
                    .child(t!("Something else…"))
                    .test_support(),
            )
            .into_any_element()
    }

    fn render_access_step(&self, theme: &AppTheme, access: Option<AuthMcpClientAccess>, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let dark = theme.appearance.is_dark();
        let heading = match self.choice {
            Some(Choice::App(ix)) => t!("What can {app} do?", app = MCP_APPS[ix].name),
            _ => t!("What can the app do?"),
        };
        let options = LEVELS.iter().map(|level| {
            let level = *level;
            let (label, description, icon) = access_config(level);
            let reason = self.grantable(level, name, cx);
            let selected = Some(level) == access;
            let detail = reason.clone().unwrap_or(if level == AuthMcpClientAccess::FullAccess {
                t!("Runs commands and edits files without asking")
            } else {
                description
            });
            div()
                .id(SharedString::from(format!("mcp-access-{}", level.as_str())))
                .flex()
                .items_start()
                .gap(theme.sp(3.))
                .rounded(theme.radii.lg)
                .border_1()
                .px(theme.sp(3.) - px(1.))
                .py(theme.sp(2.5) - px(1.))
                .map(|d| match (selected, dark) {
                    (true, true) => d.border_color(gpui_kit::transparent_black()).bg(with_alpha(c.primary(), 0.1)).shadow(vec![inset_ring(with_alpha(c.primary(), 0.3), 1.)]),
                    (true, false) => d.border_color(c.primary()).bg(c.background()).shadow(vec![inset_ring(with_alpha(c.primary(), 0.25), 2.)]),
                    (false, true) => d.border_color(gpui_kit::transparent_black()).bg(with_alpha(gpui_kit::white(), 0.035)),
                    (false, false) => d.border_color(c.border).bg(c.background()),
                })
                .when(reason.is_none(), |d| {
                    d.cursor_pointer()
                        .when(!selected, |d| d.hover(|d| d.bg(if dark { c.highlight() } else { with_alpha(c.muted, 0.5) })))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.picked = Some(level);
                            cx.notify();
                        }))
                })
                .child(div().mt(theme.sp(0.5)).child(Icon::new(icon).size(IconSize::Md).color(if selected { c.primary() } else { c.muted_foreground })))
                .child(
                    div()
                        .min_w_0()
                        .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label))
                        .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(detail)),
                )
                .test_support()
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(3.))
            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(heading))
            .child(div().flex().flex_col().gap(theme.sp(3.)).children(options))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_connect_step(
        &self,
        theme: &AppTheme,
        id: &EnvironmentId,
        name: &SharedString,
        mcp_url: &SharedString,
        choice: Choice,
        access: Option<AuthMcpClientAccess>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let app = match choice {
            Choice::App(ix) => Some(MCP_APPS[ix]),
            Choice::Other => None,
        };
        let code = match access {
            None => div()
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(match app {
                    None => t!("When the app opens the sign-in page, choose what it may do and approve with your T3 Connect account."),
                    Some(app) => t!(
                        "When {app} opens the sign-in page, choose what it may do and approve with your T3 Connect account.",
                        app = app.name
                    ),
                })
                .into_any_element(),
            Some(access) => self.render_approval_code(theme, access, name.clone(), cx),
        };
        let linked = self.render_linked_line(theme, access, cx);
        let url_label = t!("URL");
        let section = div().flex().flex_col().gap(theme.sp(4.));
        if self.another_way || app.is_none() {
            let message = app.and_then(|app| match app.handoff {
                McpAppHandoff::CopyMessage { message, .. } => Some(message(mcp_url)),
                _ => None,
            });
            let steps = app.map(|app| (app.steps)()).unwrap_or_else(other_mcp_app_steps);
            return section
                .child(self.copy_row(theme, url_label, mcp_url.to_string(), cx))
                .children(message.map(|message| self.copy_row(theme, t!("Message", context = "message to send"), message, cx)))
                .child(div().flex().flex_col().gap(theme.sp(1.)).ui_text(theme, TextSize::Sm, weight::NORMAL).children(
                    steps.into_iter().enumerate().map(|(ix, step)| {
                        div()
                            .flex()
                            .child(div().w(theme.sp(5.)).flex_none().flex().justify_end().pr(theme.sp(1.5)).child(format!("{}.", ix + 1)))
                            .child(div().flex_1().min_w_0().child(step))
                    }),
                ))
                .child(match app {
                    None => div()
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(t!("Apps that require their own client ID cannot connect."))
                        .into_any_element(),
                    Some(app) => match app.docs {
                        Some(docs) => text_button(theme, "mcp-docs", t!("{app} help", app = app.name), move |_, cx| cx.open_url(docs)),
                        None => div().into_any_element(),
                    },
                })
                .child(code)
                .children(app.map(|app| {
                    let label = if matches!(app.handoff, McpAppHandoff::Commands { .. }) { t!("Back to commands") } else { t!("Back to one click") };
                    let this = cx.entity().downgrade();
                    text_button(theme, "mcp-back-to", label, move |_, cx| {
                        this.update(cx, |this, cx| {
                            this.another_way = false;
                            cx.notify();
                        })
                        .ok();
                    })
                }))
                .children(linked)
                .into_any_element();
        }
        let app = app.expect("checked above");
        let this = cx.entity().downgrade();
        let _ = id;
        section
            .child(self.render_primary_action(theme, app, mcp_url, cx))
            .child(self.copy_row(theme, url_label, mcp_url.to_string(), cx))
            .child(code)
            .child(text_button(theme, "mcp-another-way", t!("Set up another way"), move |_, cx| {
                this.update(cx, |this, cx| {
                    this.another_way = true;
                    cx.notify();
                })
                .ok();
            }))
            .children(linked)
            .into_any_element()
    }

    fn render_primary_action(&self, theme: &AppTheme, app: McpApp, mcp_url: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let note = app.note.map(|note| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(note()));
        let block = div().flex().flex_col().gap(theme.sp(2.));
        match mcp_handoff_action(&app, mcp_url) {
            None => {
                let commands = match app.handoff {
                    McpAppHandoff::Commands { commands } => commands(mcp_url),
                    _ => Vec::new(),
                };
                block
                    .children(
                        commands
                            .into_iter()
                            .enumerate()
                            .map(|(ix, command)| self.command_block(theme, command.into(), SharedString::from(format!("mcp-command-{ix}")), cx)),
                    )
                    .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!(
                        "Run these where {app} runs.",
                        app = app.name
                    )))
                    .children(note)
                    .into_any_element()
            }
            Some(action) => {
                let url = mcp_url.to_string();
                block
                    .child(div().id("mcp-primary").w_full().flex().flex_col().child(
                        Button::new("mcp-primary-action").label(action.label).on_click(cx.listener(move |this, _, _, cx| this.hand_off(app, &url, cx))),
                    ))
                    .children(note)
                    .into_any_element()
            }
        }
    }

    fn render_approval_code(&self, theme: &AppTheme, access: AuthMcpClientAccess, name: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let now = jiff::Timestamp::now();
        let live = self.code.as_ref().is_some_and(|code| now < code.expires_at);
        let (label, _, _) = access_config(access);
        let mut block = div().flex().flex_col().gap(theme.sp(1.5)).child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(theme.sp(2.))
                .child(
                    div().ui_text(theme, TextSize::Sm, weight::NORMAL).child(
                        crate::i18n::t_rich!("Code to approve{\" \"}<span>{access}</span>", access = label.as_ref()).styled(|tag| {
                            (tag == "span").then(|| gpui_kit::HighlightStyle { font_weight: Some(weight::MEDIUM), ..Default::default() })
                        }),
                    ),
                )
                .when(!live, |d| {
                    let button_label = if self.code_pending {
                        t!("Getting code…")
                    } else if self.code.is_none() {
                        t!("Get code")
                    } else {
                        t!("New code")
                    };
                    d.child(
                        Button::new("mcp-get-code")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .label(button_label)
                            .disabled(self.code_pending)
                            .on_click(cx.listener(move |this, _, _, cx| this.mint(access, name.clone(), cx))),
                    )
                }),
        );
        if let Some(code) = &self.code {
            if live {
                let left = code.expires_at.duration_since(now).as_secs().max(0);
                block = block.child(self.copy_row(theme, t!("Code", context = "approval code"), code.credential.to_string(), cx)).child(
                    div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!(
                        "Expires {time} · one use",
                        time = format!("{}:{:02}", left / 60, left % 60)
                    )),
                );
            } else {
                block = block.child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("The code expired.")));
            }
        }
        block
            .children(self.code_error.clone().map(|error| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive_foreground()).child(error)))
            .into_any_element()
    }

    /// `LinkedLine`: machines this environment links to, at the level a forwarded call gets.
    fn render_linked_line(&self, theme: &AppTheme, access: Option<AuthMcpClientAccess>, cx: &App) -> Option<AnyElement> {
        let links = self.links.as_ref().filter(|links| !links.is_empty())?;
        let state = ConnectionRuntime::state(cx);
        let state = state.read(cx);
        let named: Vec<String> = links
            .iter()
            .map(|link| {
                let name = state.environment(&link.environment_id).map(|e| e.entry.label.clone()).unwrap_or_else(|| link.label.clone());
                // A forwarded call keeps the lower of the app's level and the link's own.
                let level = match access {
                    Some(access) if level_rank(link.access) >= level_rank(access) => access,
                    _ => link.access,
                };
                let level = if link.status == PeerLinkStatus::Expired { t!("Expired") } else { access_config(level).0 };
                format!("{name} · {level}")
            })
            .collect();
        Some(
            div()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(theme.colors.muted_foreground)
                .child(t!("Linked: {environments}", environments = named.join(", ")))
                .into_any_element(),
        )
    }

    /// `CopyRow`.
    fn copy_row(&self, theme: &AppTheme, label: SharedString, value: String, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let copied = self.copied.as_ref() == Some(&label);
        let key = label.clone();
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.sp(2.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(c.border)
            .bg(with_alpha(c.muted, 0.5))
            .px(theme.sp(3.) - px(1.))
            .py(theme.sp(2.) - px(1.))
            .child(div().flex_none().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(label.clone()))
            .child(div().min_w_0().flex_1().truncate().font(mono_font(weight::NORMAL)).text_size(px(TextSize::Sm.size(theme.fonts.interface))).child(value.clone()))
            .child(
                Button::new(SharedString::from(format!("mcp-copy-{label}")))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(if copied { IconName::Check } else { IconName::Copy })
                    .icon_size(IconSize::Custom(theme.rem(0.875)))
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(key.clone(), value.clone(), cx))),
            )
            .into_any_element()
    }

    /// `CommandBlock`.
    fn command_block(&self, theme: &AppTheme, command: SharedString, id: impl Into<SharedString>, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let id: SharedString = id.into();
        let copied = self.copied.as_ref() == Some(&id);
        let key = id.clone();
        let text = command.to_string();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(theme.sp(3.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(c.border)
            .bg(with_alpha(c.muted, 0.5))
            .px(theme.sp(3.) - px(1.))
            .py(theme.sp(2.5) - px(1.))
            .font(mono_font(weight::NORMAL))
            .text_size(px(TextSize::Sm.size(theme.fonts.interface)))
            .child(div().min_w_0().truncate().child(div().flex().child(div().mr(theme.sp(2.)).text_color(c.muted_foreground).child("$")).child(command)))
            .child(
                Button::new(SharedString::from(format!("{id}-copy")))
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconXs)
                    .icon(if copied { IconName::Check } else { IconName::Copy })
                    .icon_size(IconSize::Custom(theme.rem(0.875)))
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(key.clone(), text.clone(), cx))),
            )
            .into_any_element()
    }
}

/// A 1px ring outside the box (`ring-1`).
fn ring(color: gpui_kit::Hsla) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow { color, offset: gpui_kit::point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false }
}

/// An inset ring of `width` (`ring-1 ring-inset`).
fn inset_ring(color: gpui_kit::Hsla, width: f32) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow { color, offset: gpui_kit::point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(width), inset: true }
}

/// `WizardHeader` (`DialogHeader`: `gap-2 p-6`, `pb-3` above a panel; the title is `leading-snug`).
fn header(theme: &AppTheme, title: SharedString, steps: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.sp(2.))
        .p(theme.sp(6.))
        .pb(theme.sp(3.))
        .child(
            div()
                .pr(theme.sp(6.))
                .ui_text(theme, TextSize::Xl, weight::SEMIBOLD)
                .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                .child(title),
        )
        .children(steps)
        .into_any_element()
}

/// `WizardPanel`: `space-y-4 px-6 py-5`, white/2 with a white/5 ring (`bg-background/80` and
/// black/5 in light).
fn panel(theme: &AppTheme, last: bool, children: Vec<AnyElement>) -> AnyElement {
    let dark = theme.appearance.is_dark();
    div()
        // The popup clips its children to its rounded corners.
        .when(last, |d| d.rounded_b(theme.radii.xxl - px(1.)))
        .flex()
        .flex_col()
        .gap(theme.sp(4.))
        .min_w_0()
        .px(theme.sp(6.))
        .py(theme.sp(5.))
        .bg(if dark { with_alpha(gpui_kit::white(), 0.02) } else { with_alpha(theme.colors.background(), 0.8) })
        .shadow(vec![ring(if dark { with_alpha(gpui_kit::white(), 0.05) } else { with_alpha(gpui_kit::black(), 0.05) })])
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .children(children)
        .into_any_element()
}

/// `GateMessage`: a title, muted lines and actions.
fn gate_message(theme: &AppTheme, title: SharedString, lines: Vec<SharedString>, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.sp(3.))
        .child(div().ui_text(theme, TextSize::Base, weight::MEDIUM).child(title))
        .when(!lines.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(1.))
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .children(lines.into_iter().map(|line| div().child(line))),
            )
        })
        .children(children.into_iter().map(|child| div().child(child)))
        .into_any_element()
}

/// `TextButton`: `text-sm font-medium`, underlined on hover.
fn text_button(theme: &AppTheme, id: &'static str, label: SharedString, on_click: impl Fn(&gpui_kit::ClickEvent, &mut App) + 'static) -> AnyElement {
    div()
        .flex()
        .child(
            div()
                .id(id)
                .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                .cursor_pointer()
                .hover(|d| d.underline())
                .on_click(move |event, _, cx| on_click(event, cx))
                .child(label)
                .test_support(),
        )
        .into_any_element()
}

/// `ManageAccessButton`: opens Sharing & access on this environment.
fn manage_access_button(id: EnvironmentId, label: Option<SharedString>, cx: &mut Context<UseInAnotherAppDialog>) -> AnyElement {
    Button::new("mcp-manage-access")
        .variant(ButtonVariant::Ghost)
        .label(label.unwrap_or_else(|| t!("Manage access")))
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(UseInAnotherAppEvent::ManageAccess(id.clone()))))
        .into_any_element()
}

/// Opens the dialog in a new entity (for hosts and scripted checks).
pub fn new_dialog(requested: Option<EnvironmentId>, cx: &mut App) -> Entity<UseInAnotherAppDialog> {
    cx.new(|cx| UseInAnotherAppDialog::new(requested, cx))
}
