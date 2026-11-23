//! Compiles keybinding rules into GPUI key bindings, so a keystroke resolves
//! in GPUI's own keymap lookup with no work of ours per keystroke.
//!
//! GPUI ranks matching bindings by the depth of the key context they match,
//! then by insertion order. The web app instead takes the last rule whose
//! chord and `when` match. Two choices make GPUI produce that answer:
//!
//! - Every predicate is built only from `!` over identifiers, which GPUI
//!   evaluates against the whole focus path at any depth. So every rule
//!   matches at the focused element's depth, and among rules ties go to the
//!   later one. Added after the widget bindings, a rule also beats a focused
//!   input's own binding for the same chord, as the web's window listeners do.
//! - Last-match-wins is compiled in: a rule's predicate excludes the `when`
//!   of every later rule on the same chord. At most one rule matches a
//!   keystroke, so an unhandled winner falls through to the focused widget,
//!   never to a rule it shadows.
//!
//! `when` keys become identifiers. Focus keys are GPUI key contexts the
//! focused views set ([`TERMINAL_CONTEXT`], [`INPUT_CONTEXT`],
//! [`COMPOSER_CONTEXT`], [`PREVIEW_CONTEXT`]); state keys are identifiers the
//! shortcut root adds to its own context from [`ShortcutState`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Global, KeyBinding, KeyBindingContextPredicate as Pred, KeyContext, SharedString};
use t3code_shared::keybindings::{
    KeybindingCommand, ResolvedKeybindingRule, ResolvedKeybindingsConfig, ServerConfigIssue, ShortcutChord,
    ShortcutPlatform, StaticCommand, US_SHIFTED_TO_UNSHIFTED_KEYS, WhenNode, default_resolved_keybindings,
};

use super::commands::command_action;
use crate::keybindings::{
    ShortcutContext, WhenKey, WhenKeySource, is_send_shortcut_shifted_by_preference, shortcut_label_for_command,
};

/// Key context of the terminal surface.
pub const TERMINAL_CONTEXT: &str = "Terminal";
/// Key context of gpui-component text inputs and editors.
pub const INPUT_CONTEXT: &str = "Input";
/// Key context the composer sets on its editor's container.
pub const COMPOSER_CONTEXT: &str = "Composer";
/// Key context the browser preview sets on its surface.
pub const PREVIEW_CONTEXT: &str = "Preview";
/// Key context of the shortcut root, which also carries the state keys.
pub const ROOT_CONTEXT: &str = "T3Shortcuts";
/// An identifier no context has. It marks our bindings and builds constants.
const NEVER: &str = "t3.never";

fn never() -> Pred {
    Pred::Identifier(NEVER.into())
}

fn always() -> Pred {
    Pred::Not(Box::new(never()))
}

fn not(pred: Pred) -> Pred {
    Pred::Not(Box::new(pred))
}

fn and(left: Pred, right: Pred) -> Pred {
    Pred::And(Box::new(left), Box::new(right))
}

fn or(left: Pred, right: Pred) -> Pred {
    Pred::Or(Box::new(left), Box::new(right))
}

/// True when some context on the focus path has `identifier`, at any depth.
fn anywhere(identifier: &'static str) -> Pred {
    not(not(Pred::Identifier(identifier.into())))
}

fn when_key(key: WhenKey) -> Pred {
    match key.source() {
        WhenKeySource::Focus(contexts) => {
            contexts.iter().map(|context| anywhere(context)).reduce(or).unwrap_or_else(never)
        }
        WhenKeySource::State => anywhere(key.name()),
    }
}

/// A `when` tree as a depth-independent GPUI predicate.
fn translate(node: &WhenNode) -> Pred {
    match node {
        WhenNode::Identifier { name } => match &**name {
            "true" => always(),
            "false" => never(),
            name => WhenKey::from_name(name).map_or_else(never, when_key),
        },
        WhenNode::Not { node } => not(translate(node)),
        WhenNode::And { left, right } => and(translate(left), translate(right)),
        WhenNode::Or { left, right } => or(translate(left), translate(right)),
    }
}

fn rule_when(rule: &ResolvedKeybindingRule) -> Pred {
    rule.when_ast.as_ref().map_or_else(always, translate)
}

/// One chord a rule can fire on, and the extra condition for that chord.
struct Variant {
    chord: ShortcutChord,
    condition: Option<Pred>,
}

