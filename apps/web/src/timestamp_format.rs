//! Port of `timestampFormat.ts`. Times print in the host's regional locale with the
//! user's 12/24-hour setting; the words around them ("just now", "5m ago") are
//! translated strings. Pass `now_ms` from one clock read to keep a render consistent.

use std::sync::RwLock;

use gpui_kit::SharedString;
use jiff::{Timestamp, ZonedDateTime, civil, tz::TimeZone};

use crate::i18n::{
    Locale,
    locale_format::{LocaleData, WallClock, format_parts, locale_data},
    messages::active_locale,
    t,
};

/// The `timestampFormat` client setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimestampFormat {
    /// The regional locale's own clock.
    #[default]
    Locale,
    TwelveHour,
    TwentyFourHour,
}

impl TimestampFormat {
    /// Index into [`LocaleData::time`] and [`LocaleData::full`].
    fn pattern_index(self) -> usize {
        match self {
            Self::Locale => 0,
            Self::TwelveHour => 1,
            Self::TwentyFourHour => 2,
        }
    }
}

static TIMESTAMP_LOCALE: RwLock<Option<&'static LocaleData>> = RwLock::new(None);

/// Sets the regional locale times print in, from the host's system locale tag
/// (`desktopBridge.getSystemLocale()`). `None` or an empty tag means en-US, the
/// runtime default. `i18n::runtime::init` calls this.
pub fn set_timestamp_locale(tag: Option<&str>) {
    let tag = tag.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("en-US");
    *TIMESTAMP_LOCALE.write().unwrap() = Some(locale_data(tag));
}

fn timestamp_locale() -> &'static LocaleData {
    if let Some(data) = *TIMESTAMP_LOCALE.read().unwrap() {
        return data;
    }
    set_timestamp_locale(sys_locale::get_locale().as_deref());
    TIMESTAMP_LOCALE.read().unwrap().unwrap()
}

/// First weekday of the regional locale as a `Date#getDay` index (0 is Sunday).
pub fn week_starts_on() -> u8 {
    timestamp_locale().week_first_day
}

/// Whether times print on a 12-hour clock under `format`.
pub fn resolve_hour12(format: TimestampFormat) -> bool {
    match format {
        TimestampFormat::Locale => timestamp_locale().hour12,
        TimestampFormat::TwelveHour => true,
        TimestampFormat::TwentyFourHour => false,
    }
}

/// The current time in epoch milliseconds.
pub fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}

/// Parses the ISO 8601 forms `new Date(iso)` accepts: with an offset, a local
/// date-time without one, or a bare date (UTC midnight).
pub fn parse_timestamp_date(iso: &str) -> Option<Timestamp> {
    if let Ok(ts) = iso.parse::<Timestamp>() {
        return Some(ts);
    }
    if iso.contains('T') {
        let local = iso.parse::<civil::DateTime>().ok()?;
        return local.to_zoned_datetime(TimeZone::system()).ok().map(|z| z.timestamp());
    }
    let date = iso.parse::<civil::Date>().ok()?;
    date.to_zoned_datetime(TimeZone::UTC).ok().map(|z| z.timestamp())
}

fn zoned(ts: Timestamp, zone: Option<&TimeZone>) -> ZonedDateTime {
    ts.to_zoned_datetime(zone.cloned().unwrap_or_else(TimeZone::system))
}

fn wall_clock(z: &ZonedDateTime) -> WallClock {
    WallClock {
        year: i32::from(z.year()),
        month: z.month() as u8,
        day: z.day() as u8,
        hour: z.hour() as u8,
        minute: z.minute() as u8,
        second: z.second() as u8,
    }
}

fn format_time(ts: Timestamp, format: TimestampFormat, zone: Option<&TimeZone>) -> String {
    let parts = timestamp_locale().time[format.pattern_index()];
    format_parts(parts, wall_clock(&zoned(ts, zone)))
}

const ENGLISH_MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
    "November", "December",
];

fn ordinal_suffix(day: u8) -> &'static str {
    match (day % 100, day % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    }
}

/// "3:05 PM, 7th March 2026" in English; other UI languages use their own long date.
pub fn format_chat_timestamp_tooltip(iso: &str, format: TimestampFormat) -> SharedString {
    let Some(ts) = parse_timestamp_date(iso) else {
        return SharedString::default();
    };
    let time = format_time(ts, format, None);
    let clock = wall_clock(&zoned(ts, None));
    let locale = active_locale();
    if locale != Locale::En {
        let date = format_parts(locale_data(locale.tag()).long_date, clock);
        return t!("{time}, {date}", context = "message time, then its date", time = time, date = date);
    }
    format!(
        "{time}, {}{} {} {}",
        clock.day,
        ordinal_suffix(clock.day),
        ENGLISH_MONTHS[usize::from(clock.month - 1)],
        clock.year
    )
    .into()
}

