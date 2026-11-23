// Writes locale_data.rs: date, time, number, currency and list patterns read back
// from the JavaScript runtime's Intl (CLDR), so the Rust formatters print what the
// TypeScript app prints. Run with node from this directory:
//   node locale_data.mjs > locale_data.rs
// Add a tag to TAGS to support another system locale.

const TAGS = [
  // UI languages, as the catalogs name them.
  "en", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "sv", "zh",
  // Common system (regional) locales.
  "en-US", "en-GB", "en-CA", "en-AU", "en-IN", "en-IE", "en-NZ",
  "de-DE", "de-AT", "de-CH", "es-ES", "es-MX", "es-US", "es-AR",
  "fr-FR", "fr-CA", "fr-CH", "fr-BE", "ja-JP", "ko-KR", "pt", "pt-PT",
  "ru-RU", "sv-SE", "zh-CN", "zh-TW", "zh-HK", "it", "it-IT", "nl", "nl-NL", "pl", "pl-PL",
];

const at = (h, m = 5, s = 9) => new Date(Date.UTC(2026, 2, 7, h, m, s));

const rust = (s) =>
  JSON.stringify(s).replace(/[\u007f-￿]/g, (c) => `\\u{${c.charCodeAt(0).toString(16)}}`);

function parts(tag, options, date) {
  return new Intl.DateTimeFormat(tag, { ...options, timeZone: "UTC" }).formatToParts(date);
}

function hourStyle(tag, options) {
  const hour = (h) => parts(tag, options, at(h)).find((p) => p.type === "hour").value;
  const key = [hour(0), hour(9), hour(15)].join(",");
  const styles = {
    "0,9,15": "H23", "00,09,15": "H23Pad", "12,9,3": "H12", "12,09,03": "H12Pad",
    "0,9,3": "H11", "00,09,03": "H11Pad",
  };
  if (!styles[key]) throw new Error(`unknown hour style ${key} for ${tag}`);
  return styles[key];
}

// One Rust pattern from formatToParts. Field widths come from formatting 3/7 09:05:09.
function pattern(tag, options) {
  const sample = parts(tag, options, at(15));
  const narrow = parts(tag, options, new Date(Date.UTC(2026, 2, 7, 9, 5, 9)));
  const width = (type) => narrow.find((p) => p.type === type).value.length === 2;
  const out = sample.map((p) => {
    switch (p.type) {
      case "literal": return `Lit(${rust(p.value)})`;
      case "hour": return `Hour(HourStyle::${hourStyle(tag, options)})`;
      case "minute": return `Minute { two_digit: ${width("minute")} }`;
      case "second": return `Second { two_digit: ${width("second")} }`;
      case "day": return `Day { two_digit: ${width("day")} }`;
      case "year": return "Year";
      case "month":
        if (options.month === "long") {
          const names = Array.from({ length: 12 }, (_, m) =>
            parts(tag, options, new Date(Date.UTC(2026, m, 7))).find((x) => x.type === "month").value);
          return `MonthName(&[${names.map(rust).join(", ")}])`;
        }
        return `Month { two_digit: ${width("month")} }`;
      case "dayPeriod": {
        const am = parts(tag, options, at(0)).find((x) => x.type === "dayPeriod").value;
        return `DayPeriod { am: ${rust(am)}, pm: ${rust(p.value)} }`;
      }
      default: throw new Error(`unexpected part ${p.type} for ${tag}`);
    }
  });
  return `&[${out.join(", ")}]`;
}

function numberSymbols(tag) {
  const nf = new Intl.NumberFormat(tag);
  const p = nf.formatToParts(-1234567.891);
  const value = (type) => p.find((x) => x.type === type)?.value ?? "";
  const groups = p.filter((x) => x.type === "integer").map((x) => x.value.length);
  const minGrouping = nf.format(1234).length === 4 ? 2 : 1;
  const indian = groups.join(",") === "2,2,3";
  return `NumberSymbols { decimal: ${rust(value("decimal"))}, group: ${rust(value("group"))}, minus: ${rust(value("minusSign"))}, min_grouping: ${minGrouping}, indian: ${indian} }`;
}

function currency(tag) {
  const p = new Intl.NumberFormat(tag, {
    style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2,
  }).formatToParts(1234.5);
  const first = p.findIndex((x) => x.type === "integer");
  const last = p.findLastIndex((x) => x.type === "fraction");
  const text = (list) => list.map((x) => x.value).join("");
  return `CurrencyPattern { prefix: ${rust(text(p.slice(0, first)))}, suffix: ${rust(text(p.slice(last + 1)))} }`;
}

function list(tag) {
  const lf = new Intl.ListFormat(tag, { type: "conjunction" });
  const between = (items, a, b) => {
    const s = lf.format(items);
    return s.slice(s.indexOf(items[a]) + 1, s.indexOf(items[b]));
  };
  const e = ["", "", "", ""];
  return `ListPattern { two: ${rust(between(e.slice(0, 2), 0, 1))}, start: ${rust(between(e.slice(0, 3), 0, 1))}, middle: ${rust(between(e, 1, 2))}, end: ${rust(between(e.slice(0, 3), 1, 2))} }`;
}

const hm = { hour: "numeric", minute: "2-digit" };
const full = { year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric" };

const rows = TAGS.map((tag) => {
  const weekInfo = new Intl.Locale(tag).getWeekInfo();
  const hour12 = new Intl.DateTimeFormat(tag, { ...hm, timeZone: "UTC" }).resolvedOptions().hour12 ?? false;
  return `    LocaleData {
        tag: ${rust(tag)},
        number: ${numberSymbols(tag)},
        currency: ${currency(tag)},
        list: ${list(tag)},
        hour12: ${hour12},
        week_first_day: ${weekInfo.firstDay % 7},
        time: [${pattern(tag, hm)}, ${pattern(tag, { ...hm, hour12: true })}, ${pattern(tag, { ...hm, hour12: false })}],
        full: [${pattern(tag, full)}, ${pattern(tag, { ...full, hour12: true })}, ${pattern(tag, { ...full, hour12: false })}],
        month_day: ${pattern(tag, { month: "numeric", day: "numeric" })},
        year_month_day: ${pattern(tag, { month: "numeric", day: "numeric", year: "numeric" })},
        long_date: ${pattern(tag, { day: "numeric", month: "long", year: "numeric" })},
    },`;
});

console.log(`//! Generated by \`node locale_data.mjs > locale_data.rs\` with node ${process.version}
//! (ICU ${process.versions.icu}, CLDR ${process.versions.cldr}). Do not edit by hand.

use super::locale_format::{CurrencyPattern, HourStyle, ListPattern, LocaleData, NumberSymbols, Part::*};

/// Every locale the formatters know, by BCP 47 tag.
pub static LOCALES: &[LocaleData] = &[
${rows.join("\n")}
];`);
