//! Keybinding rules: the wire types, the shortcut and `when` parsers, the
//! default keymap, and how user rules layer over it. Ports
//! `packages/contracts/src/keybindings.ts`, `packages/shared/src/keybindings.ts`
//! and `keybindingMatching.ts`.
//!
//! The server owns `keybindings.json`. It merges the user's rules over
//! [`default_keybindings`] with [`merge_with_default_keybindings`] and sends
//! the resolved list in `ServerConfig.keybindings`. The last rule whose
//! shortcut and `when` both match wins, whatever its command.

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// The config holds at most this many rules; older ones fall off the front.
pub const MAX_KEYBINDINGS_COUNT: usize = 256;
/// Nesting limit of a `when` expression (parentheses and `!` runs).
pub const MAX_WHEN_EXPRESSION_DEPTH: usize = 64;
/// Longest project script id in `script.<id>.run`.
pub const MAX_SCRIPT_ID_LENGTH: usize = 24;
const MAX_KEYBINDING_VALUE_LENGTH: usize = 64;
const MAX_KEYBINDING_WHEN_LENGTH: usize = 256;

macro_rules! static_commands {
    ($($variant:ident => $id:literal,)*) => {
        /// Every built-in command id (`STATIC_KEYBINDING_COMMANDS`), in the TypeScript order.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum StaticCommand {
            $($variant,)*
        }

        impl StaticCommand {
            /// All built-in commands, in declaration order.
            pub const ALL: &'static [StaticCommand] = &[$(Self::$variant,)*];

            /// The wire id, such as `"sidebar.toggle"`.
            pub const fn id(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)*
                }
            }

            /// Parses a wire id.
            pub fn from_id(id: &str) -> Option<Self> {
                match id {
                    $($id => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

static_commands! {
    WorkspaceFocusNext => "workspace.focusNext",
    WorkspaceFocusPrevious => "workspace.focusPrevious",
    SidebarToggle => "sidebar.toggle",
    NavigationBack => "navigation.back",
    NavigationForward => "navigation.forward",
    TerminalToggle => "terminal.toggle",
    TerminalSplit => "terminal.split",
    TerminalSplitVertical => "terminal.splitVertical",
    TerminalNew => "terminal.new",
    TerminalClose => "terminal.close",
    RightPanelToggle => "rightPanel.toggle",
    RightPanelNew => "rightPanel.new",
    ThreadPanelToggle => "threadPanel.toggle",
    RightPanelToggleMaximized => "rightPanel.toggleMaximized",
    RightPanelClose => "rightPanel.close",
    ViewReopenClosed => "view.reopenClosed",
    PullRequestCopyNumber => "pullRequest.copyNumber",
    DiffToggle => "diff.toggle",
    PreviewToggle => "preview.toggle",
    PreviewRefresh => "preview.refresh",
    PreviewFocusUrl => "preview.focusUrl",
    PreviewZoomIn => "preview.zoomIn",
    PreviewZoomOut => "preview.zoomOut",
    PreviewResetZoom => "preview.resetZoom",
    PreviewFind => "preview.find",
    CommandPaletteToggle => "commandPalette.toggle",
    FilePickerToggle => "filePicker.toggle",
    ProjectSearchToggle => "projectSearch.toggle",
    UsageOpen => "usage.open",
    ThemeSelect => "theme.select",
    AppearanceCycle => "appearance.cycle",
    ThemeEditorToggle => "themeEditor.toggle",
    ComposerStash => "composer.stash",
    ComposerSendAlternate => "composer.sendAlternate",
    ComposerSendBackground => "composer.sendBackground",
    ComposerSendAndNewThread => "composer.sendAndNewThread",
    ComposerDictation => "composer.dictation",
    ComposerHost => "composer.host",
    ComposerCycleHost => "composer.cycleHost",
    ComposerEffort => "composer.effort",
    ComposerIncreaseEffort => "composer.increaseEffort",
    ComposerDecreaseEffort => "composer.decreaseEffort",
    ComposerMode => "composer.mode",
    ComposerWorkspace => "composer.workspace",
    ComposerPreviousWorktree => "composer.previousWorktree",
    ComposerBranch => "composer.branch",
    ChatNew => "chat.new",
    ChatNewLocal => "chat.newLocal",
    ChatNewWithoutProject => "chat.newWithoutProject",
    ChatFind => "chat.find",
    EditorOpenFavorite => "editor.openFavorite",
    UsageCost => "usage.cost",
    UsageTokens => "usage.tokens",
    UsageLimits => "usage.limits",
    UsagePeriodDay => "usage.period.day",
    UsagePeriodWeek => "usage.period.week",
    UsagePeriodMonth => "usage.period.month",
    UsagePeriodQuarter => "usage.period.quarter",
    ModelPickerToggle => "modelPicker.toggle",
    ModelPickerPreviousProvider => "modelPicker.previousProvider",
    ModelPickerNextProvider => "modelPicker.nextProvider",
    ModelPickerJump1 => "modelPicker.jump.1",
    ModelPickerJump2 => "modelPicker.jump.2",
    ModelPickerJump3 => "modelPicker.jump.3",
    ModelPickerJump4 => "modelPicker.jump.4",
    ModelPickerJump5 => "modelPicker.jump.5",
    ModelPickerJump6 => "modelPicker.jump.6",
    ModelPickerJump7 => "modelPicker.jump.7",
    ModelPickerJump8 => "modelPicker.jump.8",
    ModelPickerJump9 => "modelPicker.jump.9",
    ThreadStop => "thread.stop",
    ThreadSteerQueuedMessage => "thread.steerQueuedMessage",
    ThreadEditQueuedMessage => "thread.editQueuedMessage",
    ThreadPrevious => "thread.previous",
    ThreadNext => "thread.next",
    ThreadCopyReference => "thread.copyReference",
    ThreadSettle => "thread.settle",
    ThreadPin => "thread.pin",
    ThreadRegenerateTitle => "thread.regenerateTitle",
    ThreadUndo => "thread.undo",
    ThreadJump1 => "thread.jump.1",
    ThreadJump2 => "thread.jump.2",
    ThreadJump3 => "thread.jump.3",
    ThreadJump4 => "thread.jump.4",
    ThreadJump5 => "thread.jump.5",
    ThreadJump6 => "thread.jump.6",
    ThreadJump7 => "thread.jump.7",
    ThreadJump8 => "thread.jump.8",
    ThreadJump9 => "thread.jump.9",
}

/// `thread.jump.1` … `thread.jump.9`, in order.
pub const THREAD_JUMP_COMMANDS: [StaticCommand; 9] = [
    StaticCommand::ThreadJump1,
    StaticCommand::ThreadJump2,
    StaticCommand::ThreadJump3,
    StaticCommand::ThreadJump4,
    StaticCommand::ThreadJump5,
    StaticCommand::ThreadJump6,
    StaticCommand::ThreadJump7,
    StaticCommand::ThreadJump8,
    StaticCommand::ThreadJump9,
];

/// `modelPicker.jump.1` … `modelPicker.jump.9`, in order.
pub const MODEL_PICKER_JUMP_COMMANDS: [StaticCommand; 9] = [
    StaticCommand::ModelPickerJump1,
    StaticCommand::ModelPickerJump2,
    StaticCommand::ModelPickerJump3,
    StaticCommand::ModelPickerJump4,
    StaticCommand::ModelPickerJump5,
    StaticCommand::ModelPickerJump6,
    StaticCommand::ModelPickerJump7,
    StaticCommand::ModelPickerJump8,
    StaticCommand::ModelPickerJump9,
];

impl StaticCommand {
    /// The 0-based slot of a `thread.jump.N` command.
    pub fn thread_jump_index(self) -> Option<usize> {
        THREAD_JUMP_COMMANDS.iter().position(|command| *command == self)
    }

    /// The 0-based slot of a `modelPicker.jump.N` command.
    pub fn model_picker_jump_index(self) -> Option<usize> {
        MODEL_PICKER_JUMP_COMMANDS.iter().position(|command| *command == self)
    }
}

/// A project script id from `script.<id>.run`: 1–24 of `[a-z0-9-]`, not starting with `-`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScriptId(Arc<str>);

impl ScriptId {
    /// Validates an id with the contract's pattern.
    pub fn new(id: &str) -> Option<Self> {
        let valid = !id.is_empty()
            && id.len() <= MAX_SCRIPT_ID_LENGTH
            && id.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        valid.then(|| Self(id.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A bindable command id: a built-in command or a project script.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeybindingCommand {
    Static(StaticCommand),
    /// `script.<id>.run`.
    Script(ScriptId),
}

impl KeybindingCommand {
    pub fn as_static(&self) -> Option<StaticCommand> {
        match self {
            Self::Static(command) => Some(*command),
            Self::Script(_) => None,
        }
    }
}

impl From<StaticCommand> for KeybindingCommand {
    fn from(command: StaticCommand) -> Self {
        Self::Static(command)
    }
}

impl PartialEq<StaticCommand> for KeybindingCommand {
    fn eq(&self, other: &StaticCommand) -> bool {
        matches!(self, Self::Static(command) if command == other)
    }
}

impl fmt::Display for KeybindingCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(command) => f.write_str(command.id()),
            Self::Script(id) => write!(f, "script.{}.run", id.as_str()),
        }
    }
}

/// Why a command id did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCommand(pub String);

impl fmt::Display for UnknownCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown command {:?}", self.0)
    }
}

impl FromStr for KeybindingCommand {
    type Err = UnknownCommand;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        if let Some(command) = StaticCommand::from_id(id) {
            return Ok(Self::Static(command));
        }
        id.strip_prefix("script.")
            .and_then(|rest| rest.strip_suffix(".run"))
            .and_then(ScriptId::new)
            .map(Self::Script)
            .ok_or_else(|| UnknownCommand(id.to_owned()))
    }
}

impl Serialize for KeybindingCommand {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for KeybindingCommand {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        id.parse().map_err(de::Error::custom)
    }
}

/// One rule as written in `keybindings.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeybindingRule {
    pub key: String,
    pub command: KeybindingCommand,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

impl KeybindingRule {
    pub fn new(key: impl Into<String>, command: impl Into<KeybindingCommand>, when: Option<&str>) -> Self {
        Self { key: key.into(), command: command.into(), when: when.map(str::to_owned) }
    }
}

/// A parsed key chord. `key` is a lowercased `KeyboardEvent.key` ("k",
/// "arrowup", "enter", " " for space); `mod` is Cmd on macOS and Ctrl elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingShortcut {
    pub key: String,
    pub meta_key: bool,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub mod_key: bool,
}