/// `new Date(iso).toLocaleString(undefined, …)` in the regional locale.
pub fn format_full_timestamp(iso: &str, format: TimestampFormat) -> String {
    let Some(ts) = parse_timestamp_date(iso) else {
        return "Invalid Date".to_owned();
    };
    format_parts(timestamp_locale().full[format.pattern_index()], wall_clock(&zoned(ts, None)))
}

/// The time alone, e.g. "3:05 PM".
pub fn format_short_timestamp(iso: &str, format: TimestampFormat) -> String {
    parse_timestamp_date(iso).map_or_else(String::new, |ts| format_time(ts, format, None))
}

/// A calendar day in a zone: its year and days since the epoch.
struct CalendarDay {
    year: i16,
    index: i32,
}

fn calendar_day(ts: Timestamp, zone: Option<&TimeZone>) -> CalendarDay {
    let date = zoned(ts, zone).date();
    CalendarDay {
        year: date.year(),
        index: days_from_civil(i32::from(date.year()), date.month() as u32, date.day() as u32),
    }
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i32;
    era * 146_097 + doe - 719_468
}

fn format_numeric_date(ts: Timestamp, same_year: bool, zone: Option<&TimeZone>) -> String {
    let data = timestamp_locale();
    let parts = if same_year { data.month_day } else { data.year_month_day };
    format_parts(parts, wall_clock(&zoned(ts, zone)))
}

/// The time today, "yesterday at …", or a numeric date and time. `zone`
/// overrides the system's current zone.
pub fn format_day_aware_timestamp(
    iso: &str,
    format: TimestampFormat,
    now_ms: i64,
    zone: Option<&TimeZone>,
) -> SharedString {
    let Some(ts) = parse_timestamp_date(iso) else {
        return SharedString::default();
    };
    let time = format_time(ts, format, zone);
    let today = calendar_day(Timestamp::from_millisecond(now_ms).unwrap_or(ts), zone);
    let day = calendar_day(ts, zone);
    match today.index - day.index {
        ..=0 => time.into(),
        1 => t!("yesterday at {time}", time = time),
        _ => format!("{} {time}", format_numeric_date(ts, day.year == today.year, zone)).into(),
    }
}

/// Like [`format_day_aware_timestamp`] for future times: "tomorrow at …".
pub fn format_upcoming_timestamp(
    iso: &str,
    format: TimestampFormat,
    now_ms: i64,
    zone: Option<&TimeZone>,
) -> SharedString {
    let Some(ts) = parse_timestamp_date(iso) else {
        return SharedString::default();
    };
    let time = format_time(ts, format, zone);
    let today = calendar_day(Timestamp::from_millisecond(now_ms).unwrap_or(ts), zone);
    let target = calendar_day(ts, zone);
    match target.index - today.index {
        ..0 => format_day_aware_timestamp(iso, format, now_ms, zone),
        0 => time.into(),
        1 => t!("tomorrow at {time}", time = time),
        _ => format!("{} {time}", format_numeric_date(ts, target.year == today.year, zone)).into(),
    }
}

/// `label` is the whole phrase ("5m ago"); `value` and `suffix` split it for styled layouts.
#[derive(Clone, Debug, PartialEq)]
pub struct RelativeTimeParts {
    pub value: SharedString,
    pub suffix: Option<SharedString>,
    pub label: SharedString,
}

/// What a relative-time view shows for an optional timestamp.
#[derive(Clone, Debug, PartialEq)]
pub enum RelativeTimeState {
    Missing,
    Invalid,
    Relative(RelativeTimeParts),
}

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

fn relative_time_unit_ms(elapsed_ms: i64) -> i64 {
    match elapsed_ms {
        ..HOUR_MS => MINUTE_MS,
        ..DAY_MS => HOUR_MS,
        _ => DAY_MS,
    }
}

/// Elapsed time floored to the relative label's precision; equal labels give equal values.
pub fn relative_time_elapsed_ms(iso: &str, now_ms: i64) -> Option<i64> {
    let ts = parse_timestamp_date(iso)?;
    let elapsed = (now_ms - ts.as_millisecond()).max(0);
    let unit = relative_time_unit_ms(elapsed);
    Some(elapsed.div_euclid(unit) * unit)
}

