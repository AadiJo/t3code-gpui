//! The command registry: every bindable command as a typed GPUI action, with
//! its title, category, default shortcuts and availability. Titles are the
//! Settings → Keybindings labels (`KeybindingsSettings.logic.ts`).
//!
//! A view that owns a command handles its action with `.on_action(...)` on an
//! element that is an ancestor of every focus target the command should work
//! from. A command whose action has no handler on the focused path is
//! unavailable: keystrokes fall through to the focused widget, and the
//! palette lists it as unavailable.

use std::any::TypeId;
use std::rc::Rc;

// `gpui` keeps the `Action` derive's generated `gpui::` paths resolving.
#[allow(unused_imports)]
use gpui_kit::{Action, App, FocusHandle, InteractiveElement, SharedString, Window, gpui};
use t3code_shared::keybindings::{
    KeybindingCommand, ResolvedKeybindingRule, ScriptId, StaticCommand, default_resolved_keybindings,
};

use crate::i18n::t;

/// Where a command sits in Settings → Keybindings and the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CommandCategory {
    Workspace,
    Sidebar,
    Navigation,
    Terminal,
    RightPanel,
    ThreadPanel,
    View,
    PullRequest,
    Diff,
    Preview,
    CommandPalette,
    FilePicker,
    ProjectSearch,
    Usage,
    Theme,
    Appearance,
    ThemeEditor,
    Composer,
    Queue,
    Chat,
    Editor,
    ModelPicker,
    Thread,
    Script,
}

/// Runs the project script `script.<id>.run`.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = t3, no_json)]
pub struct RunProjectScript {
    pub id: ScriptId,
}

macro_rules! command_registry {
    ($($variant:ident => $action:ident, $category:ident, $title:expr;)*) => {
        gpui_kit::actions!(t3, [$($action,)*]);

        /// The action a built-in command dispatches.
        pub fn static_action(command: StaticCommand) -> Box<dyn Action> {
            match command {
                $(StaticCommand::$variant => Box::new($action),)*
            }
        }

        /// The command an action stands for, if it is one of ours.
        pub fn static_command_for_action(action: &dyn Action) -> Option<StaticCommand> {
            let id = action.as_any().type_id();
            $(if id == TypeId::of::<$action>() {
                return Some(StaticCommand::$variant);
            })*
            None
        }

        fn static_title(command: StaticCommand) -> SharedString {
            match command {
                $(StaticCommand::$variant => $title,)*
            }
        }

        fn static_category(command: StaticCommand) -> CommandCategory {
            match command {
                $(StaticCommand::$variant => CommandCategory::$category,)*
            }
        }

        /// Adds `listener` to `element` for the action of every built-in
        /// command `handles` accepts, which makes those commands available
        /// on paths through `element`. It runs only for actions no deeper
        /// element handled.
        pub fn on_commands<E: InteractiveElement>(
            mut element: E,
            handles: impl Fn(StaticCommand) -> bool,
            listener: Rc<dyn Fn(StaticCommand, &mut Window, &mut App)>,
        ) -> E {
            $(if handles(StaticCommand::$variant) {
                let listener = listener.clone();
                element = element.on_action(move |_: &$action, window, cx| listener(StaticCommand::$variant, window, cx));
            })*
            element
        }
    };
}

