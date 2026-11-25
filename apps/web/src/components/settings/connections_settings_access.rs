//! Settings → Sharing & access: `ConnectionsSettings view="access"` for a
//! remote environment. A section named after the environment with its
//! machine icon and a "More actions" menu (the environment icon), the
//! preview tunnel row, the session's permissions, and the folded
//! "Authorized clients" section from `subscribeAuthAccess`.
//!
//! The Authorized clients actions, per-client revoke and T3 Connect devices
//! are in `connections_settings_access_clients`.
//!
//! Left out: Version, Network access, Tailscale HTTPS, WSL and endpoint
//! rows, which the web shows only for the local backend this remote-only
//! app does not run; and the T3 Connect link row.

use gpui_kit::{AnyElement, App, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window, div, prelude::*};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::{
    AuthAccessSnapshot, AuthAccessStreamEvent, AuthClientSession, AuthEnvironmentScope, AuthSessionState, ClientServerSettingsPatch,
    EnvironmentMachineKind,
};

use super::server_settings_controls::{bool_setting, lock_reason};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::{SettingsSection, page_container};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::managed_policy::{ManagedCapability, managed_by, managed_capability_denial};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// What the session check found (`useEnvironmentSessionState`).
#[derive(Clone)]
pub(super) enum SessionCheck {
    Unavailable,
    Checking,
    Failed,
    Done(Box<AuthSessionState>),
}

/// The access page.
pub struct AccessSettingsPanel {
    model: Entity<ServerPanelsModel>,
    permissions_open: bool,
    clients_open: bool,
    pub(super) session: SessionCheck,
    pub(super) access: Option<AuthAccessSnapshot>,
    pub(super) clients: super::connections_settings_access_clients::ClientsUi,
    /// Codes of pairing links this client created, shown on their rows.
    pub(super) created_credentials: std::collections::HashMap<String, SharedString>,
    _tasks: Vec<Task<()>>,
    _observe: Subscription,
}

/// `AUTH_SCOPE_OPTIONS`: the permissions the list shows, in order.
fn scope_options() -> [(AuthEnvironmentScope, SharedString); 17] {
    use AuthEnvironmentScope as S;
    [
        (S::OrchestrationRead, t!("View environment")),
        (S::OrchestrationOperate, t!("Operate tasks")),
        (S::SettingsWrite, t!("Change environment settings")),
        (S::ProvidersManage, t!("Manage providers")),
        (S::EnvironmentMaintain, t!("Maintain environment")),
        (S::PreviewOperate, t!("Control previews")),
        (S::DiagnosticsRead, t!("View diagnostics and usage")),
        (S::TerminalRead, t!("View terminals")),
        (S::TerminalOperate, t!("Use terminals")),
        (S::SourceControlWrite, t!("Change source control")),
        (S::FilesystemRead, t!("Read files")),
        (S::FilesystemWrite, t!("Write files")),
        (S::AccessRead, t!("View access")),
        (S::AccessWrite, t!("Manage access")),
        (S::RelayRead, t!("View relay")),
        (S::RelayWrite, t!("Manage relay")),
        (S::RelayConfigure, t!("Configure relay (admin)")),
    ]
}

/// `SessionPermissions`: what this session may do on the environment, two
/// columns of scope and state, or why that is not known. `route_context`
/// says the list is the active route's (a saved environment's row).
pub(super) fn session_permissions(session: &SessionCheck, route_context: bool, cx: &App) -> AnyElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let text = |s: SharedString| div().ui_text(theme, TextSize::Xs, weight::NORMAL).child(s);
    match session {
        SessionCheck::Done(session) if session.authenticated => {
            let items: Vec<AnyElement> = scope_options()
                .into_iter()
                .map(|(scope, title)| {
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(theme.sp(3.))
                        .child(text(title))
                        .child(text(if grants(session, scope) { t!("Allowed", context = "permission state") } else { t!("Not granted") }).text_color(c.muted_foreground))
                        .into_any_element()
                })
                .collect();
            let mut rows = Vec::new();
            let mut iter = items.into_iter();
            while let Some(left) = iter.next() {
                let right = iter.next();
                rows.push(div().flex().gap(theme.sp(6.)).child(div().flex_1().min_w_0().child(left)).child(div().flex_1().min_w_0().children(right)));
            }
            let note = if route_context {
                t!("These permissions apply to your active route. Other routes may have different permissions.")
            } else {
                t!("These permissions apply to this client’s current connection.")
            };
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(3.))
                .py(theme.sp(3.))
                .child(text(note).text_color(c.muted_foreground))
                .child(div().flex().flex_col().gap(theme.sp(2.)).children(rows))
                .into_any_element()
        }
        state => {
            let message = match state {
                SessionCheck::Unavailable => t!("Connect to this environment to view this session’s permissions."),
                SessionCheck::Done(_) => t!("This environment did not accept your session. Reconnect to try again."),
                SessionCheck::Failed => t!("Could not verify your permissions. Retrying automatically."),
                SessionCheck::Checking => t!("Checking permissions…"),
            };
            div().py(theme.sp(3.)).child(text(message).text_color(c.muted_foreground)).into_any_element()
        }
    }
}

