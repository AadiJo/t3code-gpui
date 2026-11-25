//! The owner side of guest links. Ports `GuestShareControls.tsx` and `guestShareAvailability.ts`:
//! the "Share with guest" dialog (create a link with `guestShare.create`, copy it), and the
//! header's status of a shared thread with "Stop sharing" (`thread.guest-share.close`).

use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Task, Window, component::input::{InputEvent, InputState}, div, prelude::*,
};
use t3code_contracts::rpc::methods::GuestShareCreate;
use t3code_contracts::{EnvironmentId, GuestShareCreateInput, ThreadGuestShare, ThreadId};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::managed_policy::{ManagedCapability, managed_capability_denial};
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::TextInput;
use crate::i18n::t;
use crate::live::rpc::RpcLink;
use crate::state::StateBridge;
use crate::state::shell::runtime_ids;

/// `canShareWithGuestAtom`: the environment's Guest links beta is on and its policy leaves
/// T3 Connect on (guests arrive through it). The server refuses links otherwise.
pub fn can_share_with_guest(environment: &EnvironmentId, cx: &App) -> bool {
    let Some(config) = StateBridge::is_running(cx).then(|| StateBridge::environment(environment, cx)).flatten().and_then(|e| e.read(cx).config.clone()) else {
        return false;
    };
    config.settings.guest_sharing_enabled == Some(true) && managed_capability_denial(config.managed_policy.as_ref(), ManagedCapability::Connect).is_none()
}

/// The open thread's guest share, as its projection holds it (`useThreadGuestShare`).
pub fn thread_guest_share(key: &ThreadKey, cx: &App) -> Option<ThreadGuestShare> {
    let threads = StateBridge::is_running(cx).then(|| StateBridge::threads(cx))?;
    let thread = threads.read(cx).find(key, cx)?;
    thread.read(cx).projection()?.thread.guest_share.clone()
}

/// `GUEST_SHARE_CREATE_MESSAGES`: the server error's own message (English, as in the fork).
fn create_error_message(reason: t3code_contracts::GuestShareCreateErrorReason) -> SharedString {
    use t3code_contracts::GuestShareCreateErrorReason as R;
    match reason {
        R::Disabled => "Guest links are off. Turn them on in Settings > Labs.",
        R::ConnectUnavailable => "Guest links need a running T3 Connect tunnel.",
        R::ThreadUnavailable => "This thread cannot be shared.",
        R::ReadFailed => "Failed to read the shared thread.",
        R::Changed => "The guest link changed while it was being created.",
    }
    .into()
}

/// `CreateState`.
#[derive(Clone, Debug, PartialEq)]
enum CreateState {
    Idle,
    Creating,
    Created(SharedString),
    Failed(SharedString),
}

/// The dialog asks its host to close it.
pub struct GuestShareDialogClosed;

/// "Share with guest" for one thread.
pub struct GuestShareDialog {
    key: ThreadKey,
    state: CreateState,
    link: Entity<InputState>,
    copied: bool,
    _copied_reset: Option<Task<()>>,
    _link_edits: Subscription,
}

impl EventEmitter<GuestShareDialogClosed> for GuestShareDialog {}