/// A parsed `when` expression.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WhenNode {
    Identifier { name: Arc<str> },
    Not { node: Box<WhenNode> },
    And { left: Box<WhenNode>, right: Box<WhenNode> },
    Or { left: Box<WhenNode>, right: Box<WhenNode> },
}

impl WhenNode {
    pub fn identifier(name: &str) -> Self {
        Self::Identifier { name: name.into() }
    }

    pub fn not(node: WhenNode) -> Self {
        Self::Not { node: Box::new(node) }
    }

    pub fn and(left: WhenNode, right: WhenNode) -> Self {
        Self::And { left: Box::new(left), right: Box::new(right) }
    }

    pub fn or(left: WhenNode, right: WhenNode) -> Self {
        Self::Or { left: Box::new(left), right: Box::new(right) }
    }

    /// The expression as the Settings page writes it (`whenAstToExpression`).
    pub fn to_expression(&self) -> String {
        fn wrap(node: &WhenNode) -> String {
            match node {
                WhenNode::Identifier { .. } | WhenNode::Not { .. } => node.to_expression(),
                _ => format!("({})", node.to_expression()),
            }
        }
        match self {
            Self::Identifier { name } => name.to_string(),
            Self::Not { node } => format!("!{}", wrap(node)),
            Self::And { left, right } => format!("{} && {}", wrap(left), wrap(right)),
            Self::Or { left, right } => format!("{} || {}", wrap(left), wrap(right)),
        }
    }
}

