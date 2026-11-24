//! Quit confirmation for the quit shortcut (`QuitHold.ts`, `QuitHoldOverlay.tsx`). With the
//! "hold" setting, mod+q must be held 1.2 s (or pressed twice within 0.5 s); with "double
//! press" it must be pressed twice; "direct" quits at once. The indicator shows while the
//! shortcut is down and lingers 1.2 s after a hold is released. The menu's Quit item is not
//! affected.

use std::time::{Duration, Instant};

use gpui_kit::*;
use t3code_contracts::{ClientSettingsSchemaConfirmQuit, QuitConfirmationMode};

use crate::ActiveAppTheme as _;
use crate::i18n::t;

/// `QUIT_HOLD_DURATION_MS`.
const HOLD: Duration = Duration::from_millis(1200);
/// `QUIT_DOUBLE_PRESS_MS`.
const DOUBLE_PRESS: Duration = Duration::from_millis(500);
/// `QUIT_HOLD_RELEASE_GRACE_MS`: once held, the quit waits for a quiet period so trailing
/// repeats cannot reach the next app.
const RELEASE_GRACE: Duration = Duration::from_millis(600);
/// `HOLD_HINT_LINGER_MS`.
const HINT_LINGER: Duration = Duration::from_millis(1200);

/// Which hint the indicator shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HintMode {
    Hold,
    DoublePress,
}

/// The shortcut's state (`makeQuitShortcutHandler`).
#[derive(Default)]
pub struct QuitHold {
    holding: bool,
    armed: bool,
    quit_on_release: bool,
    mode: Option<HintMode>,
    held_since: Option<Instant>,
    last_press: Option<Instant>,
    last_repeat: Option<Instant>,
    repeat_cadence: Duration,
    /// Bumped whenever a pending timer must not act.
    generation: u64,
    /// The indicator on screen.
    visible: Option<HintMode>,
}

impl Global for QuitHold {}

/// The user's quit setting (`confirmQuit`; the legacy `true` means hold).
fn quit_mode(cx: &App) -> QuitConfirmationMode {
    use crate::hooks::use_settings::{ClientSettings, ClientSettingsStore};
    let setting = match cx.has_global::<ClientSettingsStore>() {
        true => ClientSettingsStore::get(cx).confirm_quit.clone(),
        false => ClientSettings::default().confirm_quit,
    };
    match setting {
        ClientSettingsSchemaConfirmQuit::QuitConfirmationMode(mode) => mode,
        ClientSettingsSchemaConfirmQuit::Bool(true) => QuitConfirmationMode::Hold,
        ClientSettingsSchemaConfirmQuit::Bool(false) => QuitConfirmationMode::Direct,
    }
}

impl QuitHold {
    fn state(cx: &mut App) -> &mut Self {
        cx.default_global::<Self>()
    }

    /// A mod+q key down (`is_held` for auto-repeat). The caller stops the key there.
    pub fn key_down(is_held: bool, cx: &mut App) {
        let now = Instant::now();
        let state = Self::state(cx);
        if is_held {
            state.repeat_cadence = now - state.last_repeat.or(state.held_since).unwrap_or(now);
            state.last_repeat = Some(now);
        }
        if state.quit_on_release {
            Self::quit_after_quiet_period(cx);
            return;
        }
        if is_held {
            let held_long = state.held_since.is_some_and(|since| now - since >= HOLD);
            if state.mode == Some(HintMode::Hold) && state.armed && held_long {
                state.armed = false;
                state.quit_on_release = true;
                // `concealWindow`: the app steps aside while the key is still down.
                cx.hide();
                Self::quit_after_quiet_period(cx);
            }
            return;
        }
        let previous = state.last_press.replace(now);
        if state.holding || state.visible.is_some() {
            Self::release(cx, false);
        }
        let state = Self::state(cx);
        state.generation += 1;
        if previous.is_some_and(|at| now - at <= DOUBLE_PRESS) {
            return Self::quit_now(cx);
        }
        let generation = state.generation;
        match quit_mode(cx) {
            QuitConfirmationMode::Direct => Self::quit_now(cx),
            QuitConfirmationMode::DoubleClick => {
                let state = Self::state(cx);
                state.mode = Some(HintMode::DoublePress);
                Self::show(HintMode::DoublePress, cx);
                Self::after(DOUBLE_PRESS, generation, cx, |cx| Self::release(cx, false));
            }
            QuitConfirmationMode::Hold => {
                let state = Self::state(cx);
                state.holding = true;
                state.held_since = Some(now);
                state.mode = Some(HintMode::Hold);
                state.armed = true;
                Self::show(HintMode::Hold, cx);
                Self::after(HOLD + RELEASE_GRACE, generation, cx, |cx| Self::release(cx, false));
            }
        }
    }

