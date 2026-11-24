//! Motion. Two kinds:
//!
//! - One-shot transitions (`data-starting-style` and `data-ending-style` in
//!   the web's primitives): [`Transition`] runs GPUI's animation element for
//!   a finite duration, repainting each frame only until it ends.
//! - Stepped pulses: the app's pulses hold a value, step to another in a few
//!   discrete frames, hold, and step back (`index.css` keyframes with
//!   `steps(n)`). A pulse re-renders its view only at those step boundaries.
//!
//! `cx.reduce_motion()` (set by the headless tools) skips both. The OS
//! reduced-motion preference is [`PrefersReducedMotion`]; like the web, it
//! only turns off what the web marks `motion-reduce` (panels, heights).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, EntityId, Global, IntoElement, Styled, Window,
};

use crate::theme_tokens::CubicBezier;

/// The OS "reduce motion" accessibility setting (`prefers-reduced-motion`),
/// set by the desktop shell at startup.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrefersReducedMotion(pub bool);

impl Global for PrefersReducedMotion {}

/// Whether transitions the web marks `motion-reduce:transition-none` must be instant.
pub fn prefers_reduced_motion(cx: &App) -> bool {
    cx.reduce_motion() || cx.try_global::<PrefersReducedMotion>().is_some_and(|p| p.0)
}

/// A CSS transition: a duration and an easing curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub duration: Duration,
    pub easing: CubicBezier,
}

impl Transition {
    /// Tailwind's bare `transition`: 150ms, `cubic-bezier(0.4, 0, 0.2, 1)`.
    pub const STANDARD: Self = Self::new(150, CubicBezier(0.4, 0., 0.2, 1.));
    /// `duration-200 ease-in-out`: dialogs and sheets.
    pub const DIALOG: Self = Self::new(200, CubicBezier(0.4, 0., 0.2, 1.));
    /// `transform .5s cubic-bezier(.22,1,.36,1), opacity .5s`: toasts.
    pub const TOAST: Self = Self::new(500, CubicBezier(0.22, 1., 0.36, 1.));
    /// `duration-200 ease-out`: `AnimatedHeight` and collapsible panels.
    pub const HEIGHT: Self = Self::new(200, CubicBezier(0., 0., 0.2, 1.));
    /// `duration-150 ease-out`: `AnimatedWidth`.
    pub const WIDTH: Self = Self::new(150, CubicBezier(0., 0., 0.2, 1.));

    pub const fn new(ms: u64, easing: CubicBezier) -> Self {
        Self { duration: Duration::from_millis(ms), easing }
    }

    fn animation(self) -> Animation {
        let easing = self.easing;
        Animation::new(self.duration).with_easing(move |t| easing.ease(t))
    }

    /// Runs `animate(element, t)` with `t` easing 0 to 1 from the frame the
    /// element first appears under `id`, then holds 1 with no more frames.
    pub fn run<E: IntoElement + 'static>(
        self,
        id: impl Into<ElementId>,
        element: E,
        animate: impl Fn(E, f32) -> E + 'static,
    ) -> AnyElement {
        if self.duration.is_zero() {
            return animate(element, 1.).into_any_element();
        }
        element.with_animation(id, self.animation(), animate).into_any_element()
    }

    /// Fades in from transparent (`data-starting-style:opacity-0`).
    pub fn fade_in<E: IntoElement + Styled + 'static>(self, id: impl Into<ElementId>, element: E) -> AnyElement {
        self.run(id, element, |el, t| el.opacity(t))
    }

    /// Fades out to transparent (`data-ending-style:opacity-0`).
    pub fn fade_out<E: IntoElement + Styled + 'static>(self, id: impl Into<ElementId>, element: E) -> AnyElement {
        self.run(id, element, |el, t| el.opacity(1. - t))
    }
}

/// A duty-cycled opacity pulse: 1 until `fall_start`, `steps` steps down to
/// `low` by `fall_end`, `low` until `rise_start`, then `steps` steps back to 1.
/// Times are fractions of `cycle`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SteppedPulse {
    pub cycle: Duration,
    pub fall_start: f32,
    pub fall_end: f32,
    pub rise_start: f32,
    pub low: f32,
    pub steps: u32,
}

impl SteppedPulse {
    /// `animate-skeleton`: 2.4s, 1 to 0.55, `steps(4)`.
    pub const SKELETON: Self = Self {
        cycle: Duration::from_millis(2400),
        fall_start: 0.42,
        fall_end: 0.50,
        rise_start: 0.92,
        low: 0.55,
        steps: 4,
    };

