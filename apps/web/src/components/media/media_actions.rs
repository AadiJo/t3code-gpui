//! The context menu on an image or video. Ports `MediaActions.tsx`:
//! copy full or relative path, copy URL (not offered: signed asset URLs are
//! bearer credentials, spec 01 section 6.2), open in the file viewer, save,
//! and copy image.

use std::{path::PathBuf, rc::Rc};

use gpui_kit::{App, AppContext as _, ClipboardItem, SharedString, Window};

use super::media_source::{DecodedImage, MediaBytes};
use crate::{
    components::{
        icons::IconName,
        ui::menu::{MenuItem, MenuPopup},
    },
    i18n::t,
};

/// Image or video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

/// `MediaActionSource`: what the menu acts on.
#[derive(Clone)]
pub struct MediaActionSource {
    pub kind: MediaKind,
    pub name: SharedString,
    pub relative_path: Option<SharedString>,
    pub absolute_path: Option<SharedString>,
    pub bytes: MediaBytes,
    /// Set once the image decoded, for "Copy image".
    pub decoded: Option<DecodedImage>,
}

/// Opens the media's file in the file viewer.
pub type OpenFileHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The menu for a media item.
pub fn media_actions_menu(source: &MediaActionSource, open_file: Option<OpenFileHandler>) -> MenuPopup {
    let mut menu = MenuPopup::new();
    if let Some(path) = source.absolute_path.clone() {
        menu = menu.item(MenuItem::new(t!("Copy full path")).icon(IconName::Copy).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
        }));
    }
    if let Some(path) = source.relative_path.clone() {
        menu = menu.item(MenuItem::new(t!("Copy relative path")).icon(IconName::Copy).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(path.to_string()));
        }));
    }
    if let Some(open) = open_file {
        menu = menu.item(MenuItem::new(t!("Open in file viewer")).icon(IconName::FileText).on_select(move |window, cx| open(window, cx)));
    }
    let save_label = match source.kind {
        MediaKind::Image => t!("Save image"),
        MediaKind::Video => t!("Save video"),
    };
    let save_source = source.clone();
    menu = menu.item(MenuItem::new(save_label).icon(IconName::Download).on_select(move |_, cx| save_media(&save_source, cx)));
    if let Some(decoded) = source.decoded.clone().filter(|_| source.kind == MediaKind::Image) {
        menu = menu.item(MenuItem::new(t!("Copy image")).icon(IconName::Image).on_select(move |_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_image(&decoded.clipboard_image()));
        }));
    }
    menu
}

/// Asks where to save, then writes the media's bytes there.
pub fn save_media(source: &MediaActionSource, cx: &mut App) {
    let directory = std::env::var_os("HOME").map(PathBuf::from).map(|home| home.join("Downloads")).unwrap_or_default();
    let target = cx.prompt_for_new_path(&directory, Some(&source.name));
    let fetch = source.bytes.fetch();
    cx.background_spawn(async move {
        let Ok(Ok(Some(path))) = target.await else { return };
        if let Ok(bytes) = fetch.await {
            std::fs::write(path, &bytes).ok();
        }
    })
    .detach();
}
