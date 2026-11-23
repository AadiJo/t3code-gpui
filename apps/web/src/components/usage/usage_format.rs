//! The usage chart's date labels: ports of `formatDayShort`,
//! `formatHourShort`, `formatDateTimeShort` (`packages/shared/src/usageFormat.ts`)
//! and `formatRelativeHourShort` (`usageFormatCopy.ts`). Patterns per UI
//! locale come from node's `Intl` in [`usage_date_table`](super::usage_date_table).

use jiff::Timestamp;
use jiff::tz::TimeZone;
use t3code_shared::usage_format::parse_instant_ms;

use crate::i18n::locales::Locale;
use crate::i18n::t;

use super::usage_date_table::TABLE;

/// How a locale prints the hour for `hour: "numeric"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HourStyle {
    Twelve,
    TwentyFour,
    TwentyFour2,
}

/// One locale's short date and hour patterns; `{d}` is the day of month and
/// `{h}` the hour.
pub(super) struct DatePatterns {
    pub hour_style: HourStyle,
    pub day: [&'static str; 12],
    pub hour_am: &'static str,
    pub hour_pm: &'static str,
    pub hour_h23: &'static str,
    pub date_hour_am: [&'static str; 12],
    pub date_hour_pm: [&'static str; 12],
    pub date_hour_h23: [&'static str; 12],
}

fn patterns(locale: Locale) -> &'static DatePatterns {
    TABLE.iter().find(|(l, _)| *l == locale).map_or(&TABLE[0].1, |(_, p)| p)
}

/// The zone a usage window was read in; an unknown name reads as UTC.
pub fn usage_zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

/// "Nov 3" (or the locale's short month and day) for a `YYYY-MM-DD` day.
pub fn format_day_short(day: &str, locale: Locale) -> String {
    let mut parts = day.split('-').map(|part| part.parse::<u32>());
    let (Some(Ok(_)), Some(Ok(month)), Some(Ok(day_of_month))) = (parts.next(), parts.next(), parts.next()) else {
        return day.to_owned();
    };
    let Some(pattern) = patterns(locale).day.get(month.wrapping_sub(1) as usize) else { return day.to_owned() };
    pattern.replace("{d}", &day_of_month.to_string())
}

fn hour_text(hour: i8, style: HourStyle) -> String {
    let twelve = if hour % 12 == 0 { 12 } else { hour % 12 };
    match style {
        HourStyle::Twelve => twelve.to_string(),
        HourStyle::TwentyFour => hour.to_string(),
        HourStyle::TwentyFour2 => format!("{hour:02}"),
    }
}

fn fill(pattern: &str, day: i8, hour: &str) -> String {
    pattern.replace("{d}", &day.to_string()).replace("{h}", hour)
}

/// The hour of `hour_start` in `zone`: "3 PM", or "15:00" without `hour12`.
/// A wall hour that repeats when clocks fall back also names its zone.
pub fn format_hour_short(hour_start: &str, zone: &TimeZone, hour12: bool, locale: Locale) -> String {
    let Some(ms) = parse_instant_ms(hour_start) else { return hour_start.to_owned() };
    let Ok(instant) = Timestamp::from_millisecond(ms) else { return hour_start.to_owned() };
    let zoned = instant.to_zoned_datetime(zone.clone());
    let p = patterns(locale);
    let label = if hour12 {
        let pattern = if zoned.hour() < 12 { p.hour_am } else { p.hour_pm };
        fill(pattern, zoned.day(), &hour_text(zoned.hour(), p.hour_style))
    } else {
        fill(p.hour_h23, zoned.day(), &format!("{:02}", zoned.hour()))
    };
    let wall = |at: i64| {
        Timestamp::from_millisecond(at).map(|ts| {
            let z = ts.to_zoned_datetime(zone.clone());
            (z.date(), z.hour())
        })
    };
    let repeated = [-3_600_000, 3_600_000].into_iter().any(|offset| wall(ms + offset).ok() == wall(ms).ok());
    if !repeated {
        return label;
    }
    // `timeZoneName: "short"`: English names the zone ("EDT"); other locales print the
    // offset in their own style, and Chinese puts it first.
    let info = zone.to_offset_info(instant);
    if locale == Locale::En {
        return format!("{label} {}", info.abbreviation());
    }
    let (prefix, minus) = match locale {
        Locale::Fr => ("UTC", '\u{2212}'),
        Locale::Sv => ("GMT", '\u{2212}'),
        _ => ("GMT", '-'),
    };
    let seconds = info.offset().seconds();
    let (sign, hours, minutes) = (if seconds < 0 { minus } else { '+' }, seconds.abs() / 3600, seconds.abs() % 3600 / 60);
    let name = if minutes == 0 { format!("{prefix}{sign}{hours}") } else { format!("{prefix}{sign}{hours}:{minutes:02}") };
    if locale == Locale::Zh { format!("{name}{label}") } else { format!("{label} {name}") }
}

/// "Nov 3, 3 PM": the day and hour of `instant` in `zone`.
pub fn format_date_time_short(instant: &str, zone: &TimeZone, hour12: bool, locale: Locale) -> String {
    let Some(ms) = parse_instant_ms(instant) else { return instant.to_owned() };
    let Ok(ts) = Timestamp::from_millisecond(ms) else { return instant.to_owned() };
    let zoned = ts.to_zoned_datetime(zone.clone());
    let p = patterns(locale);
    let month = (zoned.month() - 1) as usize;
    if hour12 {
        let pattern = if zoned.hour() < 12 { p.date_hour_am[month] } else { p.date_hour_pm[month] };
        fill(pattern, zoned.day(), &hour_text(zoned.hour(), p.hour_style))
    } else {
        fill(p.date_hour_h23[month], zoned.day(), &format!("{:02}", zoned.hour()))
    }
}

/// "3 PM today", "3 PM yesterday", or the day and hour when older.
pub fn format_relative_hour_short(hour_start: &str, relative_to: &str, zone: &TimeZone, hour12: bool, locale: Locale) -> String {
    let day_of = |value: &str| {
        parse_instant_ms(value)
            .and_then(|ms| Timestamp::from_millisecond(ms).ok())
            .map(|ts| ts.to_zoned_datetime(zone.clone()).date())
    };
    let (Some(instant), Some(reference)) = (day_of(hour_start), day_of(relative_to)) else {
        return format_date_time_short(hour_start, zone, hour12, locale);
    };
    let days_ago = (reference - instant).get_days();
    let hour = format_hour_short(hour_start, zone, hour12, locale);
    match days_ago {
        0 => t!("{hour} today", hour = hour).to_string(),
        1 => t!("{hour} yesterday", hour = hour).to_string(),
        _ => format_date_time_short(hour_start, zone, hour12, locale),
    }
}
