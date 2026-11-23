//! Shortcut resolution and labels. Ports `apps/web/src/keybindings.ts`: the
//! `when` context, which shortcut a command effectively has, how it is
//! labelled, and the reference resolver (`resolveShortcutCommand`).
//!
//! Live keystrokes do not go through [`resolve_shortcut_command`]: the rules
//! are compiled into GPUI key bindings by [`crate::shortcuts::keymap`], which
//! encodes the same last-match-wins order. The resolver here gives labels,
//! the palette's own key handling, and a reference to check the keymap against.

use std::fmt::Write as _;

use gpui_kit::SharedString;
use t3code_shared::keybindings::{
    KeybindingCommand, KeybindingShortcut, ModifierKeys, ResolvedKeybindingRule, ResolvedKeybindingsConfig,
    ShortcutChord, ShortcutPlatform, StaticCommand, THREAD_JUMP_COMMANDS, WhenNode, matches_shortcut_modifiers,
};

/// A `when` context key. Unknown names in a rule evaluate to false.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WhenKey {
    TerminalFocus,
    TerminalOpen,
    PreviewFocus,
    PreviewOpen,
    RightPanelOpen,
    ModelPickerOpen,
    UsagePageOpen,
    ComposerFocus,
    DraftThreadRoute,
    TurnRunning,
    EditableFocus,
    IsWeb,
    IsDesktop,
    /// The send shortcut requires a modifier, so `mod+enter` rules move to `mod+shift+enter`.
    ComposerRequiresModifier,
}

/// Where a context key's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhenKeySource {
    /// True while focus is inside an element whose GPUI key context has one of these identifiers.
    Focus(&'static [&'static str]),
    /// App state the views report through [`crate::shortcuts::keymap::ShortcutState`].
    State,
}

impl WhenKey {
    pub const ALL: [WhenKey; 14] = [
        Self::TerminalFocus,
        Self::TerminalOpen,
        Self::PreviewFocus,
        Self::PreviewOpen,
        Self::RightPanelOpen,
        Self::ModelPickerOpen,
        Self::UsagePageOpen,
        Self::ComposerFocus,
        Self::DraftThreadRoute,
        Self::TurnRunning,
        Self::EditableFocus,
        Self::IsWeb,
        Self::IsDesktop,
        Self::ComposerRequiresModifier,
    ];

    /// The name rules use.
    pub const fn name(self) -> &'static str {
        match self {
            Self::TerminalFocus => "terminalFocus",
            Self::TerminalOpen => "terminalOpen",
            Self::PreviewFocus => "previewFocus",
            Self::PreviewOpen => "previewOpen",
            Self::RightPanelOpen => "rightPanelOpen",
            Self::ModelPickerOpen => "modelPickerOpen",
            Self::UsagePageOpen => "usagePageOpen",
            Self::ComposerFocus => "composerFocus",
            Self::DraftThreadRoute => "draftThreadRoute",
            Self::TurnRunning => "turnRunning",
            Self::EditableFocus => "editableFocus",
            Self::IsWeb => "isWeb",
            Self::IsDesktop => "isDesktop",
            Self::ComposerRequiresModifier => "composerRequiresModifier",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.name() == name)
    }

    /// Focus keys follow GPUI key contexts: the terminal surface sets
    /// `Terminal`, gpui-component inputs set `Input`, and the composer and the
    /// preview must set `Composer` and `Preview`. Everything else is state.
    pub const fn source(self) -> WhenKeySource {
        match self {
            Self::TerminalFocus => WhenKeySource::Focus(&[crate::shortcuts::keymap::TERMINAL_CONTEXT]),
            Self::PreviewFocus => WhenKeySource::Focus(&[crate::shortcuts::keymap::PREVIEW_CONTEXT]),
            Self::ComposerFocus => WhenKeySource::Focus(&[crate::shortcuts::keymap::COMPOSER_CONTEXT]),
            // A text field, the composer, or the terminal's input (xterm types into a textarea).
            Self::EditableFocus => WhenKeySource::Focus(&[
                crate::shortcuts::keymap::INPUT_CONTEXT,
                crate::shortcuts::keymap::COMPOSER_CONTEXT,
                crate::shortcuts::keymap::TERMINAL_CONTEXT,
            ]),
            _ => WhenKeySource::State,
        }
    }

    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

