//! `SnoozePresetsRow` (`SnoozePresetsSettings.tsx`): the snooze menu's
//! options, one editable row each ("In 1 hour", "Tomorrow at 09:00"), each
//! with a preview of its label and wake time, the menu as it reads now, and
//! "Add option". Every edit saves the whole list (`snoozePresets`).
//! Labels and wake times follow `resolveSnoozePreset` in `threadSettled.ts`.

use gpui_kit::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window, div, prelude::*, px};
use jiff::{ToSpan as _, ZonedDateTime as Zoned, civil::Time};
use t3code_contracts::{ClientServerSettingsPatch, SnoozePresetDay, SnoozePresetSetting, SnoozePresetSettingAfter, SnoozePresetSettingAt};

use super::server_settings_controls::{Fields, field_input, save_patch};
use super::server_settings_data::ServerPanelsModel;
use super::settings_layout::SettingsRow;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::number_field::NumberField;
use crate::components::ui::select::{Select, SelectSize};
use crate::i18n::t;

/// `MAX_SNOOZE_PRESETS`.
const MAX_PRESETS: usize = 8;
/// `MAX_SNOOZE_PRESET_MINUTES`.
const MAX_MINUTES: u64 = 30 * 24 * 60;

/// `DEFAULT_SNOOZE_PRESETS`.
pub fn default_presets() -> Vec<SnoozePresetSetting> {
    let at = |day, time: &str| SnoozePresetSetting::At(SnoozePresetSettingAt { day, time: time.into() });
    vec![
        SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes: 60 }),
        SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes: 180 }),
        at(SnoozePresetDay::Today, "18:00"),
        at(SnoozePresetDay::Tomorrow, "09:00"),
        at(SnoozePresetDay::Monday, "09:00"),
    ]
}

const DAYS: [SnoozePresetDay; 9] = [
    SnoozePresetDay::Today,
    SnoozePresetDay::Tomorrow,
    SnoozePresetDay::Monday,
    SnoozePresetDay::Tuesday,
    SnoozePresetDay::Wednesday,
    SnoozePresetDay::Thursday,
    SnoozePresetDay::Friday,
    SnoozePresetDay::Saturday,
    SnoozePresetDay::Sunday,
];

/// Sunday is 0, as `Date.getDay`.
fn weekday_index(day: SnoozePresetDay) -> Option<i64> {
    Some(match day {
        SnoozePresetDay::Sunday => 0,
        SnoozePresetDay::Monday => 1,
        SnoozePresetDay::Tuesday => 2,
        SnoozePresetDay::Wednesday => 3,
        SnoozePresetDay::Thursday => 4,
        SnoozePresetDay::Friday => 5,
        SnoozePresetDay::Saturday => 6,
        _ => return None,
    })
}

fn day_label(day: SnoozePresetDay) -> SharedString {
    match day {
        SnoozePresetDay::Today => t!("Today"),
        SnoozePresetDay::Tomorrow => t!("Tomorrow"),
        SnoozePresetDay::Monday => t!("Monday"),
        SnoozePresetDay::Tuesday => t!("Tuesday"),
        SnoozePresetDay::Wednesday => t!("Wednesday"),
        SnoozePresetDay::Thursday => t!("Thursday"),
        SnoozePresetDay::Friday => t!("Friday"),
        SnoozePresetDay::Saturday => t!("Saturday"),
        SnoozePresetDay::Sunday => t!("Sunday"),
    }
}

fn next_day_label(day: SnoozePresetDay) -> SharedString {
    match day {
        SnoozePresetDay::Monday => t!("Next Monday"),
        SnoozePresetDay::Tuesday => t!("Next Tuesday"),
        SnoozePresetDay::Wednesday => t!("Next Wednesday"),
        SnoozePresetDay::Thursday => t!("Next Thursday"),
        SnoozePresetDay::Friday => t!("Next Friday"),
        SnoozePresetDay::Saturday => t!("Next Saturday"),
        SnoozePresetDay::Sunday => t!("Next Sunday"),
        other => day_label(other),
    }
}

/// `snoozeDurationLabel`: the largest unit that divides evenly.
fn duration_label(minutes: u64) -> SharedString {
    let one = |n: u64, a: SharedString, b: SharedString| if n == 1 { a } else { b };
    if minutes % (24 * 60) == 0 {
        let n = minutes / (24 * 60);
        one(n, t!("In {count} day", count = n), t!("In {count} days", count = n))
    } else if minutes % 60 == 0 {
        let n = minutes / 60;
        one(n, t!("In {count} hour", count = n), t!("In {count} hours", count = n))
    } else {
        one(minutes, t!("In {count} minute", count = minutes), t!("In {count} minutes", count = minutes))
    }
}

