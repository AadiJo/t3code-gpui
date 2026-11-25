//! The file context menu shared by the file tree, file chips, diff headers and the
//! changed-files card. Ports `fileContextMenu.ts`: "Open", "Reveal in …" and "Open with"
//! act on the server's machine through `shell.openInEditor` (`live::host_menus`), so they
//! only appear when the server lists the editor.

use gpui_kit::SharedString;
use t3code_contracts::EditorId;

use crate::{components::icons::IconName, i18n::t};

/// What a menu row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileContextMenuAction {
    /// `reveal-in-folder`.
    RevealInFolder,
    /// `editor:<id>`.
    OpenWith(EditorId),
    /// `open`: the host's default app, through `shell.openInEditor` with `file-manager`.
    Open,
    /// The tree's own rows.
    CopyMention,
    AddToChat,
}

/// One row; `children` makes it a submenu ("Open with").
#[derive(Clone, Debug)]
pub struct FileContextMenuItem {
    pub action: Option<FileContextMenuAction>,
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub children: Vec<FileContextMenuItem>,
}

/// How the server's host reveals files, from its config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealKind {
    Finder,
    FileExplorer,
    Files,
}

/// `FileContextMenuCapabilities`, from the server config.
#[derive(Clone, Debug, Default)]
pub struct FileContextMenuCapabilities {
    pub reveal: Option<RevealKind>,
    pub editor_ids: Vec<EditorId>,
}

/// `revealInFileExplorerLabelForKind`.
pub fn reveal_label(kind: RevealKind) -> SharedString {
    match kind {
        RevealKind::Finder => t!("Reveal in Finder"),
        RevealKind::FileExplorer => t!("Reveal in File Explorer"),
        RevealKind::Files => t!("Reveal in Files"),
    }
}

/// `EDITORS[].label`.
pub fn editor_label(editor: &EditorId) -> &'static str {
    match editor {
        EditorId::Cursor => "Cursor",
        EditorId::Trae => "Trae",
        EditorId::Kiro => "Kiro",
        EditorId::Vscode => "VS Code",
        EditorId::VscodeInsiders => "VS Code Insiders",
        EditorId::Vscodium => "VSCodium",
        EditorId::Zed => "Zed",
        EditorId::Antigravity => "Antigravity",
        EditorId::Idea => "IntelliJ IDEA",
        EditorId::Aqua => "Aqua",
        EditorId::Clion => "CLion",
        EditorId::Datagrip => "DataGrip",
        EditorId::Dataspell => "DataSpell",
        EditorId::Goland => "GoLand",
        EditorId::Phpstorm => "PhpStorm",
        EditorId::Pycharm => "PyCharm",
        EditorId::Rider => "Rider",
        EditorId::Rubymine => "RubyMine",
        EditorId::Rustrover => "RustRover",
        EditorId::Webstorm => "WebStorm",
        EditorId::FileManager => "File Manager",
    }
}

/// `buildFileContextMenuItems`: Open (when the host has a file manager),
/// Reveal, and Open with › each editor. Every action runs on the host.
pub fn file_context_menu_items(capabilities: &FileContextMenuCapabilities, has_absolute_path: bool) -> Vec<FileContextMenuItem> {
    let mut items = Vec::new();
    if has_absolute_path {
        if capabilities.editor_ids.contains(&EditorId::FileManager) {
            items.push(item(FileContextMenuAction::Open, t!("Open", context = "verb, opens a file"), Some(IconName::Pencil)));
        }
        if let Some(kind) = capabilities.reveal {
            items.push(item(FileContextMenuAction::RevealInFolder, reveal_label(kind), Some(IconName::FolderTree)));
        }
        let editors: Vec<_> = capabilities.editor_ids.iter().filter(|id| **id != EditorId::FileManager).cloned().collect();
        if !editors.is_empty() {
            items.push(FileContextMenuItem {
                action: None,
                label: t!("Open with"),
                icon: None,
                children: editors
                    .into_iter()
                    .map(|id| {
                        let label = editor_label(&id).into();
                        item(FileContextMenuAction::OpenWith(id), label, None)
                    })
                    .collect(),
            });
        }
    }
    items
}

/// The file tree's menu: the file menu, then "Copy mention" and "Add to chat".
pub fn file_tree_menu_items(capabilities: &FileContextMenuCapabilities, has_absolute_path: bool) -> Vec<FileContextMenuItem> {
    let mut items = file_context_menu_items(capabilities, has_absolute_path);
    items.push(item(FileContextMenuAction::CopyMention, t!("Copy mention"), None));
    items.push(item(FileContextMenuAction::AddToChat, t!("Add to chat"), None));
    items
}

fn item(action: FileContextMenuAction, label: SharedString, icon: Option<IconName>) -> FileContextMenuItem {
    FileContextMenuItem { action: Some(action), label, icon, children: Vec::new() }
}

/// The menu for `items`. A submenu ("Open with") shows as its label with its rows inset under it.
pub fn file_menu_popup(
    items: &[FileContextMenuItem],
    on_action: std::rc::Rc<dyn Fn(FileContextMenuAction, &mut gpui_kit::Window, &mut gpui_kit::App)>,
) -> crate::components::ui::menu::MenuPopup {
    use crate::components::ui::menu::{MenuItem, MenuPopup};
    let row = |item: &FileContextMenuItem| {
        let mut row = MenuItem::new(item.label.clone());
        if let Some(icon) = item.icon {
            row = row.icon(icon);
        }
        if let Some(action) = item.action.clone() {
            let on_action = on_action.clone();
            row = row.on_select(move |window, cx| on_action(action.clone(), window, cx));
        }
        row
    };
    items.iter().fold(MenuPopup::new().context(), |popup, item| {
        if item.children.is_empty() {
            popup.item(row(item))
        } else {
            item.children.iter().fold(popup.label(item.label.clone()), |popup, child| popup.item(row(child).inset()))
        }
    })
}

/// `serializeComposerFileLink`: the markdown link a file chip round-trips as.
pub fn composer_file_mention(path: &str) -> String {
    crate::composer_editor_mentions::serialize_file_link(path)
}
