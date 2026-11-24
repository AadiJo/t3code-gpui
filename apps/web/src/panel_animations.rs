//! Panel open and close transitions. Ports `panelAnimations.ts`: a panel
//! stays mounted while it closes, and its width or height eases between 0
//! and full over the Panel animations setting (`ease-out`). The setting at
//! 0 ms or the OS reduced-motion preference makes every change instant.
//!
//! A transition repaints only while it runs: [`PanelTransition::fraction`]
//! asks for the next frame until the end, then stops. Time is GPUI's
//! executor clock, so headless checks can step it.

use std::time::{Duration, Instant};

use gpui_kit::{App, Window};

use crate::ActiveAppTheme as _;
use crate::components::ui::motion::prefers_reduced_motion;

/// One panel's open state and its transition between closed (0) and open (1).
#[derive(Clone, Copy, Debug)]
pub struct PanelTransition {
    open: bool,
    /// Where the running transition started, and when.
    from: f32,
    started: Option<Instant>,
    duration: Duration,
}

impl PanelTransition {
    /// A panel at rest, open or closed.
    pub fn new(open: bool) -> Self {
        Self { open, from: if open { 1. } else { 0. }, started: None, duration: Duration::ZERO }
    }

    /// Whether the panel is open (its target, not where the transition is).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Moves toward `open`, starting from wherever a running transition is,
    /// as a CSS transition reverses. Instant when `animate` is false.
    pub fn set(&mut self, open: bool, animate: bool, duration: Duration, now: Instant) {
        if open == self.open {
            return;
        }
        self.from = self.linear_at(now);
        self.open = open;
        self.duration = duration;
        self.started = (animate && !duration.is_zero()).then_some(now);
    }

    /// Follows `open` with the app's Panel animations setting and the OS
    /// reduced-motion preference (`usePanelAnimationSettings`).
    pub fn follow(&mut self, open: bool, cx: &App) {
        let duration = cx.app_theme().motion.panel;
        self.set(open, !prefers_reduced_motion(cx), duration, cx.background_executor().now());
    }

    /// Jumps to `open` with no transition (restoring saved state).
    pub fn snap(&mut self, open: bool) {
        *self = Self::new(open);
    }

    fn target(&self) -> f32 {
        if self.open { 1. } else { 0. }
    }

    /// Linear progress from `from` toward the target.
    fn linear_at(&self, now: Instant) -> f32 {
        let Some(started) = self.started else { return self.target() };
        let t = (now.duration_since(started).as_secs_f32() / self.duration.as_secs_f32()).min(1.);
        self.from + (self.target() - self.from) * crate::theme_tokens::Motion::default().ease_out.ease(t)
    }

    /// Whether the panel must be drawn: open, or still closing (`usePanelPresence`).
    pub fn present(&self) -> bool {
        self.open || self.started.is_some()
    }

    /// The open fraction to draw now, 0 to 1. While a transition runs it
    /// requests the next frame; once it ends it settles and requests nothing.
    pub fn fraction(&mut self, window: &mut Window, cx: &App) -> f32 {
        let Some(started) = self.started else { return self.target() };
        let now = cx.background_executor().now();
        if now.duration_since(started) >= self.duration {
            self.started = None;
            self.from = self.target();
            return self.target();
        }
        window.request_animation_frame();
        self.linear_at(now)
    }
}
