//! The window's sharing dialogs, opened from the command palette and thread menus and drawn once
//! over the app (`GuestShareDialogHost`): "Share with guest".

use gpui_kit::{AnyView, App, AppContext as _, Global, Subscription, Window};

use crate::components::chat::guest_share_controls::{GuestShareDialog, GuestShareDialogClosed};
use crate::components::sidebar::model::ThreadKey;

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

    pub fn is_open(cx: &App) -> bool {
        Self::layer(cx).is_some()
    }
}