command_registry! {
    WorkspaceFocusNext => FocusNextWorkspaceArea, Workspace, t!("Focus Next Workspace Area");
    WorkspaceFocusPrevious => FocusPreviousWorkspaceArea, Workspace, t!("Focus Previous Workspace Area");
    SidebarToggle => ToggleSidebar, Sidebar, t!("Sidebar: Toggle");
    NavigationBack => NavigateBack, Navigation, t!("Navigation: Back");
    NavigationForward => NavigateForward, Navigation, t!("Navigation: Forward");
    TerminalToggle => ToggleTerminal, Terminal, t!("Terminal: Toggle");
    TerminalSplit => SplitTerminal, Terminal, t!("Terminal: Split");
    TerminalSplitVertical => SplitTerminalVertical, Terminal, t!("Terminal: Split Vertical");
    TerminalNew => NewTerminal, Terminal, t!("Terminal: New");
    TerminalClose => CloseTerminal, Terminal, t!("Terminal: Close");
    RightPanelToggle => ToggleRightPanel, RightPanel, t!("Right Panel: Toggle");
    RightPanelNew => NewRightPanelTab, RightPanel, t!("Right Panel: New");
    ThreadPanelToggle => ToggleThreadPanel, ThreadPanel, t!("Thread Panel: Toggle");
    RightPanelToggleMaximized => ToggleRightPanelMaximized, RightPanel, t!("Right Panel: Toggle Maximized");
    RightPanelClose => CloseRightPanelTab, RightPanel, t!("Right Panel: Close");
    ViewReopenClosed => ReopenClosedView, View, t!("Reopen Closed Tab");
    PullRequestCopyNumber => CopyPullRequestNumber, PullRequest, t!("Pull Request: Copy Number");
    DiffToggle => ToggleDiff, Diff, t!("Diff: Toggle");
    PreviewToggle => TogglePreview, Preview, t!("Preview: Toggle");
    PreviewRefresh => RefreshPreview, Preview, t!("Preview: Refresh");
    PreviewFocusUrl => FocusPreviewUrl, Preview, t!("Preview: Focus URL");
    PreviewZoomIn => ZoomInPreview, Preview, t!("Preview: Zoom In");
    PreviewZoomOut => ZoomOutPreview, Preview, t!("Preview: Zoom Out");
    PreviewResetZoom => ResetPreviewZoom, Preview, t!("Preview: Reset Zoom");
    PreviewFind => FindInPreview, Preview, t!("Preview: Find on Page");
    CommandPaletteToggle => ToggleCommandPalette, CommandPalette, t!("Command Palette: Toggle");
    FilePickerToggle => ToggleFilePicker, FilePicker, t!("File Picker: Toggle");
    ProjectSearchToggle => ToggleProjectSearch, ProjectSearch, t!("Project Search: Toggle");
    UsageOpen => OpenUsage, Usage, t!("Usage: Open");
    ThemeSelect => SelectTheme, Theme, t!("Theme: Select");
    AppearanceCycle => CycleAppearance, Appearance, t!("Appearance: Cycle");
    ThemeEditorToggle => ToggleThemeEditor, ThemeEditor, t!("Theme Editor: Toggle");
    ComposerStash => StashComposer, Composer, t!("Composer: Stash");
    ComposerSendAlternate => SendAlternate, Composer, t!("Composer: Opposite Queue or Steer Action");
    ComposerSendBackground => SendInBackground, Composer, t!("Composer: Start in Background");
    ComposerSendAndNewThread => SendAndNewThread, Composer, t!("Composer: Send and Start New Thread");
    ComposerDictation => ToggleDictation, Composer, t!("Composer: Start or Finish Dictation");
    ComposerHost => OpenComposerHost, Composer, t!("Composer: Host");
    ComposerCycleHost => CycleComposerHost, Composer, t!("Composer: Cycle Host");
    ComposerEffort => OpenComposerEffort, Composer, t!("Composer: Effort");
    ComposerIncreaseEffort => IncreaseEffort, Composer, t!("Composer: Increase Effort");
    ComposerDecreaseEffort => DecreaseEffort, Composer, t!("Composer: Decrease Effort");
    ComposerMode => OpenComposerMode, Composer, t!("Composer: Mode");
    ComposerWorkspace => OpenComposerWorkspace, Composer, t!("Composer: Workspace");
    ComposerPreviousWorktree => UsePreviousWorktree, Composer, t!("Composer: Previous Worktree");
    ComposerBranch => OpenComposerBranch, Composer, t!("Composer: Branch");
    ChatNew => NewChat, Chat, t!("Chat: New");
    ChatNewLocal => NewLocalChat, Chat, t!("Chat: New Local");
    ChatNewWithoutProject => NewChatWithoutProject, Chat, t!("Chat: New Without Project");
    ChatFind => FindInThread, Chat, t!("Chat: Find");
    EditorOpenFavorite => OpenFavoriteEditor, Editor, t!("Editor: Open Favorite");
    UsageCost => ShowUsageCost, Usage, t!("Usage: Cost");
    UsageTokens => ShowUsageTokens, Usage, t!("Usage: Tokens");
    UsageLimits => ShowUsageLimits, Usage, t!("Usage: Limits");
    UsagePeriodDay => UsagePeriodDay, Usage, t!("Usage: Period: Past 24h");
    UsagePeriodWeek => UsagePeriodWeek, Usage, t!("Usage: Period: 7 days");
    UsagePeriodMonth => UsagePeriodMonth, Usage, t!("Usage: Period: 30 days");
    UsagePeriodQuarter => UsagePeriodQuarter, Usage, t!("Usage: Period: 90 days");
    ModelPickerToggle => ToggleModelPicker, ModelPicker, t!("Model Picker: Toggle");
    ModelPickerPreviousProvider => PreviousModelProvider, ModelPicker, t!("Model Picker: Previous Provider");
    ModelPickerNextProvider => NextModelProvider, ModelPicker, t!("Model Picker: Next Provider");
    ModelPickerJump1 => JumpToModel1, ModelPicker, t!("Model Picker: Jump: {n}", n = 1);
    ModelPickerJump2 => JumpToModel2, ModelPicker, t!("Model Picker: Jump: {n}", n = 2);
    ModelPickerJump3 => JumpToModel3, ModelPicker, t!("Model Picker: Jump: {n}", n = 3);
    ModelPickerJump4 => JumpToModel4, ModelPicker, t!("Model Picker: Jump: {n}", n = 4);
    ModelPickerJump5 => JumpToModel5, ModelPicker, t!("Model Picker: Jump: {n}", n = 5);
    ModelPickerJump6 => JumpToModel6, ModelPicker, t!("Model Picker: Jump: {n}", n = 6);
    ModelPickerJump7 => JumpToModel7, ModelPicker, t!("Model Picker: Jump: {n}", n = 7);
    ModelPickerJump8 => JumpToModel8, ModelPicker, t!("Model Picker: Jump: {n}", n = 8);
    ModelPickerJump9 => JumpToModel9, ModelPicker, t!("Model Picker: Jump: {n}", n = 9);
    ThreadStop => StopThread, Thread, t!("Thread: Stop");
    ThreadSteerQueuedMessage => SteerQueuedMessage, Queue, t!("Queue: Send First Queued Message");
    ThreadEditQueuedMessage => EditQueuedMessage, Queue, t!("Queue: Edit Last Queued Message");
    ThreadPrevious => PreviousThread, Thread, t!("Thread: Previous");
    ThreadNext => NextThread, Thread, t!("Thread: Next");
    ThreadCopyReference => CopyThreadReference, PullRequest, t!("Pull Request: Copy Link or Thread");
    ThreadSettle => SettleThread, Thread, t!("Thread: Settle");
    ThreadPin => PinThread, Thread, t!("Thread: Pin");
    ThreadRegenerateTitle => RegenerateThreadTitle, Thread, t!("Thread: Regenerate Title");
    ThreadUndo => UndoThreadAction, Thread, t!("Thread: Undo");
    ThreadJump1 => JumpToThread1, Thread, t!("Thread: Jump: {n}", n = 1);
    ThreadJump2 => JumpToThread2, Thread, t!("Thread: Jump: {n}", n = 2);
    ThreadJump3 => JumpToThread3, Thread, t!("Thread: Jump: {n}", n = 3);
    ThreadJump4 => JumpToThread4, Thread, t!("Thread: Jump: {n}", n = 4);
    ThreadJump5 => JumpToThread5, Thread, t!("Thread: Jump: {n}", n = 5);
    ThreadJump6 => JumpToThread6, Thread, t!("Thread: Jump: {n}", n = 6);
    ThreadJump7 => JumpToThread7, Thread, t!("Thread: Jump: {n}", n = 7);
    ThreadJump8 => JumpToThread8, Thread, t!("Thread: Jump: {n}", n = 8);
    ThreadJump9 => JumpToThread9, Thread, t!("Thread: Jump: {n}", n = 9);
}

