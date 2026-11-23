//! Prints the usage chart's date labels for every UI locale, hour style and a
//! few zones (including a daylight-saving fall-back hour) as JSON lines, to
//! diff against `formatDayShort`/`formatHourShort`/`formatDateTimeShort` in
//! TypeScript. `cargo run -p t3code-web --example usage_dates`

use t3code_web::components::usage::usage_format::{format_date_time_short, format_day_short, format_hour_short, usage_zone};
use t3code_web::i18n::locales::Locale;

const INSTANTS: [&str; 6] = [
    "2026-11-20T15:00:00.000Z",
    "2026-11-20T03:00:00.000Z",
    "2026-01-05T00:00:00.000Z",
    "2026-11-01T05:00:00.000Z",
    "2026-11-01T06:00:00.000Z",
    "2026-07-04T12:00:00.000Z",
];

fn main() {
    for locale in Locale::ALL {
        for day in ["2026-11-03", "2026-05-31", "2026-09-17"] {
            println!("{}", serde_json::json!(["day", locale.tag(), day, format_day_short(day, locale)]));
        }
        for zone in ["UTC", "America/New_York", "Asia/Tokyo"] {
            let tz = usage_zone(zone);
            for instant in INSTANTS {
                for hour12 in [true, false] {
                    println!("{}", serde_json::json!(["hour", locale.tag(), zone, instant, hour12, format_hour_short(instant, &tz, hour12, locale)]));
                    println!("{}", serde_json::json!(["datetime", locale.tag(), zone, instant, hour12, format_date_time_short(instant, &tz, hour12, locale)]));
                }
            }
        }
    }
}
