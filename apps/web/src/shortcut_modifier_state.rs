//! Which modifier keys are held, for jump-hint badges. Ports
//! `shortcutModifierState.ts`. The shortcut root feeds it modifier changes,
//! key presses and window blur; the sidebar observes [`ShortcutModifierStore`]
//! and draws thread jump hints while [`ShortcutModifierStore::show_thread_jump_hints`].

use gpui_kit::{App, AppContext as _, Context, Entity, Global, Modifiers};
use t3code_shared::keybindings::ModifierKeys;

use crate::keybindings::{ShortcutContext, WhenKey, should_show_thread_jump_hints};
use crate::shortcuts::keymap::ShortcutState;

/// A keyboard event, as the store reads it.
#[derive(Clone, Copy, Debug)]
pub enum ModifierEvent {
    /// The platform reported the full set of held modifiers.
    ModifiersChanged(Modifiers),
    /// A non-modifier key went down with these modifiers.
    KeyDown(Modifiers),
}

fn keys(modifiers: Modifiers) -> ModifierKeys {
    ModifierKeys { meta: modifiers.platform, ctrl: modifiers.control, shift: modifiers.shift, alt: modifiers.alt }
}

/// The held state after an event (`shortcutModifierStateAfterKeyboardEvent`).
/// A non-modifier key can only release modifiers: dictation tools paste with
/// a synthetic Meta that never gets its key-up.
pub fn state_after_event(current: ModifierKeys, event: ModifierEvent) -> ModifierKeys {
    match event {
        ModifierEvent::ModifiersChanged(modifiers) => keys(modifiers),
        ModifierEvent::KeyDown(modifiers) => {
            let held = keys(modifiers);
            ModifierKeys {
                meta: current.meta && held.meta,
                ctrl: current.ctrl && held.ctrl,
                shift: current.shift && held.shift,
                alt: current.alt && held.alt,
            }
        }
    }
}

/// The held modifiers, shared by every view that draws hints.
#[derive(Default)]
pub struct ShortcutModifierStore {
    held: ModifierKeys,
    /// Focus keys when the state last changed, so hints never show over a terminal.
    focus: ShortcutContext,
    jump_hints: bool,
}

struct GlobalStore(Entity<ShortcutModifierStore>);

impl Global for GlobalStore {}

impl ShortcutModifierStore {
    /// Creates the shared store. Call once at startup, after the keymap.
    pub fn init(cx: &mut App) {
        let store = cx.new(|_| Self::default());
        cx.set_global(GlobalStore(store));
    }

    /// The shared store; observe it to redraw when hints toggle.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStore>().0.clone()
    }

    pub fn held(&self) -> ModifierKeys {
        self.held
    }

    /// Held modifiers, or none while a text field has focus (`ignoreEditable`).
    pub fn held_outside_editable(&self) -> ModifierKeys {
        if self.focus.get(WhenKey::EditableFocus) { ModifierKeys::default() } else { self.held }
    }

    /// Whether the held modifiers are the thread jump chord's.
    pub fn show_thread_jump_hints(&self) -> bool {
        self.jump_hints
    }

    /// Applies an event; `focus` carries the focus keys at that moment.
    pub fn apply(&mut self, event: ModifierEvent, focus: ShortcutContext, cx: &mut Context<Self>) {
        let held = state_after_event(self.held, event);
        self.set(held, focus, cx);
    }

    /// Clears everything, as on window blur or paste.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.set(ModifierKeys::default(), self.focus, cx);
    }

    fn set(&mut self, held: ModifierKeys, focus: ShortcutContext, cx: &mut Context<Self>) {
        let shortcuts = cx.global::<ShortcutState>();
        let jump_hints = should_show_thread_jump_hints(held, &shortcuts.config, focus, shortcuts.platform);
        if held == self.held && focus == self.focus && jump_hints == self.jump_hints {
            return;
        }
        self.held = held;
        self.focus = focus;
        self.jump_hints = jump_hints;
        cx.notify();
    }
}