impl GuestShareDialog {
    pub fn new(key: ThreadKey, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link = cx.new(|cx| InputState::new(window, cx));
        // The field is read-only: an edit puts the link back.
        let link_edits = cx.subscribe_in(&link, window, |this, input, event: &InputEvent, window, cx| {
            if let (InputEvent::Change, CreateState::Created(url)) = (event, &this.state)
                && input.read(cx).value() != *url
            {
                let url = url.clone();
                input.update(cx, |input, cx| input.set_value(url, window, cx));
            }
        });
        Self { key, state: CreateState::Idle, link, copied: false, _copied_reset: None, _link_edits: link_edits }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        // `onOpenChange`: a link being created keeps the dialog open.
        if self.state != CreateState::Creating {
            cx.emit(GuestShareDialogClosed);
        }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (environment, thread_id): (EnvironmentId, ThreadId) = runtime_ids(&self.key);
        let Some(link) = RpcLink::new(&environment, cx) else {
            self.state = CreateState::Failed(t!("Could not create a guest link."));
            return cx.notify();
        };
        self.state = CreateState::Creating;
        cx.notify();
        let session_link = link.clone();
        let request = link.run(async move {
            let session = session_link.session().await?;
            session.client.call::<GuestShareCreate>(GuestShareCreateInput { thread_id }).await.map_err(|error| match &error {
                t3code_client_runtime::rpc::client::RpcError::Failure(t3code_contracts::GuestShareCreateError2::Error(failure)) => create_error_message(failure.reason),
                other => crate::live::rpc::error_message(other),
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(created) => {
                        let url = SharedString::from(created.url);
                        this.link.update(cx, |input, cx| input.set_value(url.clone(), window, cx));
                        this.state = CreateState::Created(url);
                    }
                    Err(message) if message.is_empty() => this.state = CreateState::Failed(t!("Could not create a guest link.")),
                    Err(message) => this.state = CreateState::Failed(message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let CreateState::Created(url) = &self.state else { return };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.to_string()));
        self.copied = true;
        // `useCopyToClipboard`'s 2 second check mark.
        self._copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Why the last Create link failed.
    pub fn failure(&self) -> Option<SharedString> {
        match &self.state {
            CreateState::Failed(message) => Some(message.clone()),
            _ => None,
        }
    }

    /// The link the dialog created, once it has one.
    pub fn created_url(&self) -> Option<SharedString> {
        match &self.state {
            CreateState::Created(url) => Some(url.clone()),
            _ => None,
        }
    }
}

impl Render for GuestShareDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let weak = cx.weak_entity();
        let active = thread_guest_share(&self.key, cx).is_some();
        let can_create = can_share_with_guest(&runtime_ids(&self.key).0, cx);
        let creating = self.state == CreateState::Creating;
        let body = match &self.state {
            CreateState::Created(_) => {
                let copy = weak.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(div().flex_1().min_w_0().child(TextInput::new(&self.link)))
                    .child(
                        Button::new("guest-share-copy")
                            .size(ButtonSize::Sm)
                            .variant(ButtonVariant::Outline)
                            .icon(if self.copied { IconName::Check } else { IconName::Copy })
                            .label(if self.copied { t!("Copied") } else { t!("Copy", context = "verb, copy the link") })
                            .on_click(move |_, _, cx| _ = copy.update(cx, |this, cx| this.copy(cx))),
                    )
                    .into_any_element()
            }
            _ => div()
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(if active { t!("A new link ends the current one.") } else { t!("The link works once and expires in 24 hours.") })
                .into_any_element(),
        };
        let failure = match &self.state {
            CreateState::Failed(message) => Some(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(message.clone())),
            _ => None,
        };
        let close = weak.clone();
        let mut surface = DialogSurface::new(t!("Share with guest"))
            .max_width_rem(32.)
            .description(t!(
                "The guest sees the thread as you do, including command output and the agent's reasoning. Each message they send waits for you."
            ))
            .on_close(move |_, cx| _ = close.update(cx, |this, cx| this.close(cx)))
            .child(body)
            .children(failure);
        surface = match &self.state {
            CreateState::Created(_) => {
                let done = weak.clone();
                surface.footer(
                    Button::new("guest-share-done")
                        .size(ButtonSize::Sm)
                        .label(t!("Done", context = "button that closes the dialog"))
                        .on_click(move |_, _, cx| _ = done.update(cx, |this, cx| this.close(cx))),
                )
            }
            _ => {
                let (cancel, create) = (weak.clone(), weak.clone());
                surface
                    .footer(
                        Button::new("guest-share-cancel")
                            .size(ButtonSize::Sm)
                            .variant(ButtonVariant::Outline)
                            .label(t!("Cancel"))
                            .disabled(creating)
                            .on_click(move |_, _, cx| _ = cancel.update(cx, |this, cx| this.close(cx))),
                    )
                    .footer(
                        Button::new("guest-share-create")
                            .size(ButtonSize::Sm)
                            .label(if creating { t!("Creating...") } else { t!("Create link") })
                            .disabled(!can_create || creating)
                            .on_click(move |_, window, cx| _ = create.update(cx, |this, cx| this.create(window, cx))),
                    )
            }
        };
        let backdrop = weak;
        Modal::dialog("guest-share-dialog", div().id("guest-share-content").occlude().w_full().max_w(theme.rem(32.)).child(surface), move |_, cx| {
            _ = backdrop.update(cx, |this, cx| this.close(cx));
        })
    }
}

/// `GuestShareStatus`: "Guest link expired" once its minute has passed, else who is a guest.
pub fn guest_share_status(share: &ThreadGuestShare, now_minute: &str) -> SharedString {
    if share.expires_at.get(..16).unwrap_or(&share.expires_at) <= now_minute {
        return t!("Guest link expired");
    }
    match &share.guest_name {
        Some(name) => t!("{guestName} is a guest", guestName = name.clone()),
        None => t!("Guest link active"),
    }
}

/// Threads whose "Stop sharing" is in flight, so the button stays disabled until the server answers.
#[derive(Default)]
struct ClosingShares(std::collections::HashSet<ThreadKey>);

impl gpui_kit::Global for ClosingShares {}

/// `GuestShareIndicator`: a shared thread's status in the chat header, with "Stop sharing"
/// (`thread.guest-share.close`). Nothing while the thread has no share.
pub fn guest_share_indicator(key: &ThreadKey, cx: &App) -> Option<gpui_kit::AnyElement> {
    let share = thread_guest_share(key, cx)?;
    let theme = cx.app_theme();
    let now_minute = t3code_client_runtime::state::models::format_ms(t3code_client_runtime::state::models::now_ms());
    let status = guest_share_status(&share, now_minute.get(..16).unwrap_or(&now_minute));
    let closing = cx.try_global::<ClosingShares>().is_some_and(|c| c.0.contains(key));
    let close_key = key.clone();
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.sp(1.))
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(theme.colors.muted_foreground)
            .child(crate::components::icons::Icon::new(IconName::User).size(crate::components::icons::IconSize::Custom(theme.rem(0.875))).color(theme.colors.muted_foreground))
            .child(status)
            .child(
                Button::new("guest-share-stop")
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::GhostMuted)
                    .label(t!("Stop sharing"))
                    .disabled(closing)
                    .on_click(move |_, _, cx| stop_sharing(close_key.clone(), cx)),
            )
            .into_any_element(),
    )
}

/// Ends `key`'s guest share on its server.
pub fn stop_sharing(key: ThreadKey, cx: &mut App) {
    let (environment, thread_id) = runtime_ids(&key);
    let Some(commands) = StateBridge::environment(&environment, cx).map(|e| e.read(cx).commands.clone()) else { return };
    let command = t3code_contracts::OrchestrationV2Command::ThreadGuestShareClose(t3code_contracts::OrchestrationV2CommandThreadGuestShareClose {
        command_id: t3code_contracts::CommandId::from(uuid::Uuid::new_v4().to_string().as_str()),
        thread_id,
    });
    cx.default_global::<ClosingShares>().0.insert(key.clone());
    cx.refresh_windows();
    let task = StateBridge::spawn(async move { commands.dispatch(command).await.map_err(|e| e.to_string()) }, cx);
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            cx.default_global::<ClosingShares>().0.remove(&key);
            if let Some(Err(error)) = result {
                crate::components::ui::toast::Toaster::push(
                    crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, t!("Could not stop sharing")).description(error),
                    cx,
                );
            }
            cx.refresh_windows();
        });
    })
    .detach();
}
