//! Port of `formatDuration` in `packages/shared/src/orchestrationTimingCopy.ts`.

use gpui_kit::SharedString;

use super::t;

/// `Math.round` for the non-negative values durations use.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// "850ms", "2.5s", "42s", "3m 5s", "1h 2m 3s": a work duration, in the UI language.
pub fn format_duration(duration_ms: f64) -> SharedString {
    if !duration_ms.is_finite() || duration_ms < 0.0 {
        return t!("{ms}ms", ms = 0);
    }
    if duration_ms < 1_000.0 {
        return t!("{ms}ms", ms = js_round(duration_ms).max(1.0));
    }
    if duration_ms < 10_000.0 {
        return t!("{seconds}s", seconds = js_round(duration_ms / 100.0) / 10.0);
    }
    let total_seconds = js_round(duration_ms / 1_000.0) as i64;
    // Choose the unit after rounding so 59.5 seconds never renders as "60s".
    if total_seconds < 60 {
        return t!("{seconds}s", seconds = total_seconds);
    }
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours == 0 {
        return t!("{minutes}m {seconds}s", minutes = minutes, seconds = seconds);
    }
    if minutes > 0 {
        return t!("{hours}h {minutes}m {seconds}s", hours = hours, minutes = minutes, seconds = seconds);
    }
    if seconds > 0 {
        t!("{hours}h {seconds}s", hours = hours, seconds = seconds)
    } else {
        t!("{hours}h", hours = hours)
    }
}