/// A rule after parsing: what the server sends in `ServerConfig.keybindings`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedKeybindingRule {
    pub command: KeybindingCommand,
    pub shortcut: KeybindingShortcut,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_ast: Option<WhenNode>,
}

/// The resolved rules in precedence order (later wins). Decoding drops
/// entries this build does not understand, such as commands from a newer
/// server, instead of failing the whole config (`ForwardCompatibleArray`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ResolvedKeybindingsConfig(pub Vec<ResolvedKeybindingRule>);

impl ResolvedKeybindingsConfig {
    pub fn rules(&self) -> &[ResolvedKeybindingRule] {
        &self.0
    }
}

/// Decodes `T`, or skips a value it cannot decode.
enum Lenient<T> {
    Ok(T),
    Skipped,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Buffered so a failed decode can still consume the value.
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(T::deserialize(value).map_or(Self::Skipped, Self::Ok))
    }
}

/// A JSON array decoded element by element, dropping the ones that fail.
fn lenient_vec<'de, D: Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> Result<Vec<T>, D::Error> {
    struct LenientVisitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for LenientVisitor<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(item) = seq.next_element::<Lenient<T>>()? {
                if let Lenient::Ok(item) = item {
                    out.push(item);
                }
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(LenientVisitor(std::marker::PhantomData))
}