/// Milliseconds until the relative label for `iso` changes, capped at a day.
pub fn ms_until_relative_time_change(iso: &str, now_ms: i64) -> Option<i64> {
    let start = parse_timestamp_date(iso)?.as_millisecond();
    let elapsed = (now_ms - start).max(0);
    let unit = relative_time_unit_ms(elapsed);
    let next_change = start + (elapsed.div_euclid(unit) + 1) * unit;
    Some((next_change - now_ms).min(DAY_MS))
}

fn seconds_label(count: i64) -> SharedString {
    t!("{count}s", context = "seconds, abbreviated", count = count)
}

fn minutes_label(count: i64) -> SharedString {
    t!("{count}m", context = "minutes, abbreviated", count = count)
}

fn hours_label(count: i64) -> SharedString {
    t!("{count}h", context = "hours, abbreviated", count = count)
}

fn days_label(count: i64) -> SharedString {
    t!("{count}d", context = "days, abbreviated", count = count)
}

fn relative_elapsed_label(elapsed_ms: i64) -> SharedString {
    let seconds = elapsed_ms.div_euclid(1000);
    if seconds < 60 {
        return t!("just now");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return t!("{count}m ago", count = minutes);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return t!("{count}h ago", count = hours);
    }
    t!("{count}d ago", count = hours / 24)
}

/// "5m ago" split into value and suffix.
pub fn format_relative_elapsed(elapsed_ms: i64) -> RelativeTimeParts {
    let label = relative_elapsed_label(elapsed_ms);
    let seconds = elapsed_ms.div_euclid(1000);
    if seconds < 60 {
        return RelativeTimeParts {
            value: label.clone(),
            suffix: None,
            label,
        };
    }
    let ago = Some(t!("ago", context = "follows an elapsed time such as 5m"));
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let value = match () {
        _ if minutes < 60 => minutes_label(minutes),
        _ if hours < 24 => hours_label(hours),
        _ => days_label(hours / 24),
    };
    RelativeTimeParts {
        value,
        suffix: ago,
        label,
    }
}

/// Relative parts for a timestamp, or `None` when it does not parse.
pub fn format_relative_time(iso: &str, now_ms: i64) -> Option<RelativeTimeParts> {
    relative_time_elapsed_ms(iso, now_ms).map(format_relative_elapsed)
}

/// The whole relative phrase, or empty when the timestamp does not parse.
pub fn format_relative_time_label(iso: &str, now_ms: i64) -> SharedString {
    relative_time_elapsed_ms(iso, now_ms).map_or_else(SharedString::default, relative_elapsed_label)
}

/// "12s", "5m", "3h" or "2d" since `iso`; "just now" under five seconds.
pub fn format_elapsed_duration_label(iso: &str, now_ms: i64) -> SharedString {
    let Some(ts) = parse_timestamp_date(iso) else {
        return SharedString::default();
    };
    let diff = now_ms - ts.as_millisecond();
    if diff <= 0 {
        return t!("just now");
    }
    let seconds = diff / 1000;
    if seconds < 5 {
        return t!("just now");
    }
    if seconds < 60 {
        return seconds_label(seconds);
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return minutes_label(minutes);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return hours_label(hours);
    }
    days_label(hours / 24)
}

/// "Expires in 2d 3h 5m" until `iso`; "Expired" after it. Pass the parent's `now_ms`.
pub fn format_expires_in_label(iso: &str, now_ms: i64) -> SharedString {
    let Some(ts) = parse_timestamp_date(iso) else {
        return SharedString::default();
    };
    let diff = ts.as_millisecond() - now_ms;
    if diff <= 0 {
        return t!("Expired");
    }
    let total = diff / 1000;
    if total < 5 {
        return t!("Expires in a moment");
    }
    let mut parts = Vec::with_capacity(4);
    let (days, rem) = (total / 86_400, total % 86_400);
    let (hours, rem) = (rem / 3600, rem % 3600);
    let (minutes, seconds) = (rem / 60, rem % 60);
    if total >= 86_400 {
        parts.push(days_label(days));
    }
    if total >= 3600 && hours > 0 {
        parts.push(hours_label(hours));
    }
    if total >= 60 && (minutes > 0 || total < 3600) {
        parts.push(minutes_label(minutes));
    }
    if seconds > 0 || total < 60 {
        parts.push(seconds_label(seconds));
    }
    let duration = parts.iter().map(SharedString::as_str).collect::<Vec<_>>().join(" ");
    t!("Expires in {duration}", duration = duration)
}