/// Asks the environment what this session may do; `None` without a runtime
/// or a link to it.
pub(crate) fn check_session(id: &EnvironmentId, cx: &App) -> Option<Task<Result<AuthSessionState, crate::connection::CommandError>>> {
    if !ConnectionRuntime::is_running(cx) {
        return None;
    }
    let link = ConnectionRuntime::global(cx).environment_link(id)?;
    Some(ConnectionRuntime::global(cx).spawn(
        async move { link.session_state().await.map_err(|e| crate::connection::CommandError(e.to_string().into())) },
        cx,
    ))
}

/// `sessionGrantsScope`, with the legacy parent scopes.
pub(crate) fn grants(session: &AuthSessionState, scope: AuthEnvironmentScope) -> bool {
    use AuthEnvironmentScope as S;
    if !session.authenticated {
        return false;
    }
    if let Some(permissions) = &session.permissions {
        return permissions.contains(&scope);
    }
    let scopes = session.scopes.as_deref().unwrap_or_default();
    if scopes.contains(&scope) {
        return true;
    }
    if session.auth.server_update_scope.is_some() {
        return false;
    }
    let parent = match scope {
        S::FilesystemRead | S::DiagnosticsRead => Some(S::OrchestrationRead),
        S::SettingsWrite | S::ProvidersManage | S::EnvironmentMaintain | S::PreviewOperate | S::SourceControlWrite | S::FilesystemWrite => {
            Some(S::OrchestrationOperate)
        }
        S::TerminalRead => Some(S::TerminalOperate),
        _ => None,
    };
    parent.is_some_and(|parent| scopes.contains(&parent))
}

/// The access page on screen, for scripted checks.
struct ShownAccessPanel(gpui_kit::WeakEntity<AccessSettingsPanel>);

impl gpui_kit::Global for ShownAccessPanel {}

fn shown_panel(cx: &App) -> Option<Entity<AccessSettingsPanel>> {
    cx.try_global::<ShownAccessPanel>()?.0.upgrade()
}

/// The access state the shown page follows.
pub fn shown_access(cx: &App) -> Option<AuthAccessSnapshot> {
    shown_panel(cx)?.read(cx).access.clone()
}

/// The code of the pairing link labelled `label` that the shown page created.
pub fn created_credential(label: &str, cx: &App) -> Option<String> {
    let panel = shown_panel(cx)?;
    let panel = panel.read(cx);
    let link = panel.access.as_ref()?.pairing_links.iter().find(|l| l.label.as_deref() == Some(label))?;
    panel.created_credentials.get(&link.id).map(|c| c.to_string())
}

/// Unfolds the shown page's Authorized clients.
pub fn open_clients(cx: &mut App) {
    if let Some(panel) = shown_panel(cx) {
        panel.update(cx, |panel, cx| {
            panel.clients_open = true;
            cx.notify();
        });
    }
}

impl AccessSettingsPanel {
    pub fn new(model: Entity<ServerPanelsModel>, cx: &mut Context<Self>) -> Self {
        cx.set_global(ShownAccessPanel(cx.entity().downgrade()));
        let observe = cx.observe(&model, |_, _, cx| cx.notify());
        let mut this = Self {
            model,
            permissions_open: false,
            clients_open: false,
            session: SessionCheck::Unavailable,
            access: None,
            clients: Default::default(),
            created_credentials: Default::default(),
            _tasks: Vec::new(),
            _observe: observe,
        };
        this.start(cx);
        this
    }