/// Values of the `when` keys at one moment (`ShortcutMatchContext`). The
/// default is the desktop app with nothing focused or open.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortcutContext(u16);

impl Default for ShortcutContext {
    fn default() -> Self {
        Self(WhenKey::IsDesktop.bit())
    }
}

impl std::fmt::Debug for ShortcutContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(WhenKey::ALL.into_iter().filter(|k| self.get(*k)).map(WhenKey::name)).finish()
    }
}

impl ShortcutContext {
    pub fn get(self, key: WhenKey) -> bool {
        self.0 & key.bit() != 0
    }

    pub fn set(&mut self, key: WhenKey, value: bool) {
        if value {
            self.0 |= key.bit();
        } else {
            self.0 &= !key.bit();
        }
    }

    pub fn with(mut self, key: WhenKey, value: bool) -> Self {
        self.set(key, value);
        self
    }

    /// The true keys, in [`WhenKey::ALL`] order.
    pub fn keys(self) -> impl Iterator<Item = WhenKey> {
        WhenKey::ALL.into_iter().filter(move |key| self.get(*key))
    }
}

/// Evaluates a `when` tree (`evaluateWhenNode`). `true` and `false` are literals.
pub fn evaluate_when(node: &WhenNode, context: ShortcutContext) -> bool {
    match node {
        WhenNode::Identifier { name } => match &**name {
            "true" => true,
            "false" => false,
            name => WhenKey::from_name(name).is_some_and(|key| context.get(key)),
        },
        WhenNode::Not { node } => !evaluate_when(node, context),
        WhenNode::And { left, right } => evaluate_when(left, context) && evaluate_when(right, context),
        WhenNode::Or { left, right } => evaluate_when(left, context) || evaluate_when(right, context),
    }
}

fn matches_when(rule: &ResolvedKeybindingRule, context: ShortcutContext) -> bool {
    rule.when_ast.as_ref().is_none_or(|ast| evaluate_when(ast, context))
}

/// Whether a send rule moves from `mod+enter` to `mod+shift+enter` when the
/// send shortcut requires a modifier (`shortcutForContext`).
pub fn is_send_shortcut_shifted_by_preference(rule: &ResolvedKeybindingRule) -> bool {
    let shortcut = &rule.shortcut;
    (rule.command == StaticCommand::ComposerSendAlternate || rule.command == StaticCommand::ComposerSendBackground)
        && shortcut.key == "enter"
        && shortcut.mod_key
        && !shortcut.meta_key
        && !shortcut.ctrl_key
        && !shortcut.alt_key
        && !shortcut.shift_key
}

/// The shortcut a rule means in `context` (`shortcutForContext`).
pub fn shortcut_for_context(rule: &ResolvedKeybindingRule, context: ShortcutContext) -> KeybindingShortcut {
    if context.get(WhenKey::ComposerRequiresModifier) && is_send_shortcut_shifted_by_preference(rule) {
        return KeybindingShortcut { shift_key: true, ..rule.shortcut.clone() };
    }
    rule.shortcut.clone()
}

/// The shortcuts that run `command` in `context`, newest first: a rule
/// counts only if no later rule claims the same chord (`effectiveShortcutsForCommand`).
pub fn effective_shortcuts_for_command(
    keybindings: &ResolvedKeybindingsConfig,
    command: &KeybindingCommand,
    context: ShortcutContext,
    platform: ShortcutPlatform,
) -> Vec<KeybindingShortcut> {
    let mut claimed: Vec<ShortcutChord> = Vec::new();
    let mut effective = Vec::new();
    for rule in keybindings.rules().iter().rev() {
        if !matches_when(rule, context) {
            continue;
        }
        let shortcut = shortcut_for_context(rule, context);
        let chord = shortcut.chord(platform);
        if claimed.contains(&chord) {
            continue;
        }
        claimed.push(chord);
        if &rule.command == command {
            effective.push(shortcut);
        }
    }
    effective
}

/// A keystroke as the resolver sees it: a `KeyboardEvent.key`-style name
/// (lowercase, "arrowup", " ") and the held modifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPress {
    pub key: String,
    pub modifiers: ModifierKeys,
    /// Auto-repeat while held.
    pub repeat: bool,
}

