//! The window's sharing dialogs, opened from the command palette, thread menus and settings, and
//! drawn once over the app (`GuestShareDialogHost`, `UseInAnotherAppDialogHost`): "Share with
//! guest", "Use T3 Code in another app (MCP)", and the T3 Connect sign-in that wizard asks for.

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Global, IntoElement, Render, Subscription, Window, div, prelude::*};
use t3code_contracts::EnvironmentId;

use crate::components::chat::guest_share_controls::{GuestShareDialog, GuestShareDialogClosed};
use crate::components::clerk::{T3ConnectSignIn, T3ConnectSignInEvent};
use crate::components::mcp::use_in_another_app_dialog::{UseInAnotherAppDialog, UseInAnotherAppEvent, new_dialog};
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::dialog::Modal;
use crate::router::Route;

/// Opens a route for a dialog that leads somewhere else (Manage access).
pub type Navigate = Rc<dyn Fn(Route, &mut App)>;

/// The open sharing dialog, if any.
#[derive(Default)]
pub struct SharingDialogs {
    open: Option<(AnyView, Subscription)>,
}

impl Global for SharingDialogs {}

impl SharingDialogs {
    /// Opens "Share with guest" for `key` (`openGuestShareDialog`).
    pub fn open_guest_share(key: ThreadKey, window: &mut Window, cx: &mut App) {
        let dialog = cx.new(|cx| GuestShareDialog::new(key, window, cx));
        let subscription = cx.subscribe(&dialog, |_, _: &GuestShareDialogClosed, cx| Self::close(cx));
        cx.set_global(Self { open: Some((dialog.into(), subscription)) });
        cx.refresh_windows();
    }

    /// Opens "Use T3 Code in another app (MCP)" for `requested`, or for the environment Sharing &
    /// access picks when `None` (`openUseInAnotherApp`).
    pub fn open_use_in_another_app(requested: Option<EnvironmentId>, navigate: Navigate, window: &mut Window, cx: &mut App) {
        let handle = window.window_handle();
        let dialog = new_dialog(requested, cx);
        let subscription = cx.subscribe(&dialog, move |_, event: &UseInAnotherAppEvent, cx| match event {
            UseInAnotherAppEvent::Close => Self::close(cx),
            UseInAnotherAppEvent::SignIn => Self::open_sign_in(handle, cx),
            UseInAnotherAppEvent::ManageAccess(id) => {
                Self::close(cx);
                navigate(Route::parse(&format!("/settings/access?machine={}", id.as_str())), cx);
            }
        });
        cx.set_global(Self { open: Some((dialog.into(), subscription)) });
        cx.refresh_windows();
    }

    /// The T3 Connect sign-in as its own modal (Clerk's sign-in in the web app).
    fn open_sign_in(window: gpui_kit::AnyWindowHandle, cx: &mut App) {
        let host = window
            .update(cx, |_, window, cx| {
                let form = cx.new(|cx| T3ConnectSignIn::new(window, cx));
                cx.new(|_| SignInHost { form })
            })
            .ok();
        let Some(host) = host else { return Self::close(cx) };
        let form = host.read(cx).form.clone();
        let subscription = cx.subscribe(&form, move |_, event: &T3ConnectSignInEvent, cx| match event {
            T3ConnectSignInEvent::Close | T3ConnectSignInEvent::SignedIn => {
                // The form's field held focus; leaving it on a removed element would swallow shortcuts.
                window.update(cx, |_, window, cx| window.blur(cx)).ok();
                Self::close(cx);
            }
        });
        cx.set_global(Self { open: Some((host.into(), subscription)) });
        cx.refresh_windows();
    }

    pub fn close(cx: &mut App) {
        cx.set_global(Self::default());
        cx.refresh_windows();
    }

    /// The open dialog, for the window root to draw over everything.
    pub fn layer(cx: &App) -> Option<AnyView> {
        cx.try_global::<Self>().and_then(|dialogs| dialogs.open.as_ref().map(|(view, _)| view.clone()))
    }

    /// The open "Share with guest" dialog.
    pub fn guest_share(cx: &App) -> Option<gpui_kit::Entity<GuestShareDialog>> {
        Self::layer(cx)?.downcast::<GuestShareDialog>().ok()
    }

    /// The open "Use T3 Code in another app" wizard.
    pub fn use_in_another_app(cx: &App) -> Option<gpui_kit::Entity<UseInAnotherAppDialog>> {
        Self::layer(cx)?.downcast::<UseInAnotherAppDialog>().ok()
    }

    /// The open T3 Connect sign-in form.
    pub fn sign_in_form(cx: &App) -> Option<gpui_kit::Entity<T3ConnectSignIn>> {
        Some(Self::layer(cx)?.downcast::<SignInHost>().ok()?.read(cx).form.clone())
    }

    pub fn is_open(cx: &App) -> bool {
        Self::layer(cx).is_some()
    }
}

/// The sign-in form over a backdrop.
struct SignInHost {
    form: gpui_kit::Entity<T3ConnectSignIn>,
}

impl Render for SignInHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        let form = self.form.clone();
        Modal::dialog("t3-connect-sign-in-modal", div().id("t3-connect-sign-in-host").occlude().w_full().child(self.form.clone()), move |_, cx| {
            form.update(cx, |_, cx| cx.emit(T3ConnectSignInEvent::Close));
        })
    }
}