/// A resolved option: its menu label, the wake time as shown, and when it wakes.
pub struct Resolved {
    pub label: SharedString,
    pub when: String,
    pub wake: Zoned,
}

/// "6:00 PM" on a 12-hour clock, "18:00" on a 24-hour one (`formatShortTimestamp`).
fn time_of_day(zoned: &Zoned, hour12: bool) -> String {
    let (hour, minute) = (zoned.hour(), zoned.minute());
    if !hour12 {
        return format!("{hour:02}:{minute:02}");
    }
    let (h12, suffix) = twelve_hour(hour);
    format!("{h12}:{minute:02} {suffix}")
}

fn twelve_hour(hour: i8) -> (i8, &'static str) {
    match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    }
}

/// A stored "HH:MM" as a native time input shows it: "06:00 PM" on a
/// 12-hour clock, "18:00" on a 24-hour one.
fn time_field_text(time: &str, hour12: bool) -> String {
    match parse_time(time) {
        Some((hour, minute)) if hour12 => {
            let (h12, suffix) = twelve_hour(hour as i8);
            format!("{h12:02}:{minute:02} {suffix}")
        }
        _ => time.to_owned(),
    }
}

/// Reads "18:00", "6:00 PM" or "6pm" back to (hour, minute).
fn parse_time(text: &str) -> Option<(u8, u8)> {
    let text = text.trim().to_ascii_lowercase();
    let (clock, meridiem) = match text.strip_suffix("am").or_else(|| text.strip_suffix("a")) {
        Some(rest) => (rest.trim().to_owned(), Some(false)),
        None => match text.strip_suffix("pm").or_else(|| text.strip_suffix("p")) {
            Some(rest) => (rest.trim().to_owned(), Some(true)),
            None => (text, None),
        },
    };
    let (h, m) = clock.split_once(':').unwrap_or((clock.as_str(), "0"));
    let (mut hour, minute): (u8, u8) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    if let Some(pm) = meridiem {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// `resolveSnoozePreset` on a 12-hour clock: `None` for a "Today" option whose time has passed.
pub fn resolve(setting: &SnoozePresetSetting, now: &Zoned) -> Option<Resolved> {
    resolve_with(setting, now, true)
}

/// [`resolve`] with times on a 12- or 24-hour clock.
pub fn resolve_with(setting: &SnoozePresetSetting, now: &Zoned, hour12: bool) -> Option<Resolved> {
    let (wake, days_ahead) = match setting {
        SnoozePresetSetting::After(a) => (now.checked_add((a.minutes as i64).minutes()).ok()?, 0),
        SnoozePresetSetting::At(at) => {
            let days = match at.day {
                SnoozePresetDay::Today => 0,
                SnoozePresetDay::Tomorrow => 1,
                day => {
                    let target = weekday_index(day)?;
                    let today = i64::from(now.weekday().to_sunday_zero_offset());
                    match (target - today + 7) % 7 {
                        0 => 7,
                        n => n,
                    }
                }
            };
            let (h, m) = at.time.split_once(':')?;
            let time = Time::new(h.parse().ok()?, m.parse().ok()?, 0, 0).ok()?;
            let day = now.checked_add(days.days()).ok()?;
            let wake = day.with().time(time).build().ok()?;
            if wake <= *now {
                return None;
            }
            (wake, days)
        }
        SnoozePresetSetting::Unknown(_) => return None,
    };
    let label = match setting {
        SnoozePresetSetting::After(a) => duration_label(a.minutes),
        SnoozePresetSetting::At(at) if days_ahead == 0 => {
            let hour: u8 = at.time[..2].parse().unwrap_or(0);
            if hour < 12 {
                t!("This morning")
            } else if hour < 17 {
                t!("This afternoon")
            } else {
                t!("This evening")
            }
        }
        SnoozePresetSetting::At(_) if days_ahead == 1 => t!("Tomorrow"),
        SnoozePresetSetting::At(at) if at.day == SnoozePresetDay::Monday => t!("Next week"),
        SnoozePresetSetting::At(at) if days_ahead == 7 => next_day_label(at.day),
        SnoozePresetSetting::At(at) => day_label(at.day),
        SnoozePresetSetting::Unknown(_) => SharedString::default(),
    };
    let names_day = matches!(setting, SnoozePresetSetting::At(at) if !(days_ahead > 1 && at.day == SnoozePresetDay::Monday));
    let time = time_of_day(&wake, hour12);
    let when = if names_day || wake.date() == now.date() { time } else { format!("{} {time}", wake.strftime("%a")) };
    Some(Resolved { label, when, wake })
}

fn same(a: &SnoozePresetSetting, b: &SnoozePresetSetting) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// The Snooze options row, bound to the model's `snoozePresets`.
pub fn snooze_presets_row<V: 'static>(
    model: &Entity<ServerPanelsModel>,
    fields: &mut Fields,
    read_only: bool,
    window: &mut Window,
    cx: &mut Context<V>,
) -> SettingsRow {
    let theme = &cx.app_theme().clone();
    let c = &theme.colors;
    let presets = model.read(cx).snapshot().settings.snooze_presets.clone().unwrap_or_else(default_presets);
    let now = Zoned::now();
    let hour12 = crate::timestamp_format::resolve_hour12(crate::hooks::settings_bridge::timestamp_format(cx));
    let save = {
        let model = model.clone();
        move |next: Vec<SnoozePresetSetting>, cx: &mut App| {
            save_patch(&model, "snoozePresets", ClientServerSettingsPatch { snooze_presets: Some(next), ..Default::default() }, cx);
        }
    };
    let mut rows = div().flex().flex_col().gap(theme.sp(1.5)).pt(theme.sp(3.)).pb(theme.sp(2.));
    for (ix, preset) in presets.iter().enumerate() {
        let after = matches!(preset, SnoozePresetSetting::After(_));
        let mut when_options = vec![t!("In", context = "snooze for a duration, as in: In 2 hours")];
        when_options.extend(DAYS.iter().map(|d| day_label(*d)));
        let when_ix = match preset {
            SnoozePresetSetting::At(at) => DAYS.iter().position(|d| *d == at.day).map_or(0, |p| p + 1),
            _ => 0,
        };
        let (list, save_when) = (presets.clone(), save.clone());
        let when = div().w(theme.sp(36.)).flex_none().child(
            Select::new(SharedString::from(format!("snooze-when-{ix}")), when_options, when_ix, move |pick, _, cx| {
                let mut next = list.clone();
                next[ix] = if pick == 0 {
                    SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes: 60 })
                } else {
                    let time = match &list[ix] {
                        SnoozePresetSetting::At(at) => at.time.clone(),
                        _ => "09:00".into(),
                    };
                    SnoozePresetSetting::At(SnoozePresetSettingAt { day: DAYS[pick - 1], time })
                };
                save_when(next, cx);
            })
            .size(SelectSize::Sm),
        );
        let editor: AnyElement = match preset {
            SnoozePresetSetting::After(a) => {
                let (amount, unit) = if a.minutes % (24 * 60) == 0 {
                    (a.minutes / (24 * 60), 2)
                } else if a.minutes % 60 == 0 {
                    (a.minutes / 60, 1)
                } else {
                    (a.minutes, 0)
                };
                let factor = [1u64, 60, 24 * 60];
                let (list, save_amount) = (presets.clone(), save.clone());
                let max_amount = MAX_MINUTES / factor[unit];
                let (list, save_amount) = (presets.clone(), save.clone());
                let commit = move |n: u64, cx: &mut App| {
                    let mut next = list.clone();
                    next[ix] = SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes: (n.clamp(1, max_amount) * factor[unit]).min(MAX_MINUTES) });
                    save_amount(next, cx);
                };
                let typed = commit.clone();
                let field = fields.draft(format!("snooze-amount-{ix}"), amount.to_string(), "", false, window, cx, move |text, _, cx| {
                    if let Ok(n) = text.trim().parse::<f64>() {
                        typed(n.round().max(1.) as u64, cx);
                    }
                });
                let stepper = div().w(theme.sp(32.)).flex_none().child(
                    NumberField::new(SharedString::from(format!("snooze-amount-{ix}")), &field, amount, 1, max_amount)
                        .disabled(read_only)
                        .on_step(move |n, _, cx| commit(n, cx)),
                );
                let units = vec![
                    if amount == 1 { t!("minute") } else { t!("minutes") },
                    if amount == 1 { t!("hour") } else { t!("hours") },
                    if amount == 1 { t!("day") } else { t!("days") },
                ];
                let (list, save_unit) = (presets.clone(), save.clone());
                let unit_select = div().w(theme.sp(36.)).flex_none().child(
                    Select::new(SharedString::from(format!("snooze-unit-{ix}")), units, unit, move |pick, _, cx| {
                        let mut next = list.clone();
                        next[ix] = SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes: (amount * factor[pick]).min(MAX_MINUTES) });
                        save_unit(next, cx);
                    })
                    .size(SelectSize::Sm),
                );
                div().flex().items_center().gap(theme.sp(2.)).child(stepper).child(unit_select).into_any_element()
            }
            SnoozePresetSetting::At(at) => {
                let (list, save_time, day) = (presets.clone(), save.clone(), at.day);
                let field = fields.draft(format!("snooze-time-{ix}"), time_field_text(&at.time, hour12), "", false, window, cx, move |text, _, cx| {
                    if let Some((hour, minute)) = parse_time(&text) {
                        let mut next = list.clone();
                        next[ix] = SnoozePresetSetting::At(SnoozePresetSettingAt { day, time: format!("{hour:02}:{minute:02}") });
                        save_time(next, cx);
                    }
                });
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t!("at", context = "before a time of day, as in: Monday at 09:00")))
                    .child(div().w(theme.sp(32.)).child(field_input(&field, false, false, read_only).trailing_icon(IconName::Clock)))
                    .into_any_element()
            }
            SnoozePresetSetting::Unknown(_) => div().into_any_element(),
        };
        let resolved = resolve_with(preset, &now, hour12);
        let earlier = &presets[..ix];
        let preview: AnyElement = match &resolved {
            None => div().child(t!("Passed for today")).into_any_element(),
            Some(_) if earlier.iter().any(|p| same(p, preset)) => div().child(t!("Duplicate of an option above")).into_any_element(),
            Some(r) if earlier.iter().filter_map(|p| resolve_with(p, &now, hour12)).any(|o| o.wake.timestamp().as_second() / 60 == r.wake.timestamp().as_second() / 60) => {
                div().child(t!("Same time as an option above")).into_any_element()
            }
            Some(r) => div()
                .flex()
                .child(div().text_color(c.foreground()).child(r.label.clone()))
                .child(div().pl(theme.sp(2.)).child(r.when.clone()))
                .into_any_element(),
        };
        let (list, save_remove) = (presets.clone(), save.clone());
        rows = rows.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.sp(2.))
                .when(!after, |d| d)
                .child(when)
                .child(editor)
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(theme.sp(2.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(preview)
                        .child(
                            Button::new(SharedString::from(format!("snooze-remove-{ix}")))
                                .variant(ButtonVariant::GhostMuted)
                                .size(ButtonSize::IconSm)
                                .icon(IconName::X)
                                .disabled(read_only)
                                .on_click(move |_, _, cx| {
                                    let mut next = list.clone();
                                    next.remove(ix);
                                    save_remove(next, cx);
                                }),
                        ),
                ),
        );
    }
    // The menu as it reads now: soonest first, one entry per wake minute.
    let mut menu: Vec<Resolved> = presets.iter().filter_map(|p| resolve_with(p, &now, hour12)).collect();
    menu.sort_by_key(|r| r.wake.timestamp().as_second() / 60);
    menu.dedup_by_key(|r| r.wake.timestamp().as_second() / 60);
    let menu_line: SharedString = if menu.is_empty() {
        t!("Menus offer only Custom…")
    } else {
        t!("Menu now: {options}", options = menu.iter().map(|r| r.label.to_string()).collect::<Vec<_>>().join(" · "))
    };
    let (list, save_add) = (presets.clone(), save.clone());
    rows = rows
        .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(menu_line))
        .child(
            div().flex().pt(px(4.)).child(
                Button::new("snooze-add")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Compact)
                    .icon(IconName::Plus)
                    .label(t!("Add option"))
                    .disabled(read_only || presets.len() >= MAX_PRESETS)
                    .on_click(move |_, _, cx| {
                        let mut next = list.clone();
                        next.push(SnoozePresetSetting::After(SnoozePresetSettingAfter { minutes: 30 }));
                        save_add(next, cx);
                    }),
            ),
        );
    SettingsRow::new(t!("Snooze options"))
        .description(t!("Choices in every snooze menu, listed soonest first. Weekdays mean the next one after today."))
        .child(rows)
}
