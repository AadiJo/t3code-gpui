//! Which editor "Open" uses. Ports `editorPreferences.ts` and `editorLabels.ts`:
//! the last editor the user picked when it is still available, else VS Code,
//! else the first available editor in `EDITORS` order.

use gpui_kit::{App, Global, SharedString};
use t3code_contracts::EditorId;

use crate::i18n::t;

/// `EDITORS` from `contracts/src/editor.ts`: id, label, remote URL scheme and
/// JetBrains product code, in the order the picker lists them.
pub const EDITORS: &[(EditorId, &str, Option<&str>, Option<&str>)] = &[
    (EditorId::Cursor, "Cursor", Some("cursor"), None),
    (EditorId::Trae, "Trae", None, None),
    (EditorId::Kiro, "Kiro", None, None),
    (EditorId::Vscode, "VS Code", Some("vscode"), None),
    (EditorId::VscodeInsiders, "VS Code Insiders", Some("vscode-insiders"), None),
    (EditorId::Vscodium, "VSCodium", Some("vscodium"), None),
    (EditorId::Zed, "Zed", Some("zed"), None),
    (EditorId::Antigravity, "Antigravity", None, None),
    (EditorId::Idea, "IntelliJ IDEA", None, Some("IU")),
    (EditorId::Aqua, "Aqua", None, Some("QA")),
    (EditorId::Clion, "CLion", None, Some("CL")),
    (EditorId::Datagrip, "DataGrip", None, Some("DB")),
    (EditorId::Dataspell, "DataSpell", None, Some("DS")),
    (EditorId::Goland, "GoLand", None, Some("GO")),
    (EditorId::Phpstorm, "PhpStorm", None, Some("PS")),
    (EditorId::Pycharm, "PyCharm", None, Some("PY")),
    (EditorId::Rider, "Rider", None, Some("RD")),
    (EditorId::Rubymine, "RubyMine", None, Some("RM")),
    (EditorId::Rustrover, "RustRover", None, Some("RR")),
    (EditorId::Webstorm, "WebStorm", None, Some("WS")),
    (EditorId::FileManager, "File Manager", None, None),
];

/// The desktop platform, which names the file manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this binary was built for.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// `editorLabelForPlatform`.
pub fn editor_label(editor: EditorId, platform: Platform) -> SharedString {
    if editor == EditorId::FileManager {
        return match platform {
            Platform::Mac => SharedString::new_static("Finder"),
            Platform::Windows => t!("File Explorer", context = "the Windows file manager app"),
            Platform::Linux => t!("Files", context = "the Linux file manager app"),
        };
    }
    EDITORS
        .iter()
        .find(|(id, ..)| *id == editor)
        .map(|(_, label, ..)| SharedString::new_static(label))
        .unwrap_or_else(|| t!("Editor", context = "noun, a code editor app"))
}

/// `remoteSchemeForEditor`: editors that open a path over SSH from a URL.
pub fn remote_scheme_for_editor(editor: EditorId) -> Option<&'static str> {
    EDITORS
        .iter()
        .find(|(id, ..)| *id == editor)
        .and_then(|(_, _, scheme, jetbrains)| scheme.or(jetbrains.map(|_| "jetbrains")))
}

/// `defaultEditor`. VS Code wins over list order: the Cursor agent CLI installs
/// a `cursor` shim that the editor probe reports as the Cursor editor.
pub fn default_editor(available: &[EditorId]) -> Option<EditorId> {
    if available.contains(&EditorId::Vscode) {
        return Some(EditorId::Vscode);
    }
    EDITORS.iter().map(|(id, ..)| *id).find(|id| available.contains(id))
}

/// `usePreferredEditor`: the remembered editor while available, else the default.
pub fn effective_editor(last_editor: Option<EditorId>, available: &[EditorId]) -> Option<EditorId> {
    last_editor.filter(|e| available.contains(e)).or_else(|| default_editor(available))
}

/// The editor the user last opened with (`t3code:last-editor`). Kept for the
/// process; the settings layer persists it.
#[derive(Default)]
pub struct LastEditor(pub Option<EditorId>);

impl Global for LastEditor {}

impl LastEditor {
    pub fn get(cx: &App) -> Option<EditorId> {
        cx.try_global::<Self>().and_then(|l| l.0)
    }

    pub fn set(editor: EditorId, cx: &mut App) {
        cx.set_global(Self(Some(editor)));
    }
}