    /// `animate-status-pulse`: 2s, 1 to 0.5, `steps(6)`.
    pub const STATUS: Self = Self {
        cycle: Duration::from_millis(2000),
        fall_start: 0.40,
        fall_end: 0.50,
        rise_start: 0.90,
        low: 0.5,
        steps: 6,
    };

    fn ramp_len(&self) -> f32 {
        self.fall_end - self.fall_start
    }

    /// The opacity at phase `t` in 0..1 (`steps(n)` jumps at the end of each step).
    pub fn value(&self, t: f32) -> f32 {
        let step = |start: f32| ((t - start) / self.ramp_len() * self.steps as f32).floor() / self.steps as f32;
        if t < self.fall_start {
            1.
        } else if t < self.fall_end {
            1. + (self.low - 1.) * step(self.fall_start)
        } else if t < self.rise_start {
            self.low
        } else {
            self.low + (1. - self.low) * step(self.rise_start)
        }
    }

    /// The phase of the next change after `t`.
    fn next_boundary(&self, t: f32) -> f32 {
        let step_len = self.ramp_len() / self.steps as f32;
        let ramps = [self.fall_start, self.rise_start];
        ramps
            .iter()
            .flat_map(|start| (0..=self.steps).map(move |i| start + step_len * i as f32))
            .find(|boundary| *boundary > t + 1e-4)
            .unwrap_or(1. + self.fall_start)
    }
}

/// Views with a step re-render already scheduled, keyed by the target time,
/// so many pulses in one view share one timer.
#[derive(Default)]
struct PendingSteps(HashSet<(EntityId, u64)>);

impl Global for PendingSteps {}

/// The shared clock all pulses read, so pulses in different views stay in phase.
struct Epoch(Instant);

impl Global for Epoch {}

/// The pulse's opacity now. Schedules the current view to re-render at the
/// next step; returns 1 and schedules nothing when the OS asks for reduced motion.
pub fn pulse_opacity(pulse: SteppedPulse, window: &mut Window, cx: &mut App) -> f32 {
    if cx.reduce_motion() {
        return 1.;
    }
    let now = Instant::now();
    let epoch = cx.try_global::<Epoch>().map(|epoch| epoch.0).unwrap_or_else(|| {
        cx.set_global(Epoch(now));
        now
    });
    let cycle = pulse.cycle.as_secs_f32();
    let elapsed = now.duration_since(epoch).as_secs_f32();
    let t = (elapsed / cycle).fract();
    let next = elapsed - t * cycle + pulse.next_boundary(t) * cycle;
    let view = window.current_view();
    let target_ms = (next * 1000.).ceil() as u64;
    if cx.default_global::<PendingSteps>().0.insert((view, target_ms)) {
        let delay = Duration::from_millis(target_ms).saturating_sub(now.duration_since(epoch));
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(delay).await;
                cx.update(|_, cx| {
                    cx.default_global::<PendingSteps>().0.remove(&(view, target_ms));
                    cx.notify(view);
                })
                .ok();
            })
            .detach();
    }
    pulse.value(t)
}

/// A measured size that eases to each new value (`transition-[height]` and
/// `transition-[width]` on a measured box). The first measurement is taken
/// as is; later ones start a transition from wherever the last one is.
#[derive(Clone, Copy, Debug, Default)]
pub struct SizeTween {
    target: Option<f32>,
    from: f32,
    started: Option<Instant>,
}

impl SizeTween {
    /// Records a measurement. Returns whether it changed the target.
    pub fn measure(&mut self, size: f32, animate: bool, transition: Transition) -> bool {
        let size = size.ceil();
        if self.target == Some(size) {
            return false;
        }
        let now = Instant::now();
        self.from = self.value_at(now, transition).unwrap_or(size);
        self.started = (animate && self.target.is_some()).then_some(now);
        self.target = Some(size);
        true
    }

    fn value_at(&self, now: Instant, transition: Transition) -> Option<f32> {
        let target = self.target?;
        let Some(started) = self.started else { return Some(target) };
        let t = (now.duration_since(started).as_secs_f32() / transition.duration.as_secs_f32()).min(1.);
        Some(self.from + (target - self.from) * transition.easing.ease(t))
    }

    /// The size to draw now, or `None` before the first measurement. While
    /// easing it requests the next frame and reports `true` (clip the content).
    pub fn current(&mut self, transition: Transition, window: &mut Window) -> Option<(f32, bool)> {
        let target = self.target?;
        let Some(started) = self.started else { return Some((target, false)) };
        if started.elapsed() >= transition.duration {
            self.started = None;
            return Some((target, false));
        }
        window.request_animation_frame();
        self.value_at(Instant::now(), transition).map(|value| (value, true))
    }
}