impl<'de> Deserialize<'de> for ResolvedKeybindingsConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut rules: Vec<ResolvedKeybindingRule> = lenient_vec(deserializer)?;
        if rules.len() > MAX_KEYBINDINGS_COUNT {
            rules.drain(..rules.len() - MAX_KEYBINDINGS_COUNT);
        }
        Ok(Self(rules))
    }
}

/// A problem the server found in `keybindings.json` (`ServerConfigIssue`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ServerConfigIssue {
    /// The file did not parse; the server fell back to the defaults.
    #[serde(rename = "keybindings.malformed-config")]
    MalformedConfig { message: String },
    /// One rule was ignored.
    #[serde(rename = "keybindings.invalid-entry")]
    InvalidEntry { index: u32, message: String },
}

impl ServerConfigIssue {
    pub fn message(&self) -> &str {
        match self {
            Self::MalformedConfig { message } | Self::InvalidEntry { message, .. } => message,
        }
    }
}

/// `ServerConfig.issues`, dropping kinds this build does not know.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ServerConfigIssues(pub Vec<ServerConfigIssue>);

impl<'de> Deserialize<'de> for ServerConfigIssues {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        lenient_vec(deserializer).map(Self)
    }
}

/// The keybinding part of `ServerConfig`; other fields are ignored here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerKeybindings {
    #[serde(default)]
    pub keybindings_config_path: Option<String>,
    #[serde(default)]
    pub keybindings: ResolvedKeybindingsConfig,
    #[serde(default)]
    pub issues: ServerConfigIssues,
}

