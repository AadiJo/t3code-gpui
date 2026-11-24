//! Keyboard control: the command registry ([`commands`]), the GPUI keymap
//! built from keybinding rules ([`keymap`]), and [`ShortcutRoot`], the window
//! root that hosts them. Ports `apps/web/src/shortcuts/`.
//!
//! Wiring: call [`init`] after every widget `init`, render a [`ShortcutRoot`]
//! as the window's root view around the app (`AppRoot::open_window` does),
//! and call [`keymap::install`] whenever the server config's keybindings change.

pub mod commands;
pub mod keymap;

use std::rc::Rc;

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};

use crate::command_palette_store::OpenIntent;
use crate::components::background_thread_announcer::BackgroundThreadAnnouncer;
use crate::components::command_palette::{self, CommandPalette};
use crate::components::command_palette_lookups::CommandPaletteSource;
use crate::components::command_palette_theme_items::cycle_appearance;
use crate::components::ui::toast::Toaster;
use crate::shortcut_modifier_state::{ModifierEvent, ShortcutModifierStore};

/// Binds the palette's keys, installs the default keymap and the modifier
/// store. Call once, after every widget's `init`, so app rules come last.
pub fn init(cx: &mut App) {
    command_palette::init(cx);
    keymap::init(cx);
    ShortcutModifierStore::init(cx);
}

/// mod+q with no other modifier: `meta` on macOS, `control` elsewhere (`QuitHold.ts`).
fn is_quit_chord(keystroke: &gpui_kit::Keystroke) -> bool {
    let m = keystroke.modifiers;
    let modifier = if cfg!(target_os = "macos") { m.platform && !m.control } else { m.control && !m.platform };
    keystroke.key == "q" && modifier && !m.alt && !m.shift
}

/// The window root for keyboard control. It carries the shortcut state's key
/// context, keeps focus inside the window, tracks held modifiers, hosts the
/// window-wide layers (command palette, toaster, screen-reader announcer and
/// text selection), and answers the palette's commands
/// (`commandPaletteCommands.ts`).
pub struct ShortcutRoot {
    content: AnyView,
    palette: Entity<CommandPalette>,
    focus: FocusHandle,
    /// Where focus goes when nothing holds it; the root's own by default.
    default_focus: Option<FocusHandle>,
    _activation: Subscription,
}

impl ShortcutRoot {
    pub fn new(content: AnyView, source: Rc<dyn CommandPaletteSource>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let palette = cx.new(|cx| CommandPalette::new(source, cx));
        // Dictation tools can leave Meta stuck; losing the window clears it.
        let activation = cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                ShortcutModifierStore::global(cx).update(cx, |store, cx| store.reset(cx));
            }
        });
        Self { content, palette, focus: cx.focus_handle(), default_focus: None, _activation: activation }
    }

    /// Makes `focus` (inside the content) take the keyboard when nothing
    /// holds it, so the content's command handlers are on the focus path.
    pub fn set_default_focus(&mut self, focus: FocusHandle) {
        self.default_focus = Some(focus);
    }

    pub fn palette(&self) -> &Entity<CommandPalette> {
        &self.palette
    }

    /// The root's own focus, which holds the keyboard when nothing else does.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
}

impl Render for ShortcutRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(self.default_focus.as_ref().unwrap_or(&self.focus), cx);
        }
        let key_context = keymap::ShortcutState::get(cx).root_key_context();
        let palette = self.palette.clone();
        div()
            .id("shortcut-root")
            .key_context(key_context)
            .track_focus(&self.focus)
            .size_full()
            .capture_key_up(|event, _, cx| {
                if event.keystroke.key == "q" {
                    crate::quit_hold::QuitHold::q_up(cx);
                }
            })
            .on_modifiers_changed(|event, window, cx| {
                let modifier = if cfg!(target_os = "macos") { event.modifiers.platform } else { event.modifiers.control };
                if !modifier {
                    crate::quit_hold::QuitHold::modifier_up(cx);
                }
                let context = keymap::current_context(window, cx);
                let event = ModifierEvent::ModifiersChanged(event.modifiers);
                ShortcutModifierStore::global(cx).update(cx, |store, cx| store.apply(event, context, cx));
            })
            .capture_key_down(|event, window, cx| {
                if event.keystroke.is_modifier_key() {
                    return;
                }
                // The quit shortcut goes through the quit confirmation, not straight to Quit.
                if is_quit_chord(&event.keystroke) {
                    crate::quit_hold::QuitHold::key_down(event.is_held, cx);
                    cx.stop_propagation();
                    return;
                }
                crate::quit_hold::QuitHold::other_key(cx);
                let context = keymap::current_context(window, cx);
                let event = ModifierEvent::KeyDown(event.keystroke.modifiers);
                ShortcutModifierStore::global(cx).update(cx, |store, cx| store.apply(event, context, cx));
            })
            .on_action({
                let palette = palette.clone();
                move |_: &commands::ToggleCommandPalette, window, cx| {
                    palette.update(cx, |palette, cx| palette.toggle(window, cx));
                }
            })
            .on_action({
                let palette = palette.clone();
                move |_: &commands::SelectTheme, window, cx| {
                    palette.update(cx, |palette, cx| palette.open(OpenIntent::ChangeTheme, window, cx));
                }
            })
            .on_action(|_: &commands::CycleAppearance, _, cx| cycle_appearance(cx))
            .child(gpui_kit::base::TextSelectionLayer)
            .child(self.content.clone())
            .child(palette)
            .child(Toaster::view(cx))
            .children(crate::quit_hold::quit_hold_overlay(cx))
            .child(BackgroundThreadAnnouncer::view(cx))
    }
}