/// The package that builds a command whose app-level handler only says it is not available yet,
/// for the commands that reach such a notice. The wiring check reads it.
pub fn pending_command_owner(command: StaticCommand) -> Option<&'static str> {
    match command {
        // The key does what the fork does with no transcription destination; dictation itself is not built.
        StaticCommand::ComposerDictation => Some("voice"),
        _ => None,
    }
}

/// The action a command dispatches.
pub fn command_action(command: &KeybindingCommand) -> Box<dyn Action> {
    match command {
        KeybindingCommand::Static(command) => static_action(*command),
        KeybindingCommand::Script(id) => Box::new(RunProjectScript { id: id.clone() }),
    }
}

/// The command an action stands for, if it is one of ours.
pub fn command_for_action(action: &dyn Action) -> Option<KeybindingCommand> {
    if let Some(script) = action.as_any().downcast_ref::<RunProjectScript>() {
        return Some(KeybindingCommand::Script(script.id.clone()));
    }
    static_command_for_action(action).map(KeybindingCommand::Static)
}

/// The command's title, as in Settings → Keybindings (`commandLabel`).
pub fn command_title(command: &KeybindingCommand) -> SharedString {
    match command {
        KeybindingCommand::Static(command) => static_title(*command),
        KeybindingCommand::Script(id) => t!("Run Script: {name}", name = title_case_segment(id.as_str())),
    }
}

/// The command's category.
pub fn command_category(command: &KeybindingCommand) -> CommandCategory {
    match command {
        KeybindingCommand::Static(command) => static_category(*command),
        KeybindingCommand::Script(_) => CommandCategory::Script,
    }
}

/// The rules the default keymap gives a command.
pub fn default_rules(command: &KeybindingCommand) -> impl Iterator<Item = &'static ResolvedKeybindingRule> {
    default_resolved_keybindings().rules().iter().filter(move |rule| &rule.command == command)
}

/// "run-tests" → "Run Tests" (`titleCaseCommandSegment`).
fn title_case_segment(segment: &str) -> String {
    let mut spaced = String::with_capacity(segment.len() + 4);
    let mut previous: Option<char> = None;
    for c in segment.chars() {
        if c.is_ascii_uppercase() && previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit()) {
            spaced.push(' ');
        }
        spaced.push(c);
        previous = Some(c);
    }
    spaced
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether some element on `focus`'s path handles the command's action: the
/// palette's availability check, run against the focus it opened from.
pub fn is_command_available(command: &KeybindingCommand, focus: &FocusHandle, window: &Window) -> bool {
    window.is_action_available_in(command_action(command).as_ref(), focus)
}