    pub(super) fn environment_id(&self, cx: &App) -> EnvironmentId {
        EnvironmentId::from(self.model.read(cx).snapshot().environment_id.as_ref())
    }

    /// Checks the session's grants and follows `subscribeAuthAccess`.
    fn start(&mut self, cx: &mut Context<Self>) {
        if !ConnectionRuntime::is_running(cx) {
            return;
        }
        let id = self.environment_id(cx);
        let Some(link) = ConnectionRuntime::global(cx).environment_link(&id) else { return };
        self.session = SessionCheck::Checking;
        let session_link = link.clone();
        let check = ConnectionRuntime::global(cx).spawn(
            async move { session_link.session_state().await.map_err(|e| crate::connection::CommandError(e.to_string().into())) },
            cx,
        );
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                this.session = match result {
                    Ok(state) => SessionCheck::Done(Box::new(state)),
                    Err(_) => SessionCheck::Failed,
                };
                cx.notify();
            })
            .ok();
        }));
        // Every access change follows, so revokes and device requests show as they happen.
        let mut events = super::connections_settings_access_clients::follow_access(link, cx);
        self._tasks.push(cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| {
                        super::connections_settings_access_clients::apply_access_event(&mut this.access, event);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn render_permissions(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let open = self.permissions_open;
        let summary = div()
            .id("access-permissions-toggle")
            .flex()
            .items_center()
            .gap(theme.sp(2.))
            .min_h(theme.sp(11.))
            .cursor_pointer()
            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
            .on_click(cx.listener(|this, _, _, cx| {
                this.permissions_open = !this.permissions_open;
                cx.notify();
            }))
            .child(t!("Permissions"))
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Xs).color(c.muted_foreground));
        let body = open.then(|| session_permissions(&self.session, false, cx));
        div().px(theme.sp(4.)).child(summary).children(body).into_any_element()
    }

    /// `FoldedSettingsSection` "Authorized clients": a card whose header
    /// toggles the client list and shows the count.
    fn render_clients(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let access = self.access.as_ref()?;
        let theme = &cx.app_theme().clone();
        let c = &theme.colors;
        let open = self.clients_open;
        // `summarizeAuthorizedClients`.
        let mut summary = vec![t!(
            "{count, plural, one {{count} client} other {{count} clients}}",
            count = access.client_sessions.len()
        )];
        if !access.pairing_links.is_empty() {
            summary.push(t!(
                "{count, plural, one {{count} pairing link} other {{count} pairing links}}",
                count = access.pairing_links.len()
            ));
        }
        let summary = summary.join(" · ");
        let mut sessions: Vec<AuthClientSession> = access.client_sessions.clone();
        sessions.sort_by(|a, b| b.current.cmp(&a.current).then(b.connected.cmp(&a.connected)));
        let divider = with_alpha(c.border, 0.5);
        let links: Vec<_> = access.pairing_links.iter().filter(|l| l.connect_device_request.is_none()).cloned().collect();
        let link_rows: Vec<AnyElement> = if open { links.iter().map(|l| self.render_pairing_link_row(l, theme, cx)).collect() } else { Vec::new() };
        let header_action = self.may(AuthEnvironmentScope::AccessWrite).then(|| self.render_clients_header_action(cx));
        let revokes: Vec<Option<AnyElement>> = if open { sessions.iter().map(|s| self.render_client_revoke(s, cx)).collect() } else { Vec::new() };
        let mut revokes = revokes.into_iter();
        let rows = open.then(|| {
            div().border_t_1().border_color(divider).children(link_rows.into_iter().map(|r| div().border_b_1().border_color(divider).child(r))).children(sessions.into_iter().enumerate().map(|(ix, session)| {
                let label = session.client.label.clone().or(session.client.os.clone()).unwrap_or_else(|| session.subject.clone());
                let state = if session.current {
                    t!("This device")
                } else if session.connected {
                    t!("Connected")
                } else {
                    SharedString::from(session.last_connected_at.clone().unwrap_or_default())
                };
                let revoke = revokes.next().flatten();
                div()
                    .when(ix > 0, |d| d.border_t_1().border_color(divider))
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
                            .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(SharedString::from(label)))
                            .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(state)),
                    )
                    .children(revoke.map(|r| div().flex_none().child(r)))
            }))
        });
        Some(
            div()
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(with_alpha(c.border, 0.6))
                .bg(with_alpha(c.card(), 0.4))
                .shadow(theme.shadows.xs5.clone())
                .child(
                    div()
                        .id("access-clients-toggle")
                        .flex()
                        .items_center()
                        .gap(theme.sp(2.))
                        .min_h(theme.sp(11.))
                        .px(theme.sp(4.))
                        .py(theme.sp(2.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.clients_open = !this.clients_open;
                            cx.notify();
                        }))
                        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Md).color(c.muted_foreground))
                        .child(div().flex_none().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(t!("Authorized clients")))
                        .child(div().flex_1().min_w_0().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(summary))
                        .children(header_action.map(|a| div().flex_none().on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(a))),
                )
                .children(rows)
                .into_any_element(),
        )
    }
}

