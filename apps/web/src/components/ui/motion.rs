//! Stepped animation. The app's pulses hold a value, step to another in a
//! few discrete frames, hold, and step back (`index.css` keyframes with
//! `steps(n)`). Here a pulse re-renders its view only at those step
//! boundaries, never every frame.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::{App, EntityId, Global, Window};

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
