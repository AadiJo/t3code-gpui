//! The right-click menu on a pull request number. Ports `pullRequestLinkContextMenu.ts`:
//! "Copy link" and "Open on {host}" in the system browser.

use gpui_kit::{ClipboardItem, SharedString};

use crate::components::icons::IconName;
use crate::components::ui::menu::{MenuItem, MenuPopup};
use crate::i18n::t;
use crate::source_control_presentation::{UNKNOWN_SOURCE_CONTROL, source_control_definition};

/// `openOnHostLabel`: "Open on GitHub", or "Open on host" for a host the app does not know.
pub fn open_on_host_label(provider: &str) -> SharedString {
    let host = source_control_definition(Some(provider));
    if host == UNKNOWN_SOURCE_CONTROL { t!("Open on host") } else { t!("Open on {host}", host = host.label) }
}

/// `pullRequestLinkContextMenuItems` for the pull request at `url` on `provider`'s host.
pub fn pull_request_link_menu(url: &str, provider: &str) -> MenuPopup {
    let (copy, open) = (url.to_owned(), url.to_owned());
    MenuPopup::new()
        .context()
        .item(MenuItem::new(t!("Copy link")).icon(IconName::Copy).on_select(move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
        .item(MenuItem::new(open_on_host_label(provider)).on_select(move |_, cx| cx.open_url(&open)))
}