/// `EnvironmentMachineIcon`: Linux draws its brand mark, as the fork's `LinuxIcon`.
fn machine_icon(kind: EnvironmentMachineKind) -> crate::components::icons::IconSource {
    super::settings_controls::machine_icon_source(kind)
}

fn machine_label(kind: EnvironmentMachineKind) -> SharedString {
    let label = kind.as_str().split('-').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
    });
    SharedString::from(label.collect::<Vec<_>>().join(" "))
}

impl Render for AccessSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.model.read(cx).snapshot().clone();
        let s = &snapshot.settings;
        let policy = snapshot.managed_policy.as_deref();
        let ro = snapshot.read_only;
        let id = self.environment_id(cx);
        let config = ServerSettingsStore::config(cx, &id);
        let kind = s.environment_icon.unwrap_or(EnvironmentMachineKind::Desktop);
        let tunnel_capable = config.as_ref().is_some_and(|config| config.environment.capabilities.preview_tunnel_setting == Some(true));
        let tunnel_lock = managed_capability_denial(policy, ManagedCapability::PreviewTunnel)
            .map(SharedString::from)
            .or_else(|| lock_reason(managed_by(policy, &["enablePreviewTunnel"]), ro));
        let icon_model = self.model.clone();
        let menu = dropdown_menu(
            "access-machine-actions",
            Button::new("access-machine-actions-trigger").variant(ButtonVariant::GhostMuted).size(ButtonSize::IconXs).icon(IconName::Ellipsis),
            move |_, _| {
                EnvironmentMachineKind::ALL.iter().fold(MenuPopup::new(), |menu, option| {
                    let option = *option;
                    let model = icon_model.clone();
                    menu.item(MenuItem::new(machine_label(option)).icon(machine_icon(option)).kind(MenuItemKind::Radio(option == kind)).on_select(move |_, cx| {
                        super::server_settings_controls::save_patch(
                            &model,
                            "environmentIcon",
                            ClientServerSettingsPatch { environment_icon: Some(Some(option)), ..Default::default() },
                            cx,
                        );
                    }))
                })
            },
        );
        let mut section = SettingsSection::new(snapshot.environment_label.clone())
            .icon(Icon::new(machine_icon(kind)).size(IconSize::Md))
            .header_action(menu);
        if tunnel_capable {
            section = section.child(bool_setting(
                &self.model,
                "enablePreviewTunnel",
                t!("Preview tunnel"),
                Some(t!("Lets other devices open this machine's local servers: browser tabs loaded on the device, Open on this device, and port forwarding. Off: browser tabs stream from this machine.")),
                s.enable_preview_tunnel.unwrap_or(true),
                tunnel_lock,
                |v| ClientServerSettingsPatch { enable_preview_tunnel: Some(v), ..Default::default() },
            ));
        }
        section = section.child(self.render_permissions(cx));
        let clients = self.render_clients(cx);
        let devices = self.render_connect_devices(cx);
        let dialogs = self.render_access_dialogs(window.viewport_size().height, cx);
        page_container(cx).child(section).children(clients).children(devices).children(dialogs)
    }
}