/// Shifted characters on a US layout and the key that types them, for
/// recording shortcuts (`US_SHIFTED_TO_UNSHIFTED_KEYS`).
pub const US_SHIFTED_TO_UNSHIFTED_KEYS: [(char, char); 21] = [
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('~', '`'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
];

use StaticCommand as C;

/// The built-in keymap, verbatim from `DEFAULT_KEYBINDINGS`: (key, command, when).
const DEFAULT_KEYBINDING_TABLE: &[(&str, StaticCommand, Option<&str>)] = &[
    ("mod+b", C::SidebarToggle, None),
    ("f6", C::WorkspaceFocusNext, None),
    ("shift+f6", C::WorkspaceFocusPrevious, None),
    // Some browsers keep plain F6 for their own toolbars; mod+F6 reaches the page in all of them.
    ("mod+f6", C::WorkspaceFocusNext, None),
    ("mod+shift+f6", C::WorkspaceFocusPrevious, None),
    ("mod+[", C::NavigationBack, Some("!terminalFocus")),
    ("mod+]", C::NavigationForward, Some("!terminalFocus")),
    ("mod+j", C::TerminalToggle, None),
    ("mod+alt+b", C::RightPanelToggle, None),
    ("mod+shift+b", C::ThreadPanelToggle, Some("!terminalFocus")),
    ("mod+t", C::RightPanelNew, Some("rightPanelOpen && !terminalFocus")),
    ("mod+d", C::TerminalSplit, Some("terminalFocus")),
    ("mod+shift+d", C::TerminalSplitVertical, Some("terminalFocus")),
    ("mod+shift+d", C::ComposerDictation, Some("!terminalFocus")),
    ("mod+n", C::TerminalNew, Some("terminalFocus")),
    ("mod+w", C::TerminalClose, Some("terminalFocus")),
    ("mod+w", C::RightPanelClose, Some("!terminalFocus")),
    ("mod+shift+t", C::ViewReopenClosed, None),
    ("mod+d", C::DiffToggle, Some("!terminalFocus")),
    ("mod+shift+j", C::PreviewToggle, None),
    ("mod+r", C::PreviewRefresh, Some("previewFocus")),
    ("mod+l", C::PreviewFocusUrl, Some("previewFocus")),
    ("mod+=", C::PreviewZoomIn, Some("previewFocus")),
    ("mod++", C::PreviewZoomIn, Some("previewFocus")),
    ("mod+-", C::PreviewZoomOut, Some("previewFocus")),
    ("mod+0", C::PreviewResetZoom, Some("previewFocus")),
    ("mod+f", C::PreviewFind, Some("previewFocus")),
    ("mod+k", C::CommandPaletteToggle, Some("!terminalFocus")),
    ("mod+p", C::FilePickerToggle, Some("!terminalFocus")),
    ("mod+shift+f", C::ProjectSearchToggle, Some("!terminalFocus")),
    ("mod+u", C::UsageOpen, Some("!terminalFocus")),
    ("mod+alt+a", C::ThemeSelect, Some("!terminalFocus")),
    ("mod+alt+shift+a", C::AppearanceCycle, Some("!terminalFocus")),
    ("mod+alt+shift+t", C::ThemeEditorToggle, None),
    ("mod+s", C::ComposerStash, Some("!terminalFocus")),
    ("mod+shift+enter", C::ThreadSteerQueuedMessage, Some("!terminalFocus")),
    ("alt+arrowup", C::ThreadEditQueuedMessage, Some("composerFocus")),
    ("mod+enter", C::ComposerSendAlternate, Some("composerFocus && turnRunning")),
    ("mod+enter", C::ComposerSendBackground, Some("composerFocus && draftThreadRoute")),
    ("mod+alt+enter", C::ComposerSendBackground, Some("composerFocus && draftThreadRoute")),
    ("mod+alt+enter", C::ComposerSendAndNewThread, Some("composerFocus && !draftThreadRoute")),
    ("mod+n", C::ChatNew, Some("!terminalFocus")),
    ("mod+shift+o", C::ChatNew, Some("!terminalFocus")),
    ("mod+shift+n", C::ChatNewLocal, Some("!terminalFocus")),
    ("mod+alt+n", C::ChatNewWithoutProject, Some("!terminalFocus")),
    ("mod+f", C::ChatFind, Some("!terminalFocus && !previewFocus")),
    ("mod+shift+m", C::ModelPickerToggle, Some("!terminalFocus")),
    ("mod+shift+h", C::ComposerHost, Some("!terminalFocus")),
    ("mod+shift+e", C::ComposerEffort, Some("!terminalFocus")),
    ("mod+shift+a", C::ComposerMode, Some("!terminalFocus")),
    ("mod+shift+x", C::ComposerWorkspace, Some("!terminalFocus")),
    ("mod+shift+g", C::ComposerBranch, Some("!terminalFocus")),
    ("mod+shift+l", C::ComposerPreviousWorktree, Some("!terminalFocus")),
    ("mod+shift+k", C::PullRequestCopyNumber, Some("!terminalFocus")),
    ("mod+shift+arrowup", C::ModelPickerPreviousProvider, Some("modelPickerOpen")),
    ("mod+shift+arrowdown", C::ModelPickerNextProvider, Some("modelPickerOpen")),
    ("mod+o", C::EditorOpenFavorite, None),
    ("mod+shift+[", C::ThreadPrevious, None),
    ("mod+shift+]", C::ThreadNext, None),
    ("mod+shift+c", C::ThreadCopyReference, Some("!terminalFocus")),
    ("mod+shift+s", C::ThreadSettle, Some("!terminalFocus")),
    ("mod+shift+p", C::ThreadPin, Some("!terminalFocus")),
    ("mod+z", C::ThreadUndo, Some("!terminalFocus && !editableFocus")),
    ("mod+1", C::ThreadJump1, Some("!modelPickerOpen && isDesktop")),
    ("mod+2", C::ThreadJump2, Some("!modelPickerOpen && isDesktop")),
    ("mod+3", C::ThreadJump3, Some("!modelPickerOpen && isDesktop")),
    ("mod+4", C::ThreadJump4, Some("!modelPickerOpen && isDesktop")),
    ("mod+5", C::ThreadJump5, Some("!modelPickerOpen && isDesktop")),
    ("mod+6", C::ThreadJump6, Some("!modelPickerOpen && isDesktop")),
    ("mod+7", C::ThreadJump7, Some("!modelPickerOpen && isDesktop")),
    ("mod+8", C::ThreadJump8, Some("!modelPickerOpen && isDesktop")),
    ("mod+9", C::ThreadJump9, Some("!modelPickerOpen && isDesktop")),
    ("mod+1", C::ModelPickerJump1, Some("modelPickerOpen && isDesktop")),
    ("mod+2", C::ModelPickerJump2, Some("modelPickerOpen && isDesktop")),
    ("mod+3", C::ModelPickerJump3, Some("modelPickerOpen && isDesktop")),
    ("mod+4", C::ModelPickerJump4, Some("modelPickerOpen && isDesktop")),
    ("mod+5", C::ModelPickerJump5, Some("modelPickerOpen && isDesktop")),
    ("mod+6", C::ModelPickerJump6, Some("modelPickerOpen && isDesktop")),
    ("mod+7", C::ModelPickerJump7, Some("modelPickerOpen && isDesktop")),
    ("mod+8", C::ModelPickerJump8, Some("modelPickerOpen && isDesktop")),
    ("mod+9", C::ModelPickerJump9, Some("modelPickerOpen && isDesktop")),
    ("c", C::UsageCost, Some("usagePageOpen")),
    ("t", C::UsageTokens, Some("usagePageOpen")),
    ("l", C::UsageLimits, Some("usagePageOpen")),
    ("mod+shift+1", C::UsagePeriodDay, Some("usagePageOpen")),
    ("mod+shift+2", C::UsagePeriodWeek, Some("usagePageOpen")),
    ("mod+shift+3", C::UsagePeriodMonth, Some("usagePageOpen")),
    ("mod+shift+4", C::UsagePeriodQuarter, Some("usagePageOpen")),
];

/// The default keymap as rules (`DEFAULT_KEYBINDINGS`).
pub fn default_keybindings() -> Vec<KeybindingRule> {
    DEFAULT_KEYBINDING_TABLE.iter().map(|(key, command, when)| KeybindingRule::new(*key, *command, *when)).collect()
}

/// The default keymap, compiled once (`DEFAULT_RESOLVED_KEYBINDINGS`).
pub fn default_resolved_keybindings() -> &'static ResolvedKeybindingsConfig {
    static DEFAULTS: OnceLock<ResolvedKeybindingsConfig> = OnceLock::new();
    DEFAULTS.get_or_init(|| compile_resolved_keybindings_config(&default_keybindings()))
}