fn variants(rule: &ResolvedKeybindingRule, platform: ShortcutPlatform) -> Vec<Variant> {
    let chord = rule.shortcut.chord(platform);
    if !is_send_shortcut_shifted_by_preference(rule) {
        return vec![Variant { chord, condition: None }];
    }
    // `mod+enter` sends become `mod+shift+enter` while the send shortcut needs a modifier.
    let requires = when_key(WhenKey::ComposerRequiresModifier);
    let shifted = ShortcutChord { shift: true, ..chord.clone() };
    vec![
        Variant { chord, condition: Some(not(requires.clone())) },
        Variant { chord: shifted, condition: Some(requires) },
    ]
}

/// The GPUI key name for a `KeyboardEvent.key`-style name.
fn gpui_key(key: &str) -> Cow<'_, str> {
    match key {
        "arrowup" => "up".into(),
        "arrowdown" => "down".into(),
        "arrowleft" => "left".into(),
        "arrowright" => "right".into(),
        " " => "space".into(),
        "esc" => "escape".into(),
        key => key.into(),
    }
}

/// GPUI keystroke strings for a chord. A shifted US symbol also gets its
/// shifted character, which is how some platforms report it ("ctrl-{").
fn keystroke_sources(chord: &ShortcutChord) -> Vec<String> {
    let prefix = |shift: bool| {
        let mut out = String::new();
        if chord.ctrl {
            out.push_str("ctrl-");
        }
        if chord.alt {
            out.push_str("alt-");
        }
        if shift {
            out.push_str("shift-");
        }
        if chord.meta {
            out.push_str("cmd-");
        }
        out
    };
    let mut sources = vec![format!("{}{}", prefix(chord.shift), gpui_key(&chord.key))];
    if chord.shift
        && let Some((shifted, _)) =
            US_SHIFTED_TO_UNSHIFTED_KEYS.iter().find(|(_, unshifted)| chord.key.chars().eq(std::iter::once(*unshifted)))
    {
        sources.push(format!("{}{shifted}", prefix(false)));
    }
    sources
}

/// The bindings for a config, in rule order.
pub fn compile_bindings(config: &ResolvedKeybindingsConfig, platform: ShortcutPlatform, cx: &App) -> Vec<KeyBinding> {
    let rules = config.rules();
    let compiled: Vec<(Pred, Vec<Variant>)> = rules.iter().map(|rule| (rule_when(rule), variants(rule, platform))).collect();
    let mut bindings = Vec::with_capacity(rules.len() + 16);
    for (index, rule) in rules.iter().enumerate() {
        let (when, own_variants) = &compiled[index];
        for variant in own_variants {
            let mut predicate = when.clone();
            if let Some(condition) = &variant.condition {
                predicate = and(predicate, condition.clone());
            }
            // Later rules on the same chord win wherever their condition holds.
            let shadowed_by = compiled[index + 1..]
                .iter()
                .flat_map(|(later_when, later_variants)| {
                    later_variants.iter().filter(|later| later.chord == variant.chord).map(move |later| match &later.condition {
                        Some(condition) => and(later_when.clone(), condition.clone()),
                        None => later_when.clone(),
                    })
                })
                .reduce(or);
            if let Some(shadow) = shadowed_by {
                predicate = and(predicate, not(shadow));
            }
            // A focused terminal keeps its Ctrl chords, such as tmux's Ctrl+B (`isSidebarToggleShortcut`).
            if rule.command == StaticCommand::SidebarToggle && variant.chord.ctrl {
                predicate = and(predicate, not(when_key(WhenKey::TerminalFocus)));
            }
            let predicate = Rc::new(and(always(), predicate));
            for source in keystroke_sources(&variant.chord) {
                match KeyBinding::load(&source, command_action(&rule.command), Some(predicate.clone()), false, None, cx) {
                    Ok(binding) => bindings.push(binding),
                    Err(error) => eprintln!("skipping keybinding {source}: {error}"),
                }
            }
        }
    }
    bindings
}

fn mentions_never(pred: &Pred) -> bool {
    match pred {
        Pred::Identifier(name) => name.as_ref() == NEVER,
        Pred::Equal(..) | Pred::NotEqual(..) => false,
        Pred::Not(inner) => mentions_never(inner),
        Pred::Descendant(left, right) | Pred::And(left, right) | Pred::Or(left, right) => {
            mentions_never(left) || mentions_never(right)
        }
    }
}

