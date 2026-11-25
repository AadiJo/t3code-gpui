//! The Authorized clients controls and T3 Connect devices of Settings → Sharing & access, as the
//! fork shows them for a remote environment. Ports `AuthorizedClientsHeaderAction` and
//! `ConnectedClientListRow` (per-client Revoke) from `AuthorizedClientsSettings.tsx`,
//! `ConnectDeviceApprovalSettings.tsx` and `RevokeConnectDeviceDialog.tsx`: Use in another app
//! (MCP), Revoke others, Create link, approving, denying and revoking devices, and the offer to
//! turn device approval on before a revoke that would not hold.

use std::rc::Rc;

use futures::StreamExt as _;
use gpui_kit::component::input::InputState;
use gpui_kit::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, SharedString, Task, TestSupportExt as _, Window, div, prelude::*};
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_client_runtime::state::connect_device_approval as access;
use t3code_contracts::{
    AuthAccessSnapshot, AuthAccessStreamEvent, AuthClientMetadata, AuthClientSession, AuthConnectDevice, AuthEnvironmentScope, AuthGrantScope,
    AuthPairingLink, AuthSessionId, ClientServerSettingsPatch,
};

use super::connections_settings_access::{AccessSettingsPanel, SessionCheck, grants};
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, mono_font, weight};
use crate::components::icons::{IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::{CheckState, Checkbox};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::TextInput;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{CommandError, ConnectionRuntime};
use crate::i18n::{t, t_rich};
use crate::theme_color::with_alpha;

/// The subject of a session a T3 Connect device holds (`CLOUD_CONNECT_SESSION_SUBJECT`).
pub const CLOUD_CONNECT_SESSION_SUBJECT: &str = "cloud-connect";

/// Follows `subscribeAuthAccess` on tokio and hands each event to the caller.
pub fn follow_access(link: EnvironmentLink, cx: &App) -> futures::channel::mpsc::UnboundedReceiver<AuthAccessStreamEvent> {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    ConnectionRuntime::global(cx).handle().spawn(async move {
        let Ok(session) = link.session(std::time::Duration::from_secs(10)).await else { return };
        let mut stream = session.client.subscribe::<t3code_contracts::rpc::methods::SubscribeAuthAccess>(Default::default());
        while let Some(Ok(chunk)) = stream.next().await {
            for event in chunk {
                if sender.unbounded_send(event).is_err() {
                    return;
                }
            }
        }
    });
    receiver
}

/// Applies one access event to the snapshot.
pub fn apply_access_event(snapshot: &mut Option<AuthAccessSnapshot>, event: AuthAccessStreamEvent) {
    match event {
        AuthAccessStreamEvent::Snapshot(event) => *snapshot = Some(event.payload),
        AuthAccessStreamEvent::PairingLinkUpserted(event) => {
            if let Some(s) = snapshot {
                s.pairing_links.retain(|l| l.id != event.payload.id);
                s.pairing_links.push(event.payload);
            }
        }
        AuthAccessStreamEvent::PairingLinkRemoved(event) => {
            if let Some(s) = snapshot {
                s.pairing_links.retain(|l| l.id != event.payload.id);
            }
        }
        AuthAccessStreamEvent::ClientUpserted(event) => {
            if let Some(s) = snapshot {
                s.client_sessions.retain(|c| c.session_id != event.payload.session_id);
                s.client_sessions.push(event.payload);
            }
        }
        AuthAccessStreamEvent::ClientRemoved(event) => {
            if let Some(s) = snapshot {
                s.client_sessions.retain(|c| c.session_id != event.payload.session_id);
            }
        }
        AuthAccessStreamEvent::Unknown(_) => {}
    }
}

/// `describeConnectDeviceClient`: how a device describes itself; the code is what to trust.
pub fn describe_connect_device(client: Option<&AuthClientMetadata>) -> SharedString {
    let mut bits: Vec<&str> = Vec::new();
    if let Some(client) = client {
        for bit in [&client.label, &client.os, &client.browser, &client.ip_address].into_iter().flatten() {
            if !bit.is_empty() && !bits.contains(&bit.as_str()) {
                bits.push(bit);
            }
        }
    }
    if bits.is_empty() { t!("Unknown device") } else { bits.join(" · ").into() }
}

/// `connectDeviceName`.
pub fn connect_device_name(client: Option<&AuthClientMetadata>) -> SharedString {
    client.and_then(|c| c.label.clone().or(c.os.clone()).or(c.browser.clone())).map(SharedString::from).unwrap_or_else(|| t!("this device"))
}

/// A revoke waiting on the "turn approval on" offer.
#[derive(Clone)]
pub struct PendingRevoke {
    pub title: SharedString,
    pub revoke: Rc<dyn Fn(&mut AccessSettingsPanel, &mut Context<AccessSettingsPanel>)>,
    pub turn_on_approval: bool,
    pub busy: bool,
}

/// What the clients controls hold between renders.
#[derive(Default)]
pub struct ClientsUi {
    pub revoking_session: Option<AuthSessionId>,
    pub revoking_others: bool,
    pub busy_device: Option<String>,
    pub pending_revoke: Option<PendingRevoke>,
    pub create: Option<CreateLinkDialog>,
    pub tasks: Vec<Task<()>>,
}

/// "Create pairing link".
pub struct CreateLinkDialog {
    pub label: Entity<InputState>,
    pub scopes: Vec<AuthGrantScope>,
    pub creating: bool,
}

/// `AuthStandardClientScopes`.
const STANDARD_SCOPES: [AuthGrantScope; 16] = [
    AuthGrantScope::OrchestrationRead,
    AuthGrantScope::OrchestrationOperate,
    AuthGrantScope::SettingsWrite,
    AuthGrantScope::ProvidersManage,
    AuthGrantScope::EnvironmentMaintain,
    AuthGrantScope::PreviewOperate,
    AuthGrantScope::DiagnosticsRead,
    AuthGrantScope::TerminalRead,
    AuthGrantScope::TerminalOperate,
    AuthGrantScope::SourceControlWrite,
    AuthGrantScope::FilesystemRead,
    AuthGrantScope::FilesystemWrite,
    AuthGrantScope::RelayRead,
    AuthGrantScope::AccessRead,
    AuthGrantScope::AccessWrite,
    AuthGrantScope::RelayWrite,
];

const READ_ONLY_SCOPES: [AuthGrantScope; 4] =
    [AuthGrantScope::OrchestrationRead, AuthGrantScope::FilesystemRead, AuthGrantScope::DiagnosticsRead, AuthGrantScope::TerminalRead];

/// `AUTH_SCOPE_OPTIONS`: each grantable scope's title and description, in order.
fn scope_option(scope: AuthGrantScope) -> (SharedString, SharedString) {
    use AuthGrantScope as S;
    match scope {
        S::OrchestrationRead => (t!("View environment"), t!("Read threads, status, checkpoints, and configuration.")),
        S::OrchestrationOperate => (t!("Operate tasks"), t!("Start, update, and stop tasks.")),
        S::SettingsWrite => (t!("Change environment settings"), t!("Edit environment preferences and keybindings.")),
        S::ProvidersManage => (t!("Manage providers"), t!("Configure, install, sign in to, and update providers and usage sources.")),
        S::EnvironmentMaintain => (t!("Maintain environment"), t!("Update the server and control environment processes.")),
        S::PreviewOperate => (t!("Control previews"), t!("Open browser previews and host browser automation.")),
        S::DiagnosticsRead => (t!("View diagnostics and usage"), t!("Read process diagnostics, resource history, and usage totals.")),
        S::TerminalRead => (t!("View terminals"), t!("Read existing terminal output and status.")),
        S::TerminalOperate => (t!("Use terminals"), t!("Create terminals and send input to running shells.")),
        S::SourceControlWrite => (t!("Change source control"), t!("Commit, push, manage branches and repositories, and change pull requests.")),
        S::FilesystemRead => (t!("Read files"), t!("Browse host files, search workspaces, and inspect local changes.")),
        S::FilesystemWrite => (t!("Write files"), t!("Edit workspace files and save plans to disk.")),
        S::AccessRead => (t!("View access"), t!("Inspect pairing links and authorized clients.")),
        S::AccessWrite => (t!("Manage access"), t!("Issue and revoke credentials for other clients.")),
        S::RelayRead => (t!("View relay"), t!("Inspect managed relay connectivity.")),
        S::RelayWrite => (t!("Manage relay"), t!("Update relay preferences or unlink T3 Connect.")),
        _ => (SharedString::from(scope.as_str().to_owned()), SharedString::default()),
    }
}

fn environment_scope(scope: AuthGrantScope) -> Option<AuthEnvironmentScope> {
    serde_json::from_value(serde_json::Value::String(scope.as_str().to_owned())).ok()
}

impl AccessSettingsPanel {
    fn link(&self, cx: &App) -> Option<EnvironmentLink> {
        ConnectionRuntime::is_running(cx).then(|| ConnectionRuntime::global(cx).environment_link(&self.environment_id(cx))).flatten()
    }

    pub(super) fn may(&self, scope: AuthEnvironmentScope) -> bool {
        matches!(&self.session, SessionCheck::Done(session) if grants(session, scope))
    }

    /// Runs `call` on tokio; reports a failure under `title`, then clears the busy marks.
    fn run_access(
        &mut self,
        title: SharedString,
        call: impl FnOnce(EnvironmentLink) -> futures::future::BoxFuture<'static, Result<(), String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(link) = self.link(cx) else { return };
        let task = ConnectionRuntime::global(cx).spawn(async move { call(link).await.map_err(CommandError::from) }, cx);
        self.clients.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.clients.revoking_session = None;
                this.clients.revoking_others = false;
                this.clients.busy_device = None;
                if let Err(error) = result {
                    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.0), cx);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn approval_off(&self, cx: &App) -> bool {
        crate::hooks::use_settings::ServerSettingsStore::config(cx, &self.environment_id(cx)).is_some_and(|c| c.settings.require_connect_device_approval == Some(false))
    }

    /// `requestConnectRevoke`: with approval off a revoked device signs straight back in, so
    /// offer to turn approval on first.
    fn request_connect_revoke(&mut self, title: SharedString, revoke: Rc<dyn Fn(&mut Self, &mut Context<Self>)>, cx: &mut Context<Self>) {
        if self.approval_off(cx) && self.may(AuthEnvironmentScope::AccessWrite) {
            self.clients.pending_revoke = Some(PendingRevoke { title, revoke, turn_on_approval: true, busy: false });
            cx.notify();
        } else {
            revoke(self, cx);
        }
    }

    pub fn revoke_session(&mut self, session_id: AuthSessionId, cx: &mut Context<Self>) {
        self.clients.revoking_session = Some(session_id.clone());
        self.run_access(t!("Could not revoke client access"), move |link| Box::pin(async move { access::revoke_client(&link, session_id).await.map_err(|e| e.to_string()) }), cx);
    }

    /// `requestRevokeClientSession`.
    pub fn request_revoke_session(&mut self, session: AuthClientSession, cx: &mut Context<Self>) {
        let id = session.session_id.clone();
        if session.subject == CLOUD_CONNECT_SESSION_SUBJECT {
            let title = t!("Revoke {name}?", name = connect_device_name(Some(&session.client)).as_ref());
            self.request_connect_revoke(title, Rc::new(move |this, cx| this.revoke_session(id.clone(), cx)), cx);
        } else {
            self.revoke_session(id, cx);
        }
    }

    fn revoke_others(&mut self, cx: &mut Context<Self>) {
        self.clients.revoking_others = true;
        self.run_access(t!("Could not revoke other clients"), |link| Box::pin(async move { access::revoke_other_clients(&link).await.map_err(|e| e.to_string()) }), cx);
    }

    /// `requestRevokeOtherClients`.
    pub fn request_revoke_others(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.access.as_ref();
        let revokes_devices = snapshot.is_some_and(|s| {
            s.connect_devices.as_ref().is_some_and(|d| !d.is_empty()) || s.client_sessions.iter().any(|c| !c.current && c.subject == CLOUD_CONNECT_SESSION_SUBJECT)
        });
        if revokes_devices {
            self.request_connect_revoke(t!("Revoke all other clients?"), Rc::new(|this, cx| this.revoke_others(cx)), cx);
        } else {
            self.revoke_others(cx);
        }
    }

    pub fn approve_device(&mut self, pairing_link_id: String, cx: &mut Context<Self>) {
        self.clients.busy_device = Some(pairing_link_id.clone());
        self.run_access(
            t!("Could not approve the device"),
            move |link| Box::pin(async move { access::approve_connect_device(&link, pairing_link_id).await.map(|_| ()).map_err(|e| e.to_string()) }),
            cx,
        );
    }

    pub fn deny_device(&mut self, pairing_link_id: String, cx: &mut Context<Self>) {
        self.clients.busy_device = Some(pairing_link_id.clone());
        self.run_access(t!("Could not deny the device"), move |link| Box::pin(async move { access::revoke_pairing_link(&link, pairing_link_id).await.map_err(|e| e.to_string()) }), cx);
    }

    fn revoke_device(&mut self, thumbprint: String, cx: &mut Context<Self>) {
        self.clients.busy_device = Some(thumbprint.clone());
        self.run_access(
            t!("Could not revoke the device"),
            move |link| Box::pin(async move { access::revoke_connect_device(&link, thumbprint).await.map(|_| ()).map_err(|e| e.to_string()) }),
            cx,
        );
    }

    pub fn request_revoke_device(&mut self, device: AuthConnectDevice, cx: &mut Context<Self>) {
        let title = t!("Revoke {name}?", name = connect_device_name(device.client.as_ref()).as_ref());
        let thumbprint = device.proof_key_thumbprint.clone();
        self.request_connect_revoke(title, Rc::new(move |this, cx| this.revoke_device(thumbprint.clone(), cx)), cx);
    }

    /// The revoke dialog's Revoke: turns approval on first when ticked, then revokes.
    pub fn confirm_pending_revoke(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.clients.pending_revoke.as_mut() else { return };
        pending.busy = true;
        let turn_on = pending.turn_on_approval;
        cx.notify();
        if !turn_on {
            let pending = self.clients.pending_revoke.take().expect("checked above");
            (pending.revoke)(self, cx);
            return;
        }
        let id = self.environment_id(cx);
        let save = crate::hooks::use_settings::ServerSettingsStore::update(id, ClientServerSettingsPatch { require_connect_device_approval: Some(true), ..Default::default() }, cx);
        self.clients.tasks.push(cx.spawn(async move |this, cx| {
            let saved = save.await;
            this.update(cx, |this, cx| {
                if saved.is_err() {
                    if let Some(p) = this.clients.pending_revoke.as_mut() {
                        p.busy = false;
                    }
                    cx.notify();
                    return;
                }
                if let Some(pending) = this.clients.pending_revoke.take() {
                    (pending.revoke)(this, cx);
                }
            })
            .ok();
        }));
    }

    /// Opens "Create pairing link".
    pub fn open_create_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let label = cx.new(|cx| InputState::new(window, cx).placeholder(t!("e.g. Living room iPad")));
        label.update(cx, |state, cx| state.focus(window, cx));
        let delegatable = self.delegatable();
        self.clients.create = Some(CreateLinkDialog { label, scopes: STANDARD_SCOPES.iter().copied().filter(|s| delegatable.contains(s)).collect(), creating: false });
        cx.notify();
    }

    /// The scopes this session may hand on (`currentSessionScopes`).
    fn delegatable(&self) -> Vec<AuthGrantScope> {
        STANDARD_SCOPES.iter().chain([AuthGrantScope::RelayConfigure].iter()).copied().filter(|s| environment_scope(*s).is_some_and(|e| self.may(e))).collect()
    }

    pub fn create_link(&mut self, cx: &mut Context<Self>) {
        let delegatable = self.delegatable();
        let Some(dialog) = self.clients.create.as_mut() else { return };
        let scopes: Vec<AuthGrantScope> = dialog.scopes.iter().copied().filter(|s| delegatable.contains(s)).collect();
        if scopes.is_empty() || dialog.creating {
            return;
        }
        dialog.creating = true;
        let label = dialog.label.read(cx).value().trim().to_owned();
        let Some(link) = self.link(cx) else { return };
        let task = ConnectionRuntime::global(cx).spawn(
            async move { access::create_pairing_link(&link, (!label.is_empty()).then_some(label), scopes).await.map_err(CommandError::from) },
            cx,
        );
        self.clients.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(created) => {
                        this.created_credentials.insert(created.id.clone(), created.credential.into());
                        this.clients.create = None;
                    }
                    Err(error) => {
                        if let Some(d) = this.clients.create.as_mut() {
                            d.creating = false;
                        }
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not create pairing URL")).description(error.0), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `AuthorizedClientsHeaderAction`: Use in another app (MCP), Revoke others, Create link.
    pub(super) fn render_clients_header_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let can_revoke_others = self.access.as_ref().is_some_and(|s| s.client_sessions.iter().any(|c| !c.current));
        let id = self.environment_id(cx);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap(theme.sp(2.))
            .child(
                Button::new("access-use-in-another-app")
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::Outline)
                    .icon(IconName::Plug)
                    .icon_size(IconSize::Custom(theme.rem(0.75)))
                    .label(t!("Use in another app (MCP)"))
                    .on_click(move |_, window, cx| {
                        let navigate: crate::components::sharing_dialogs::Navigate = Rc::new(|route, cx| {
                            if let crate::router::Route::Settings { panel, .. } = route {
                                super::settings_layout::SettingsNavigator::open(panel.settings_path(), cx);
                            }
                        });
                        crate::components::sharing_dialogs::SharingDialogs::open_use_in_another_app(Some(id.clone()), navigate, window, cx);
                    }),
            )
            .child(
                Button::new("access-revoke-others")
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::DestructiveOutline)
                    .label(if self.clients.revoking_others { t!("Revoking…") } else { t!("Revoke others") })
                    .disabled(self.clients.revoking_others || !can_revoke_others)
                    .on_click(cx.listener(|this, _, _, cx| this.request_revoke_others(cx))),
            )
            .child(
                Button::new("access-create-link")
                    .size(ButtonSize::Xs)
                    .icon(IconName::Plus)
                    .icon_size(IconSize::Custom(theme.rem(0.75)))
                    .label(t!("Create link"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_create_link(window, cx))),
            )
            .into_any_element()
    }

    /// A pairing link row: its label, expiry, the code when this client just made it, Revoke.
    pub(super) fn render_pairing_link_row(&self, link: &AuthPairingLink, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let credential = self.created_credentials.get(&link.id).cloned();
        let id = link.id.clone();
        let busy = self.clients.busy_device.as_deref() == Some(link.id.as_str());
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
                    .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(SharedString::from(link.label.clone().unwrap_or_else(|| t!("Pairing link").to_string()))))
                    .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(crate::timestamp_format::format_expires_in_label(&link.expires_at, t3code_client_runtime::state::models::now_ms())))
                    .children(credential.map(|code| div().font(mono_font(weight::NORMAL)).text_size(gpui_kit::px(TextSize::Xs.size(theme.fonts.interface))).child(code))),
            )
            .child(
                Button::new(SharedString::from(format!("access-revoke-link-{id}")))
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::DestructiveOutline)
                    .label(if busy { t!("Revoking…") } else { t!("Revoke") })
                    .disabled(busy || !self.may(AuthEnvironmentScope::AccessWrite))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.clients.busy_device = Some(id.clone());
                        let link_id = id.clone();
                        this.run_access(t!("Could not revoke pairing link"), move |link| Box::pin(async move { access::revoke_pairing_link(&link, link_id).await.map_err(|e| e.to_string()) }), cx);
                    })),
            )
            .into_any_element()
    }

    /// The per-client Revoke (`ConnectedClientListRow`), absent on this device's own row.
    pub(super) fn render_client_revoke(&self, session: &AuthClientSession, cx: &mut Context<Self>) -> Option<AnyElement> {
        if session.current {
            return None;
        }
        let busy = self.clients.revoking_session.as_ref() == Some(&session.session_id);
        let target = session.clone();
        Some(
            Button::new(SharedString::from(format!("access-revoke-client-{}", session.session_id.as_str())))
                .size(ButtonSize::Xs)
                .variant(ButtonVariant::DestructiveOutline)
                .label(if busy { t!("Revoking…") } else { t!("Revoke") })
                .disabled(busy || !self.may(AuthEnvironmentScope::AccessWrite))
                .on_click(cx.listener(move |this, _, _, cx| this.request_revoke_session(target.clone(), cx)))
                .into_any_element(),
        )
    }

    /// `ConnectDeviceApprovalSettings`: devices waiting for approval, then trusted devices.
    pub(super) fn render_connect_devices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = self.access.as_ref()?;
        let devices = snapshot.connect_devices.clone()?;
        let requests: Vec<AuthPairingLink> = snapshot.pairing_links.iter().filter(|l| l.connect_device_request.is_some()).cloned().collect();
        if !self.may(AuthEnvironmentScope::AccessRead) || (devices.is_empty() && requests.is_empty()) {
            return None;
        }
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let can_write = self.may(AuthEnvironmentScope::AccessWrite);
        let divider = with_alpha(c.border, 0.5);
        let mut rows: Vec<AnyElement> = Vec::new();
        for request in &requests {
            let Some(connect) = &request.connect_device_request else { continue };
            let busy = self.clients.busy_device.as_deref() == Some(request.id.as_str());
            let (deny_id, approve_id) = (request.id.clone(), request.id.clone());
            rows.push(
                row(
                    &theme,
                    div()
                        .flex()
                        .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
                        .text_color(c.foreground())
                        .child(t_rich!("Waiting for approval ·{\" \"}<span>{code}</span>", code = connect.verification_code.as_str()).plain())
                        .into_any_element(),
                    t_rich!(
                        "{device}. Approve only if the device in front of you shows this code.",
                        device = describe_connect_device(connect.client.as_ref()).as_ref()
                    )
                    .plain()
                    .into(),
                    vec![
                        Button::new(SharedString::from(format!("device-deny-{}", request.id)))
                            .size(ButtonSize::Xs)
                            .variant(ButtonVariant::DestructiveOutline)
                            .label(t!("Deny"))
                            .disabled(!can_write || busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.deny_device(deny_id.clone(), cx)))
                            .into_any_element(),
                        Button::new(SharedString::from(format!("device-approve-{}", request.id)))
                            .size(ButtonSize::Xs)
                            .label(if busy { t!("Saving…") } else { t!("Approve") })
                            .disabled(!can_write || busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.approve_device(approve_id.clone(), cx)))
                            .into_any_element(),
                    ],
                ),
            );
        }
        for device in &devices {
            let busy = self.clients.busy_device.as_deref() == Some(device.proof_key_thumbprint.as_str());
            let target = device.clone();
            let since = crate::timestamp_format::parse_timestamp_date(&device.approved_at).map(|_| device.approved_at.clone()).unwrap_or_default();
            rows.push(row(
                &theme,
                div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(describe_connect_device(device.client.as_ref())).into_any_element(),
                format!("{} · {}", device.verification_code, t_rich!("Trusted since{\" \"}{time}", time = since.as_str()).plain()).into(),
                vec![
                    Button::new(SharedString::from(format!("device-revoke-{}", device.proof_key_thumbprint)))
                        .size(ButtonSize::Xs)
                        .variant(ButtonVariant::DestructiveOutline)
                        .label(if busy { t!("Revoking…") } else { t!("Revoke") })
                        .disabled(!can_write || busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.request_revoke_device(target.clone(), cx)))
                        .into_any_element(),
                ],
            ));
        }
        let rows = rows.into_iter().enumerate().map(|(ix, r)| div().when(ix > 0, |d| d.border_t_1().border_color(divider)).child(r));
        Some(div().id("connect-devices").child(super::settings_layout::SettingsSection::new(t!("T3 Connect devices")).children(rows)).into_any_element())
    }

    /// The revoke offer and the Create link dialog, when open.
    pub(super) fn render_access_dialogs(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let mut out = Vec::new();
        if let Some(pending) = &self.clients.pending_revoke {
            let ticked = pending.turn_on_approval;
            let busy = pending.busy;
            let surface = DialogSurface::new(pending.title.clone())
                .description(t_rich!("Approval for new devices is off, so a revoked device signed in to your T3 Connect account can connect again on its own.").plain())
                .child(
                    div()
                        .id("revoke-turn-on-approval")
                        .flex()
                        .items_start()
                        .gap(theme.sp(3.))
                        .cursor_pointer()
                        .child(div().mt(theme.sp(0.5)).child(Checkbox::new("revoke-approval-checkbox", if ticked { CheckState::Checked } else { CheckState::Unchecked })))
                        .child(
                            div()
                                .min_w_0()
                                .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("Require approval for new devices")))
                                .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("The device has to be approved here before it can connect again."))),
                        )
                        .when(!busy, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| {
                                if let Some(p) = this.clients.pending_revoke.as_mut() {
                                    p.turn_on_approval = !p.turn_on_approval;
                                }
                                cx.notify();
                            }))
                        })
                        .test_support(),
                )
                .footer(Button::new("revoke-device-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).disabled(busy).on_click(cx.listener(|this, _, _, cx| {
                    this.clients.pending_revoke = None;
                    cx.notify();
                })))
                .footer(Button::new("revoke-device-confirm").variant(ButtonVariant::Destructive).label(t!("Revoke")).disabled(busy).on_click(cx.listener(|this, _, _, cx| this.confirm_pending_revoke(cx))));
            let this = cx.entity().downgrade();
            out.push(
                Modal::dialog("revoke-device-dialog", div().id("revoke-device").occlude().w_full().max_w(theme.rem(32.)).child(surface), move |_, cx| {
                    this.update(cx, |this, cx| {
                        if !this.clients.pending_revoke.as_ref().is_some_and(|p| p.busy) {
                            this.clients.pending_revoke = None;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .into_any_element(),
            );
        }
        if let Some(dialog) = &self.clients.create {
            let delegatable = self.delegatable();
            let selected: Vec<AuthGrantScope> = dialog.scopes.iter().copied().filter(|s| delegatable.contains(s)).collect();
            let creating = dialog.creating;
            let preset = |id: &'static str, label: SharedString, scopes: Vec<AuthGrantScope>, cx: &mut Context<Self>| {
                Button::new(id).size(ButtonSize::Xs).variant(ButtonVariant::Outline).label(label).disabled(creating).on_click(cx.listener(move |this, _, _, cx| {
                    let delegatable = this.delegatable();
                    if let Some(d) = this.clients.create.as_mut() {
                        d.scopes = scopes.iter().copied().filter(|s| delegatable.contains(s)).collect();
                    }
                    cx.notify();
                }))
            };
            let read_preset = preset("pairing-preset-read", t!("Read only"), READ_ONLY_SCOPES.to_vec(), cx);
            let standard_preset = preset("pairing-preset-standard", t!("Standard", context = "standard permission preset"), STANDARD_SCOPES.to_vec(), cx);
            let options: Vec<_> = STANDARD_SCOPES.iter().chain([AuthGrantScope::RelayConfigure].iter()).copied().filter(|s| delegatable.contains(s)).enumerate().map(|(ix, scope)| {
                let (title, description) = scope_option(scope);
                let checked = dialog.scopes.contains(&scope);
                div()
                    .id(SharedString::from(format!("pairing-scope-{}", scope.as_str())))
                    .when(ix > 0, |d| d.border_t_1().border_color(with_alpha(c.border, 0.6)))
                    .flex()
                    .items_start()
                    .gap(theme.sp(3.))
                    .px(theme.sp(3.))
                    .py(theme.sp(2.5))
                    .cursor_pointer()
                    .hover(|d| d.bg(with_alpha(c.muted, 0.4)))
                    .child(div().mt(theme.sp(0.5)).child(Checkbox::new(SharedString::from(format!("pairing-scope-check-{}", scope.as_str())), if checked { CheckState::Checked } else { CheckState::Unchecked })))
                    .child(
                        div()
                            .min_w_0()
                            .child(div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(title))
                            .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(description))
                            .when(checked && scope == AuthGrantScope::TerminalOperate, |d| {
                                d.child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.warning).child(t!("Terminal commands can read or change files within the server account’s permissions.")))
                            }),
                    )
                    .when(!creating, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(dialog) = this.clients.create.as_mut() {
                                if dialog.scopes.contains(&scope) {
                                    dialog.scopes.retain(|s| *s != scope);
                                } else {
                                    dialog.scopes.push(scope);
                                }
                            }
                            cx.notify();
                        }))
                    })
            }).collect();
            let surface = DialogSurface::new(t!("Create pairing link"))
                .max_width_rem(28.)
                .bare_footer()
                .description(t_rich!("Generate a one-time link that another device can use to pair with this backend as an authorized client.").plain())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(1.5))
                        .child(div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(t!("Client label (optional)")))
                        .child(div().id("pairing-label").child(TextInput::new(&dialog.label).disabled(creating)).test_support()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(3.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(theme.sp(3.))
                                .child(
                                    div()
                                        .child(div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(t!("Permissions")))
                                        .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("Limit what the paired client can do."))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(theme.sp(1.))
                                        .child(read_preset)
                                        .child(standard_preset),
                                ),
                        )
                        .child(
                            // The fork scrolls the whole dialog panel; this scrolls the list, which is most of it.
                            crate::components::ui::scroll_area::scroll_area("pairing-scopes")
                                .max_h(theme.rem(18.))
                                .rounded(theme.radii.lg)
                                .border_1()
                                .border_color(c.input)
                                .bg(with_alpha(c.muted, 0.25))
                                .children(options),
                        )
                        .child(if selected.is_empty() {
                            div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(t!("Select at least one permission.")).into_any_element()
                        } else if selected.contains(&AuthGrantScope::AccessWrite) {
                            div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.warning).child(t!("This client can create or revoke access for other devices.")).into_any_element()
                        } else {
                            div().into_any_element()
                        }),
                )
                .footer(Button::new("pairing-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).disabled(creating).on_click(cx.listener(|this, _, _, cx| {
                    this.clients.create = None;
                    cx.notify();
                })))
                .footer(
                    Button::new("pairing-create")
                        .label(if creating { t!("Creating…") } else { t!("Create link") })
                        .disabled(creating || selected.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.create_link(cx))),
                );
            let this = cx.entity().downgrade();
            out.push(
                Modal::dialog("create-pairing-link-dialog", div().id("create-pairing-link").occlude().w_full().max_w(theme.rem(28.)).child(surface), move |_, cx| {
                    this.update(cx, |this, cx| {
                        this.clients.create = None;
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element(),
            );
        }
        out
    }
}

/// `ITEM_ROW_CLASSNAME` with a title, a muted line and trailing buttons.
fn row(theme: &AppTheme, title: AnyElement, line: SharedString, buttons: Vec<AnyElement>) -> AnyElement {
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
                .child(title)
                .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(line)),
        )
        .child(div().flex().flex_none().items_center().gap(theme.sp(2.)).children(buttons))
        .into_any_element()
}