/// The command a keystroke runs in `context`: the last rule whose `when`
/// and chord both match (`resolveShortcutCommand`).
pub fn resolve_shortcut_command<'a>(
    press: &KeyPress,
    keybindings: &'a ResolvedKeybindingsConfig,
    context: ShortcutContext,
    platform: ShortcutPlatform,
) -> Option<&'a KeybindingCommand> {
    keybindings.rules().iter().rev().find_map(|rule| {
        if !matches_when(rule, context) {
            return None;
        }
        let shortcut = shortcut_for_context(rule, context);
        (shortcut.key == press.key && matches_shortcut_modifiers(press.modifiers, &shortcut, platform)).then_some(&rule.command)
    })
}

/// A key's label: "Space", "Esc", "Up", "K", "F6" (`formatShortcutKeyLabel`).
pub fn format_shortcut_key_label(key: &str) -> String {
    match key {
        " " => "Space".into(),
        "escape" => "Esc".into(),
        "arrowup" => "Up".into(),
        "arrowdown" => "Down".into(),
        "arrowleft" => "Left".into(),
        "arrowright" => "Right".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        key if key.chars().count() == 1 => key.to_uppercase(),
        key => {
            let mut chars = key.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    }
}

/// A shortcut's label: "⌃⌥⇧⌘K" on macOS, "Ctrl+Alt+Shift+K" elsewhere (`formatShortcutLabel`).
pub fn format_shortcut_label(shortcut: &KeybindingShortcut, platform: ShortcutPlatform) -> String {
    let key = format_shortcut_key_label(&shortcut.key);
    let chord = shortcut.chord(platform);
    let mut label = String::new();
    if platform == ShortcutPlatform::Mac {
        for (on, glyph) in [(chord.ctrl, '⌃'), (chord.alt, '⌥'), (chord.shift, '⇧'), (chord.meta, '⌘')] {
            if on {
                label.push(glyph);
            }
        }
        label.push_str(&key);
        return label;
    }
    for (on, name) in [(chord.ctrl, "Ctrl"), (chord.alt, "Alt"), (chord.shift, "Shift"), (chord.meta, "Meta")] {
        if on {
            let _ = write!(label, "{name}+");
        }
    }
    label.push_str(&key);
    label
}

/// The label of the shortcut that runs `command`, if any (`shortcutLabelForCommand`).
pub fn shortcut_label_for_command(
    keybindings: &ResolvedKeybindingsConfig,
    command: &KeybindingCommand,
    context: ShortcutContext,
    platform: ShortcutPlatform,
) -> Option<SharedString> {
    effective_shortcuts_for_command(keybindings, command, context, platform)
        .first()
        .map(|shortcut| format_shortcut_label(shortcut, platform).into())
}

/// `thread.jump.<index+1>`.
pub fn thread_jump_command_for_index(index: usize) -> Option<StaticCommand> {
    THREAD_JUMP_COMMANDS.get(index).copied()
}

/// `previous` / `next` for the thread traversal commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraversalDirection {
    Previous,
    Next,
}

pub fn thread_traversal_direction(command: &KeybindingCommand) -> Option<TraversalDirection> {
    match command.as_static()? {
        StaticCommand::ThreadPrevious => Some(TraversalDirection::Previous),
        StaticCommand::ThreadNext => Some(TraversalDirection::Next),
        _ => None,
    }
}

/// Whether held modifiers are those of a thread jump shortcut, so the
/// sidebar shows jump hints (`shouldShowThreadJumpHintsForModifiers`). Never
/// over a focused terminal, where Ctrl is held for the shell.
pub fn should_show_thread_jump_hints(
    modifiers: ModifierKeys,
    keybindings: &ResolvedKeybindingsConfig,
    context: ShortcutContext,
    platform: ShortcutPlatform,
) -> bool {
    if context.get(WhenKey::TerminalFocus) {
        return false;
    }
    THREAD_JUMP_COMMANDS.iter().any(|command| {
        effective_shortcuts_for_command(keybindings, &KeybindingCommand::Static(*command), context, platform)
            .first()
            .is_some_and(|shortcut| matches_shortcut_modifiers(modifiers, shortcut, platform))
    })
}