fn normalize_key_token(token: &str) -> String {
    match token {
        "space" => " ".to_owned(),
        "esc" => "escape".to_owned(),
        _ => token.to_owned(),
    }
}

/// Parses `"mod+shift+k"` (`parseKeybindingShortcut`). A trailing `+` is the
/// plus key (`"mod++"`); two non-modifier keys fail.
pub fn parse_keybinding_shortcut(value: &str) -> Option<KeybindingShortcut> {
    let lowered = value.to_lowercase();
    let mut tokens: Vec<&str> = lowered.split('+').map(str::trim).collect();
    let mut trailing_empty = 0;
    while tokens.last() == Some(&"") {
        trailing_empty += 1;
        tokens.pop();
    }
    if trailing_empty > 0 {
        tokens.push("+");
    }
    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        return None;
    }
    let mut shortcut =
        KeybindingShortcut { key: String::new(), meta_key: false, ctrl_key: false, shift_key: false, alt_key: false, mod_key: false };
    let mut key = None;
    for token in tokens {
        match token {
            "cmd" | "meta" => shortcut.meta_key = true,
            "ctrl" | "control" => shortcut.ctrl_key = true,
            "shift" => shortcut.shift_key = true,
            "alt" | "option" => shortcut.alt_key = true,
            "mod" => shortcut.mod_key = true,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(normalize_key_token(other));
            }
        }
    }
    shortcut.key = key?;
    Some(shortcut)
}

/// The settings spelling of a shortcut (`shortcutToKeybindingInput`).
pub fn shortcut_to_keybinding_input(shortcut: &KeybindingShortcut) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(6);
    if shortcut.mod_key {
        parts.push("mod");
    }
    if shortcut.meta_key {
        parts.push("meta");
    }
    if shortcut.ctrl_key {
        parts.push("ctrl");
    }
    if shortcut.alt_key {
        parts.push("alt");
    }
    if shortcut.shift_key {
        parts.push("shift");
    }
    parts.push(match shortcut.key.as_str() {
        " " => "space",
        "escape" => "esc",
        key => key,
    });
    parts.join("+")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhenToken<'a> {
    Identifier(&'a str),
    Not,
    And,
    Or,
    LParen,
    RParen,
}

fn tokenize_when(expression: &str) -> Option<Vec<WhenToken<'_>>> {
    let bytes = expression.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &expression[index..];
        let current = rest.chars().next()?;
        if current.is_whitespace() {
            index += current.len_utf8();
            continue;
        }
        if rest.starts_with("&&") {
            tokens.push(WhenToken::And);
            index += 2;
            continue;
        }
        if rest.starts_with("||") {
            tokens.push(WhenToken::Or);
            index += 2;
            continue;
        }
        match current {
            '!' => tokens.push(WhenToken::Not),
            '(' => tokens.push(WhenToken::LParen),
            ')' => tokens.push(WhenToken::RParen),
            c if c.is_ascii_alphabetic() || c == '_' => {
                let len = rest
                    .bytes()
                    .position(|b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-'))
                    .unwrap_or(rest.len());
                tokens.push(WhenToken::Identifier(&rest[..len]));
                index += len;
                continue;
            }
            _ => return None,
        }
        index += 1;
    }
    Some(tokens)
}

