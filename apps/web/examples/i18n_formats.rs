//! Runs the Rust formatters over a case table written by the i18n verification
//! harness and prints one JSON line per case, for comparison with the TypeScript
//! originals run over the same table.
//!
//!   i18n_formats <cases.json> <system locale tag>

use jiff::tz::TimeZone;
use serde::Deserialize;
use serde_json::{Value, json};
use t3code_web::{
    i18n::{
        Arg, Locale, duration_format::format_duration, list_format::format_conjunction_list,
        messages::format_message, number_format, runtime::activate_locale_blocking,
    },
    text_direction::{TextDirection, has_right_to_left_letter, markdown_prose_direction, text_direction},
    timestamp_format::{self as ts, TimestampFormat},
};

#[derive(Deserialize)]
struct Case {
    id: usize,
    #[serde(rename = "fn")]
    function: String,
    args: Vec<Value>,
    ui: String,
}

fn format(value: &Value) -> TimestampFormat {
    match value.as_str() {
        Some("12-hour") => TimestampFormat::TwelveHour,
        Some("24-hour") => TimestampFormat::TwentyFourHour,
        _ => TimestampFormat::Locale,
    }
}

fn direction(direction: Option<TextDirection>) -> Value {
    match direction {
        Some(TextDirection::Ltr) => json!("ltr"),
        Some(TextDirection::Rtl) => json!("rtl"),
        None => Value::Null,
    }
}

fn run(case: &Case) -> Value {
    let a = &case.args;
    let str_arg = |i: usize| a[i].as_str().unwrap_or_default();
    let num_arg = |i: usize| a[i].as_f64().unwrap_or(f64::NAN);
    let int_arg = |i: usize| a[i].as_i64().unwrap_or_default();
    let zone = |i: usize| a[i].as_str().map(|name| TimeZone::get(name).unwrap());
    match case.function.as_str() {
        "formatShortTimestamp" => json!(ts::format_short_timestamp(str_arg(0), format(&a[1]))),
        "formatChatTimestampTooltip" => json!(ts::format_chat_timestamp_tooltip(str_arg(0), format(&a[1])).as_str()),
        "formatFullTimestamp" => json!(ts::format_full_timestamp(str_arg(0), format(&a[1]))),
        "formatDayAwareTimestamp" => {
            json!(ts::format_day_aware_timestamp(str_arg(0), format(&a[1]), int_arg(2), zone(3).as_ref()).as_str())
        }
        "formatUpcomingTimestamp" => {
            json!(ts::format_upcoming_timestamp(str_arg(0), format(&a[1]), int_arg(2), zone(3).as_ref()).as_str())
        }
        "formatRelativeTime" => match ts::format_relative_time(str_arg(0), int_arg(1)) {
            Some(p) => json!({ "value": p.value.as_str(), "suffix": p.suffix.as_ref().map(|s| s.as_str()), "label": p.label.as_str() }),
            None => Value::Null,
        },
        "formatRelativeTimeLabel" => json!(ts::format_relative_time_label(str_arg(0), int_arg(1)).as_str()),
        "formatElapsedDurationLabel" => json!(ts::format_elapsed_duration_label(str_arg(0), int_arg(1)).as_str()),
        "formatExpiresInLabel" => json!(ts::format_expires_in_label(str_arg(0), int_arg(1)).as_str()),
        "msUntilRelativeTimeChange" => json!(ts::ms_until_relative_time_change(str_arg(0), int_arg(1))),
        "relativeTimeElapsedMs" => json!(ts::relative_time_elapsed_ms(str_arg(0), int_arg(1))),
        "resolveHour12" => json!(ts::resolve_hour12(format(&a[0]))),
        "weekStartsOn" => json!(ts::week_starts_on()),
        "formatDuration" => json!(format_duration(num_arg(0)).as_str()),
        "formatConjunctionList" => {
            let items: Vec<&str> = a[0].as_array().unwrap().iter().filter_map(Value::as_str).collect();
            json!(format_conjunction_list(&items, str_arg(1)))
        }
        "formatCount" => json!(number_format::format_count(num_arg(0), Some(str_arg(1)))),
        "formatUsd" => json!(number_format::format_usd(num_arg(0), Some(str_arg(1)))),
        "formatTokens" => json!(number_format::format_tokens(num_arg(0))),
        "formatPercent" => json!(number_format::format_percent(num_arg(0), int_arg(1) as usize)),
        "icuPlural" => {
            let locale = Locale::from_tag(&case.ui).unwrap();
            json!(format_message(str_arg(0), &[("n", Arg::Num(num_arg(1)))], locale).unwrap())
        }
        "textDirection" => direction(text_direction(str_arg(0))),
        "markdownProseDirection" => direction(markdown_prose_direction(str_arg(0))),
        "hasRightToLeftLetter" => json!(has_right_to_left_letter(str_arg(0))),
        other => panic!("unknown case function {other}"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cases_path = args.next().expect("cases.json path");
    let system_locale = args.next().expect("system locale tag");
    ts::set_timestamp_locale(Some(&system_locale));
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(cases_path).unwrap()).unwrap();
    let mut ui = String::new();
    for case in &cases {
        if case.ui != ui {
            ui = case.ui.clone();
            activate_locale_blocking(Locale::from_tag(&ui).unwrap());
        }
        println!("{}", json!({ "id": case.id, "got": run(case) }));
    }
}
