//! Locale-free parts of `packages/shared/src/usageFormat.ts`: the usage
//! window, the day and hour buckets a window spans, ISO instants the way
//! JavaScript prints them, and source warnings. Locale formatting of days and
//! hours lives with the usage views in `apps/web`.

use std::collections::{BTreeMap, BTreeSet};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _};
use t3code_contracts::{UsageDay, UsageResolution, UsageSourceStatus, UsageSummaryInput, UsageSummarySourcesItem};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;

/// `Date.parse` for the ISO forms the server sends: an RFC 3339 instant or a
/// bare `YYYY-MM-DD`, which JavaScript reads as UTC midnight.
pub fn parse_instant_ms(value: &str) -> Option<i64> {
    if let Ok(ts) = value.parse::<Timestamp>() {
        return Some(ts.as_millisecond());
    }
    let date = value.parse::<Date>().ok()?;
    Some(date.to_zoned_datetime(TimeZone::UTC).ok()?.timestamp().as_millisecond())
}

/// `new Date(ms).toISOString()`: always milliseconds and a `Z`.
pub fn iso_millis(ms: i64) -> String {
    let Ok(ts) = Timestamp::from_millisecond(ms) else { return String::new() };
    let dt = ts.to_zoned_datetime(TimeZone::UTC).datetime();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.subsec_nanosecond() / 1_000_000
    )
}

/// The calendar days from `since_day` through `until_day`, inclusive.
pub fn enumerate_days(since_day: &str, until_day: &str) -> Vec<String> {
    let midnight = |day: &str| parse_instant_ms(&format!("{day}T00:00:00Z"));
    let (Some(start), Some(end)) = (midnight(since_day), midnight(until_day)) else {
        return Vec::new();
    };
    if end < start {
        return Vec::new();
    }
    (0..)
        .map(|index| start + index * DAY_MS)
        .take_while(|cursor| *cursor <= end)
        .map(|cursor| iso_millis(cursor)[..10].to_owned())
        .collect()
}

/// Each hour start from `since_time` up to, not including, `until_time`.
pub fn enumerate_hour_starts(since_time: &str, until_time: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (parse_instant_ms(since_time), parse_instant_ms(until_time)) else {
        return Vec::new();
    };
    (0..).map(|index| start + index * HOUR_MS).take_while(|cursor| *cursor < end).map(iso_millis).collect()
}

/// `makeWindow`: the `days` calendar days ending today in `zone`, or for
/// [`UsageResolution::Hour`] the rolling 24 hours ending at this minute.
pub fn make_window(days: u32, now_ms: i64, zone: &TimeZone, zone_name: &str, resolution: UsageResolution) -> UsageSummaryInput {
    let day_of = |ms: i64| {
        Timestamp::from_millisecond(ms).map(|ts| ts.to_zoned_datetime(zone.clone()).date().to_string()).unwrap_or_default()
    };
    let until_day = day_of(now_ms);
    let base = UsageSummaryInput {
        since_day: UsageDay::new(until_day.as_str()),
        until_day: UsageDay::new(until_day.as_str()),
        time_zone: zone_name.to_owned(),
        resolution: Some(resolution),
        since_time: None,
        until_time: None,
        await_refresh: None,
        provisional: None,
        model_filter: None,
    };
    match resolution {
        UsageResolution::Hour => {
            let until = now_ms.div_euclid(60_000) * 60_000;
            let since = until - 24 * HOUR_MS;
            UsageSummaryInput {
                since_day: UsageDay::new(day_of(since)),
                since_time: Some(iso_millis(since)),
                until_time: Some(iso_millis(until)),
                ..base
            }
        }
        UsageResolution::Day => {
            let today: Date = until_day.parse().unwrap_or_default();
            let start = today.checked_sub((i64::from(days) - 1).days()).unwrap_or(today);
            UsageSummaryInput { since_day: UsageDay::new(start.to_string()), ..base }
        }
    }
}

/// The server's English wording, which it also appends to a source's message.
pub fn format_malformed_usage_warning(count: u64) -> String {
    format!("{count} malformed or inconsistent usage record(s) were skipped; usage may be incomplete.")
}

/// `usageSourceMessages`: the distinct warnings worth showing for these
/// sources, with malformed records summed once per source directory and
/// worded by `malformed_warning`.
pub fn usage_source_messages(
    sources: &[&UsageSummarySourcesItem],
    malformed_warning: impl Fn(u64) -> String,
) -> Vec<String> {
    let mut messages: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut malformed_by_source: BTreeMap<String, u64> = BTreeMap::new();
    let mut push = |message: String, messages: &mut Vec<String>| {
        if seen.insert(message.clone()) {
            messages.push(message);
        }
    };
    for source in sources {
        let status = source.status;
        if status != UsageSourceStatus::Partial
            && status != UsageSourceStatus::Failed
            && source.fingerprint.provider.as_str() != "cursor"
            && !(status == UsageSourceStatus::Missing && source.malformed_records > 0)
        {
            continue;
        }
        let mut message = source.message.clone();
        if source.malformed_records > 0 {
            let fp = &source.fingerprint;
            let key = format!("{}\0{}\0{}\0{}", fp.host_id, fp.provider.as_str(), fp.resolved_home_path, fp.volume_id);
            let entry = malformed_by_source.entry(key).or_default();
            *entry = (*entry).max(source.malformed_records);
            let warning = format_malformed_usage_warning(source.malformed_records);
            if let Some(text) = &message
                && let Some(stripped) = text.strip_suffix(&warning)
            {
                message = Some(stripped.trim().to_owned());
            }
        }
        if let Some(message) = message.filter(|m| !m.is_empty()) {
            push(message, &mut messages);
        }
    }
    let malformed: u64 = malformed_by_source.values().sum();
    if malformed > 0 {
        push(malformed_warning(malformed), &mut messages);
    }
    messages
}