struct WhenParser<'a> {
    tokens: Vec<WhenToken<'a>>,
    index: usize,
}

impl WhenParser<'_> {
    fn peek(&self) -> Option<WhenToken<'_>> {
        self.tokens.get(self.index).copied()
    }

    fn primary(&mut self, depth: usize) -> Option<WhenNode> {
        if depth > MAX_WHEN_EXPRESSION_DEPTH {
            return None;
        }
        match self.peek()? {
            WhenToken::Identifier(name) => {
                let node = WhenNode::identifier(name);
                self.index += 1;
                Some(node)
            }
            WhenToken::LParen => {
                self.index += 1;
                let node = self.or(depth + 1)?;
                if self.peek()? != WhenToken::RParen {
                    return None;
                }
                self.index += 1;
                Some(node)
            }
            _ => None,
        }
    }

    fn unary(&mut self, depth: usize) -> Option<WhenNode> {
        let mut nots = 0;
        while self.peek() == Some(WhenToken::Not) {
            self.index += 1;
            nots += 1;
            if nots > MAX_WHEN_EXPRESSION_DEPTH {
                return None;
            }
        }
        let mut node = self.primary(depth)?;
        for _ in 0..nots {
            node = WhenNode::not(node);
        }
        Some(node)
    }

    fn and(&mut self, depth: usize) -> Option<WhenNode> {
        let mut left = self.unary(depth)?;
        while self.peek() == Some(WhenToken::And) {
            self.index += 1;
            left = WhenNode::and(left, self.unary(depth)?);
        }
        Some(left)
    }

    fn or(&mut self, depth: usize) -> Option<WhenNode> {
        let mut left = self.and(depth)?;
        while self.peek() == Some(WhenToken::Or) {
            self.index += 1;
            left = WhenNode::or(left, self.and(depth)?);
        }
        Some(left)
    }
}

/// Parses a `when` expression: identifiers with `!`, `&&`, `||` and
/// parentheses, `&&` binding tighter (`parseKeybindingWhenExpression`).
pub fn parse_keybinding_when_expression(expression: &str) -> Option<WhenNode> {
    let tokens = tokenize_when(expression)?;
    if tokens.is_empty() {
        return None;
    }
    let mut parser = WhenParser { tokens, index: 0 };
    let ast = parser.or(0)?;
    (parser.index == parser.tokens.len()).then_some(ast)
}

/// Why a rule was not compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidRuleReason {
    /// `key` is empty, too long, or names two keys.
    Shortcut,
    /// `when` is empty, too long, or does not parse.
    When,
}

/// Compiles one rule, or says why it is invalid (`compileResolvedKeybindingRule`).
pub fn compile_keybinding_rule(rule: &KeybindingRule) -> Result<ResolvedKeybindingRule, InvalidRuleReason> {
    let key = rule.key.trim();
    if key.is_empty() || key.chars().count() > MAX_KEYBINDING_VALUE_LENGTH {
        return Err(InvalidRuleReason::Shortcut);
    }
    let shortcut = parse_keybinding_shortcut(key).ok_or(InvalidRuleReason::Shortcut)?;
    let when_ast = match rule.when.as_deref().map(str::trim) {
        None => None,
        Some(when) if when.is_empty() || when.chars().count() > MAX_KEYBINDING_WHEN_LENGTH => {
            return Err(InvalidRuleReason::When);
        }
        Some(when) => Some(parse_keybinding_when_expression(when).ok_or(InvalidRuleReason::When)?),
    };
    Ok(ResolvedKeybindingRule { command: rule.command.clone(), shortcut, when_ast })
}

/// Compiles rules, skipping invalid ones and keeping the last 256
/// (`compileResolvedKeybindingsConfig`).
pub fn compile_resolved_keybindings_config(config: &[KeybindingRule]) -> ResolvedKeybindingsConfig {
    let mut compiled: Vec<_> = config.iter().filter_map(|rule| compile_keybinding_rule(rule).ok()).collect();
    if compiled.len() > MAX_KEYBINDINGS_COUNT {
        compiled.drain(..compiled.len() - MAX_KEYBINDINGS_COUNT);
    }
    ResolvedKeybindingsConfig(compiled)
}

