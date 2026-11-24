//! Page Up / Page Down in the composer when the prompt cannot scroll that way: the timeline
//! pages instead (`pageScrollController.ts`). A press animates one page over 150 ms with the
//! CSS `ease-in-out` curve; while the key is held, each repeat scrolls at the page speed, which
//! grows to twice that over 400 ms. A page is the viewport less 36px and the composer's inset.

use std::time::{Duration, Instant};

use gpui_kit::{Context, Pixels, Task, WeakEntity, px};

use super::messages_timeline::MessagesTimeline;
use super::timeline_viewport::TimelineNavigation;

const ANIMATION: Duration = Duration::from_millis(150);
const ACCELERATION_MS: f32 = 400.;
const MAX_MULTIPLIER: f32 = 2.;
const ALIGNMENT_OFFSET: Pixels = px(36.);
const BOUNDARY_EPSILON: Pixels = px(1.);
/// A held key starts its continuous scroll once the press's own page has had this long.
const HOLD_DELAY: Duration = ANIMATION;
/// One animation step; the page animation needs no faster clock than a 120 Hz display.
const FRAME: Duration = Duration::from_millis(8);

/// Pages the timeline for the composer. Held by the composer; `press` runs on each Page key
/// action (repeats included) and `release` on key-up.
pub struct PageScroller {
    timeline: WeakEntity<MessagesTimeline>,
    /// The held key (`down`), when it went down and when it last moved the list.
    held: Option<(bool, Instant, Instant)>,
    animation: Option<Task<()>>,
}

impl PageScroller {
    pub fn new(timeline: WeakEntity<MessagesTimeline>) -> Self {
        Self { timeline, held: None, animation: None }
    }

    /// A Page key the prompt could not use. Returns false when the timeline cannot move that way.
    pub fn press<V: 'static>(&mut self, down: bool, cx: &mut Context<V>) -> bool {
        let Some(timeline) = self.timeline.upgrade() else { return false };
        let (bounds, content, top, _) = timeline.read(cx).list_state().viewport_metrics();
        let max = (content - bounds.size.height).max(px(0.));
        let can = if down { top < max - BOUNDARY_EPSILON } else { top > BOUNDARY_EPSILON };
        let distance = (bounds.size.height - ALIGNMENT_OFFSET - timeline.read(cx).composer_bottom()).max(px(0.));
        let now = Instant::now();
        match self.held {
            Some((held_down, start, last)) if held_down == down => {
                if now.duration_since(start) < HOLD_DELAY || !can {
                    return can;
                }
                // `startHoldScroll`: speed is a page per 150 ms, times the hold multiplier.
                self.animation = None;
                let held_ms = now.duration_since(start).as_secs_f32() * 1000.;
                let multiplier = 1. + (held_ms / ACCELERATION_MS).min(1.) * (MAX_MULTIPLIER - 1.);
                let velocity = f32::from(distance) * multiplier / ANIMATION.as_secs_f32() / 1000.;
                let elapsed = now.duration_since(last).as_secs_f32() * 1000.;
                self.held = Some((down, start, now));
                scroll(&timeline, px(velocity * elapsed), down, cx);
                true
            }
            _ => {
                if !can {
                    return false;
                }
                self.held = Some((down, now, now));
                self.animate(timeline.downgrade(), distance, down, cx);
                true
            }
        }
    }

    /// The Page key came up (or another key went down): a hold stops where it is.
    pub fn release(&mut self) {
        if self.held.take().is_some_and(|(_, start, _)| start.elapsed() >= HOLD_DELAY) {
            self.animation = None;
        }
    }

    fn animate<V: 'static>(&mut self, timeline: WeakEntity<MessagesTimeline>, distance: Pixels, down: bool, cx: &mut Context<V>) {
        self.animation = Some(cx.spawn(async move |_, cx| {
            let start = Instant::now();
            let mut done = 0.;
            loop {
                cx.background_executor().timer(FRAME).await;
                let progress = (start.elapsed().as_secs_f32() / ANIMATION.as_secs_f32()).min(1.);
                let eased = ease_in_out(progress);
                let step = (eased - done) * f32::from(distance);
                done = eased;
                let Some(timeline) = timeline.upgrade() else { return };
                cx.update(|cx| scroll(&timeline, px(step), down, cx));
                if progress >= 1. {
                    return;
                }
            }
        }));
    }
}

fn scroll(timeline: &gpui_kit::Entity<MessagesTimeline>, delta: Pixels, down: bool, cx: &mut gpui_kit::App) {
    timeline.update(cx, |timeline, cx| {
        timeline.list_state().scroll_by(if down { delta } else { -delta }, TimelineNavigation::Key { up: !down });
        cx.notify();
    });
}

/// CSS `ease-in-out` (`cubic-bezier(0.42, 0, 0.58, 1)`), solved for x by Newton steps as the
/// fork's `easeInOut` does.
fn ease_in_out(progress: f32) -> f32 {
    let (ax, bx, cx) = (3. * 0.42 - 3. * 0.58 + 1., 3. * (0.58 - 2. * 0.42), 3. * 0.42);
    let (ay, by) = (-2., 3.);
    let x = |t: f32| ((ax * t + bx) * t + cx) * t;
    let mut t = progress;
    for _ in 0..5 {
        let delta = x(t) - progress;
        let derivative = (3. * ax * t + 2. * bx) * t + cx;
        if delta.abs() < 1e-4 || derivative == 0. {
            break;
        }
        t = (t - delta / derivative).clamp(0., 1.);
    }
    (ay * t + by) * t * t
}
