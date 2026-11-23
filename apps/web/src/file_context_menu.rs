//! The file context menu shared by the file tree, file chips and diff headers.
//! Ports `fileContextMenu.ts`: "Open", "Reveal in …" and "Open with" act on
//! the server's machine through `shell.openInEditor`, so they only appear
//! when the server lists the editor. "Open" differs: the web asks the server's
//! file manager, which for a remote server opens on the wrong machine, so here
//! it downloads the file and opens it with this machine's default app.

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
    /// Native only: fetch the file and open it with this machine's default app.
    OpenLocally,
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

/// `buildFileContextMenuItems`, with "Open" acting on this machine.
pub fn file_context_menu_items(capabilities: &FileContextMenuCapabilities, has_absolute_path: bool) -> Vec<FileContextMenuItem> {
    let mut items = Vec::new();
    if has_absolute_path {
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
    items.insert(0, item(FileContextMenuAction::OpenLocally, t!("Open", context = "verb, opens a file"), Some(IconName::Pencil)));
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

/// `serializeComposerFileLink`: the markdown link a file chip round-trips as.
pub fn composer_file_mention(path: &str) -> String {
    crate::composer_editor_mentions::serialize_file_link(path)
}