/// User rules over the defaults (`mergeWithDefaultKeybindings`): a command
/// the user bound keeps none of its defaults, so a rule with `when: "false"`
/// removes a default shortcut. The user's rules come last and win.
pub fn merge_with_default_keybindings(custom: &ResolvedKeybindingsConfig) -> ResolvedKeybindingsConfig {
    let defaults = default_resolved_keybindings();
    if custom.0.is_empty() {
        return defaults.clone();
    }
    let mut merged: Vec<_> = defaults
        .0
        .iter()
        .filter(|binding| !custom.0.iter().any(|rule| rule.command == binding.command))
        .cloned()
        .collect();
    merged.extend(custom.0.iter().cloned());
    if merged.len() > MAX_KEYBINDINGS_COUNT {
        merged.drain(..merged.len() - MAX_KEYBINDINGS_COUNT);
    }
    ResolvedKeybindingsConfig(merged)
}

/// Which modifier key `mod` means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutPlatform {
    /// macOS and iOS: `mod` is Command, labels use ⌘⌥⇧⌃ glyphs.
    Mac,
    /// Windows and Linux: `mod` is Control.
    Other,
}

impl ShortcutPlatform {
    /// The platform this binary runs on.
    pub const fn current() -> Self {
        if cfg!(any(target_os = "macos", target_os = "ios")) { Self::Mac } else { Self::Other }
    }

    pub fn uses_meta_for_mod(self) -> bool {
        self == Self::Mac
    }
}

/// A shortcut's physical chord on one platform, `mod` resolved. Two rules
/// with the same key here fire on the same keystroke (`shortcutConflictKey`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShortcutChord {
    pub key: String,
    pub meta: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeybindingShortcut {
    /// The chord this shortcut means on `platform`.
    pub fn chord(&self, platform: ShortcutPlatform) -> ShortcutChord {
        let meta_for_mod = platform.uses_meta_for_mod();
        ShortcutChord {
            key: self.key.clone(),
            meta: self.meta_key || (self.mod_key && meta_for_mod),
            ctrl: self.ctrl_key || (self.mod_key && !meta_for_mod),
            shift: self.shift_key,
            alt: self.alt_key,
        }
    }
}

/// Held modifiers, as on a keyboard event (`ShortcutModifierStateLike`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModifierKeys {
    pub meta: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Whether the held modifiers are exactly the shortcut's (`matchesKeybindingShortcutModifiers`).
pub fn matches_shortcut_modifiers(held: ModifierKeys, shortcut: &KeybindingShortcut, platform: ShortcutPlatform) -> bool {
    let chord = shortcut.chord(platform);
    held.meta == chord.meta && held.ctrl == chord.ctrl && held.shift == shortcut.shift_key && held.alt == shortcut.alt_key
}

/// Lowercases a key name; `esc` becomes `escape` (`normalizeEventKey`).
pub fn normalize_event_key(key: &str) -> String {
    let lowered = key.to_lowercase();
    if lowered == "esc" { "escape".to_owned() } else { lowered }
}

/// A conflict between two rules on the same chord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingConflict {
    /// Index of the earlier rule, which loses wherever both apply.
    pub shadowed: usize,
    /// Index of the later rule.
    pub winner: usize,
}

/// What [`report_keybindings`] found in the user's rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeybindingsReport {
    /// Rules that were dropped, by their index in the user's list.
    pub invalid: Vec<(usize, InvalidRuleReason)>,
    /// Pairs of resolved rules that share a chord where both can apply, by
    /// their index in the merged config: same chord and the same `when`, or
    /// one without a `when`. Rules with `when: "false"` are removals and
    /// never conflict (`keybindingConflictLabels`).
    pub conflicts: Vec<KeybindingConflict>,
}

/// Reports invalid user rules and conflicting rules in a merged config.
pub fn report_keybindings(
    custom: &[KeybindingRule],
    merged: &ResolvedKeybindingsConfig,
    platform: ShortcutPlatform,
) -> KeybindingsReport {
    let invalid = custom
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| compile_keybinding_rule(rule).err().map(|reason| (index, reason)))
        .collect();
    let rules = merged.rules();
    let whens: Vec<String> = rules.iter().map(|rule| rule.when_ast.as_ref().map(WhenNode::to_expression).unwrap_or_default()).collect();
    let chords: Vec<ShortcutChord> = rules.iter().map(|rule| rule.shortcut.chord(platform)).collect();
    let mut conflicts = Vec::new();
    for winner in 0..rules.len() {
        for shadowed in 0..winner {
            let (left, right) = (&whens[shadowed], &whens[winner]);
            let overlap = left != "false" && right != "false" && (left.is_empty() || right.is_empty() || left == right);
            if chords[shadowed] == chords[winner] && overlap && rules[shadowed].command != rules[winner].command {
                conflicts.push(KeybindingConflict { shadowed, winner });
            }
        }
    }
    KeybindingsReport { invalid, conflicts }
}