    /// The q key came up.
    pub fn q_up(cx: &mut App) {
        let quit = Self::state(cx).quit_on_release;
        Self::release(cx, true);
        if quit {
            Self::quit(cx);
        }
    }

    /// The quit modifier came up.
    pub fn modifier_up(cx: &mut App) {
        if Self::state(cx).quit_on_release {
            Self::quit_after_quiet_period(cx);
        } else {
            Self::release(cx, true);
        }
    }

    /// Another key or chord: a pending press no longer counts.
    pub fn other_key(cx: &mut App) {
        let state = Self::state(cx);
        if state.quit_on_release || (!state.holding && state.visible.is_none()) {
            return;
        }
        state.last_press = None;
        Self::release(cx, false);
    }

    /// The indicator's message while it shows.
    pub fn hint(cx: &App) -> Option<SharedString> {
        let mode = cx.try_global::<Self>()?.visible?;
        let shortcut = if cfg!(target_os = "macos") { "⌘Q" } else { "Ctrl+Q" };
        Some(match mode {
            HintMode::Hold => t!("Hold {shortcut} or press twice to quit", shortcut = shortcut),
            HintMode::DoublePress => t!("Press {shortcut} again to quit", shortcut = shortcut),
        })
    }

    /// Ends a press; a hold's hint lingers, a double-press hint goes at once. A key coming up
    /// (`keep_hint`) keeps the double-press hint and its timer for the second press.
    fn release(cx: &mut App, keep_hint: bool) {
        let state = Self::state(cx);
        let mode = state.mode.take();
        state.holding = false;
        state.armed = false;
        state.quit_on_release = false;
        state.last_repeat = None;
        state.repeat_cadence = Duration::ZERO;
        if keep_hint && mode == Some(HintMode::DoublePress) {
            // Released between the two presses: the hint stays for the second.
            state.mode = mode;
            return;
        }
        state.generation += 1;
        let generation = state.generation;
        match mode {
            Some(HintMode::Hold) => Self::after(HINT_LINGER, generation, cx, Self::hide_hint),
            _ => Self::hide_hint(cx),
        }
    }

    fn quit_after_quiet_period(cx: &mut App) {
        let state = Self::state(cx);
        state.generation += 1;
        let generation = state.generation;
        let quiet = RELEASE_GRACE.max(state.repeat_cadence * 2);
        Self::after(quiet, generation, cx, Self::quit_now);
    }

    fn quit_now(cx: &mut App) {
        let state = Self::state(cx);
        state.last_press = None;
        state.generation += 1;
        Self::hide_hint(cx);
        Self::quit(cx);
    }

    /// Runs the app's Quit, which writes state first.
    fn quit(cx: &mut App) {
        cx.dispatch_action(&crate::app_root::Quit);
    }

    fn show(mode: HintMode, cx: &mut App) {
        Self::state(cx).visible = Some(mode);
        cx.refresh_windows();
    }

    fn hide_hint(cx: &mut App) {
        if Self::state(cx).visible.take().is_some() {
            cx.refresh_windows();
        }
    }

    /// Runs `then` after `delay` unless another press or release came first.
    fn after(delay: Duration, generation: u64, cx: &mut App, then: fn(&mut App)) {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                if cx.try_global::<Self>().is_some_and(|s| s.generation == generation) {
                    then(cx);
                }
            });
        })
        .detach();
    }
}

/// The indicator (`QuitHoldOverlay`): a pill 22% from the top, centered, over everything.
pub fn quit_hold_overlay(cx: &App) -> Option<AnyElement> {
    let message = QuitHold::hint(cx)?;
    let theme = cx.app_theme();
    let colors = &theme.colors;
    Some(
        div()
            .absolute()
            .top(relative(0.22))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .rounded_full()
                    .bg(colors.foreground().opacity(0.95))
                    .px(theme.sp(8.))
                    .py(theme.sp(4.))
                    .text_size(theme.rem(1.5))
                    .line_height(theme.rem(2.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.background())
                    .shadow_xl()
                    .child(message),
            )
            .into_any_element(),
    )
}
