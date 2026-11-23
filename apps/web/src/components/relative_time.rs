//! Port of `RelativeTime.tsx` and `useRelativeTimeState.ts`: a "5m ago" label that
//! re-renders only when its text can change (each minute under an hour, each hour
//! under a day, then daily), on one timer per label and never per frame.

use std::time::Duration;

use gpui_kit::{Context, IntoElement, Render, SharedString, Task, Window};

use crate::timestamp_format::{
    RelativeTimeState, format_relative_elapsed, ms_until_relative_time_change, now_ms, relative_time_elapsed_ms,
};

/// What a relative-time view shows for `iso` at `now_ms`.
pub fn relative_time_state(iso: Option<&str>, now_ms: i64) -> RelativeTimeState {
    let Some(iso) = iso else {
        return RelativeTimeState::Missing;
    };
    match relative_time_elapsed_ms(iso, now_ms) {
        Some(elapsed) => RelativeTimeState::Relative(format_relative_elapsed(elapsed)),
        None => RelativeTimeState::Invalid,
    }
}

/// A view that renders the relative label for a timestamp, or nothing when it
/// is invalid. Create it with `cx.new(|cx| RelativeTime::new(timestamp, cx))`.
pub struct RelativeTime {
    timestamp: SharedString,
    _tick: Task<()>,
}

impl RelativeTime {
    pub fn new(timestamp: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let timestamp = timestamp.into();
        Self {
            _tick: Self::schedule(timestamp.clone(), cx),
            timestamp,
        }
    }

    /// Points the label at another timestamp and restarts its timer.
    pub fn set_timestamp(&mut self, timestamp: impl Into<SharedString>, cx: &mut Context<Self>) {
        let timestamp = timestamp.into();
        if timestamp != self.timestamp {
            self._tick = Self::schedule(timestamp.clone(), cx);
            self.timestamp = timestamp;
            cx.notify();
        }
    }

    /// Sleeps until the label's next change, re-renders, and repeats. Dropping
    /// the task (with the view) stops it.
    fn schedule(timestamp: SharedString, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while let Some(delay) = ms_until_relative_time_change(&timestamp, now_ms()) {
                let delay = Duration::from_millis(delay.max(1) as u64);
                cx.background_executor().timer(delay).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
    }
}

impl Render for RelativeTime {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match relative_time_state(Some(&self.timestamp), now_ms()) {
            RelativeTimeState::Relative(parts) => parts.label,
            RelativeTimeState::Missing | RelativeTimeState::Invalid => SharedString::default(),
        }
    }
}