fn is_ours(binding: &KeyBinding) -> bool {
    binding.predicate().is_some_and(|pred| mentions_never(&pred))
}

/// The keymap in use and the app state the `when` keys read.
pub struct ShortcutState {
    pub platform: ShortcutPlatform,
    pub config: Arc<ResolvedKeybindingsConfig>,
    pub issues: Arc<[ServerConfigIssue]>,
    /// Values of the state keys; focus keys here are ignored.
    pub state: ShortcutContext,
    /// Bumped on every install, so views can tell the keymap changed.
    pub generation: u64,
    labels: HashMap<KeybindingCommand, SharedString>,
}

impl Global for ShortcutState {}

/// The context menus, the palette and hints label shortcuts in: the desktop
/// app with a text field focused and no model picker (`handleKeyDown`).
pub fn label_context() -> ShortcutContext {
    ShortcutContext::default().with(WhenKey::EditableFocus, true)
}

impl ShortcutState {
    /// The label of the shortcut that runs `command` (cached per install).
    pub fn label(&self, command: &KeybindingCommand) -> Option<SharedString> {
        self.labels.get(command).cloned()
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Sets a state key, such as `turnRunning`, and redraws so the root's key
    /// context follows. Focus keys cannot be set; they follow focus.
    pub fn set(key: WhenKey, value: bool, cx: &mut App) {
        debug_assert_eq!(key.source(), WhenKeySource::State, "{} follows focus", key.name());
        let state = cx.global_mut::<Self>();
        if state.state.get(key) == value {
            return;
        }
        state.state.set(key, value);
        cx.refresh_windows();
    }

    /// The root's key context: [`ROOT_CONTEXT`] and the true state keys.
    pub fn root_key_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add(ROOT_CONTEXT);
        for key in self.state.keys() {
            if key.source() == WhenKeySource::State {
                context.add(key.name());
            }
        }
        context
    }
}

/// The `when` context right now: the state keys, and the focus keys read from
/// the key contexts on the focused element's path.
pub fn current_context(window: &gpui_kit::Window, cx: &App) -> ShortcutContext {
    let mut context = ShortcutState::get(cx).state;
    let stack = window.context_stack();
    for key in WhenKey::ALL {
        if let WhenKeySource::Focus(names) = key.source() {
            context.set(key, stack.iter().any(|entry| names.iter().any(|name| entry.contains(name))));
        }
    }
    context
}

/// Installs `config` as the live keymap, replacing the previous one. Other
/// bindings (widgets, the palette's own keys) stay, before ours.
pub fn install(config: ResolvedKeybindingsConfig, issues: Vec<ServerConfigIssue>, cx: &mut App) {
    let platform = cx.try_global::<ShortcutState>().map_or(ShortcutPlatform::current(), |s| s.platform);
    let ours = compile_bindings(&config, platform, cx);
    let keymap = cx.key_bindings();
    {
        let mut keymap = keymap.borrow_mut();
        let others: Vec<KeyBinding> = keymap.bindings().filter(|binding| !is_ours(binding)).cloned().collect();
        keymap.clear();
        keymap.add_bindings(others);
        keymap.add_bindings(ours);
    }
    let label_context = label_context();
    let commands = StaticCommand::ALL
        .iter()
        .map(|command| KeybindingCommand::Static(*command))
        .chain(config.rules().iter().map(|rule| rule.command.clone()));
    let labels = commands
        .filter_map(|command| {
            let label = shortcut_label_for_command(&config, &command, label_context, platform)?;
            Some((command, label))
        })
        .collect();
    let (state, generation) = cx
        .try_global::<ShortcutState>()
        .map_or((ShortcutContext::default(), 0), |previous| (previous.state, previous.generation + 1));
    cx.set_global(ShortcutState {
        platform,
        config: Arc::new(config),
        issues: issues.into(),
        state,
        generation,
        labels,
    });
    cx.refresh_windows();
}

/// Installs the default keymap. Call once at startup, after every widget's
/// `init` binds its keys.
pub fn init(cx: &mut App) {
    install(default_resolved_keybindings().clone(), Vec::new(), cx);
}

/// Labels shortcuts for another platform (scenes and checks). Reinstalls the keymap.
pub fn set_platform(platform: ShortcutPlatform, cx: &mut App) {
    let state = cx.global_mut::<ShortcutState>();
    state.platform = platform;
    let (config, issues) = ((*state.config).clone(), state.issues.to_vec());
    install(config, issues, cx);
}
