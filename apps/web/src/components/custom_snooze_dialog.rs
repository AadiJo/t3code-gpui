//! "Snooze → Custom…". Ports `CustomSnoozeDialog.tsx`: a date and time, or a duration, resolved
//! to a wake time with `resolveCustomSnooze`, and the `ui/calendar.tsx` month grid it opens.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use jiff::ZonedDateTime as Zoned;
use jiff::civil::{Date, Time};

use super::icons::{Icon, IconName, IconSize};
use super::sidebar::model::ThreadKey;
use super::ui::button::{Button, ButtonSize, ButtonVariant};
use super::ui::dialog::{DialogSurface, Modal};
use super::ui::input::TextInput;
use super::ui::number_field::NumberField;
use super::ui::popover::{below, glass_surface};
use super::ui::select::Select;
use super::ui::toggle::{Toggle, ToggleSize, ToggleVariant};
use super::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// What the dialog decided.
#[derive(Clone, Debug)]
pub enum CustomSnoozeEvent {
    /// Snooze `threads` until the time.
    Snooze { threads: Vec<ThreadKey>, until: Zoned },
    /// "Edit snooze options": Settings → Threads.
    EditOptions,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Date,
    Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Minutes,
    Hours,
    Days,
}

const UNITS: [Unit; 3] = [Unit::Minutes, Unit::Hours, Unit::Days];

/// The custom snooze dialog. Render it while open; it emits [`CustomSnoozeEvent`].
pub struct CustomSnoozeDialog {
    threads: Vec<ThreadKey>,
    mode: Mode,
    date: Date,
    /// The month the calendar shows, when open.
    calendar: Option<Date>,
    time: Entity<InputState>,
    amount: Entity<InputState>,
    unit: Unit,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CustomSnoozeEvent> for CustomSnoozeDialog {}

/// `localSnoozeTime` as the time field shows it: `03:13 AM` on a 12-hour clock, else `15:13`.
fn format_time(time: Time, hour12: bool) -> String {
    if hour12 {
        let hour = match time.hour() % 12 {
            0 => 12,
            h => h,
        };
        format!("{hour:02}:{:02} {}", time.minute(), if time.hour() < 12 { "AM" } else { "PM" })
    } else {
        format!("{:02}:{:02}", time.hour(), time.minute())
    }
}

/// Reads the time field: `HH:MM`, or `h:mm AM`/`PM`.
fn parse_time(text: &str) -> Option<Time> {
    let text = text.trim().to_ascii_uppercase();
    let (clock, meridiem) = match text.strip_suffix("AM").or_else(|| text.strip_suffix("PM")) {
        Some(clock) => (clock.trim(), Some(text.ends_with("PM"))),
        None => (text.as_str(), None),
    };
    let (hour, minute) = clock.split_once(':')?;
    let (mut hour, minute): (i8, i8) = (hour.trim().parse().ok()?, minute.trim().parse().ok()?);
    if let Some(pm) = meridiem {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    Time::new(hour, minute, 0, 0).ok()
}

impl CustomSnoozeDialog {
    /// Opens on an hour from now (`new Date(Date.now() + 3_600_000)`) and a 2-hour duration.
    pub fn new(threads: Vec<ThreadKey>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = Zoned::now().checked_add(jiff::Span::new().hours(1)).unwrap_or_else(|_| Zoned::now());
        let hour12 = crate::timestamp_format::resolve_hour12(crate::timestamp_format::TimestampFormat::Locale);
        let time_text = format_time(initial.time(), hour12);
        let time = cx.new(|cx| InputState::new(window, cx).default_value(time_text));
        let amount = cx.new(|cx| InputState::new(window, cx).default_value("2"));
        let clear = |this: &mut Self, _: &Entity<InputState>, event: &InputEvent, _: &mut Window, cx: &mut Context<Self>| {
            if matches!(event, InputEvent::Change) {
                this.error = None;
                cx.notify();
            }
        };
        let subscriptions = vec![cx.subscribe_in(&time, window, clear), cx.subscribe_in(&amount, window, clear)];
        Self { threads, mode: Mode::Date, date: initial.date(), calendar: None, time, amount, unit: Unit::Hours, error: None, _subscriptions: subscriptions }
    }

    /// `resolveCustomSnooze`: the wake time, or `None` when it is not a valid time in the future.
    fn resolve(&self, cx: &App) -> Option<Zoned> {
        let now = Zoned::now();
        let wake = match self.mode {
            Mode::Duration => {
                let amount: f64 = self.amount.read(cx).value().trim().parse().ok()?;
                if !amount.is_finite() || amount <= 0. {
                    return None;
                }
                let unit_ms = match self.unit {
                    Unit::Minutes => 60_000.,
                    Unit::Hours => 3_600_000.,
                    Unit::Days => 86_400_000.,
                };
                now.checked_add(jiff::SignedDuration::from_millis((amount * unit_ms) as i64)).ok()?
            }
            Mode::Date => {
                let time = parse_time(&self.time.read(cx).value())?;
                let wake = self.date.to_datetime(time).to_zoned_datetime(now.time_zone().clone()).ok()?;
                // A nonexistent local time (a DST gap) moves; the fork rejects it.
                if wake.date() != self.date || wake.time() != time {
                    return None;
                }
                wake
            }
        };
        (wake > now).then_some(wake)
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        match self.resolve(cx) {
            Some(until) => cx.emit(CustomSnoozeEvent::Snooze { threads: self.threads.clone(), until }),
            None => {
                self.error = Some(match self.mode {
                    Mode::Date => t!("Choose a valid date and time in the future."),
                    Mode::Duration => t!("Enter a positive duration."),
                });
                cx.notify();
            }
        }
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.error = None;
        self.calendar = None;
        cx.notify();
    }

    fn pick_date(&mut self, date: Date, cx: &mut Context<Self>) {
        self.date = date;
        self.calendar = None;
        self.error = None;
        cx.notify();
    }

    /// Opens or closes the calendar, as the date button does.
    pub fn toggle_calendar(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.calendar = if self.calendar.is_some() { None } else { Some(self.date) };
        cx.notify();
    }

    /// Switches to the Duration tab, for checks.
    pub fn show_duration(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(Mode::Duration, cx);
    }

    /// Types `amount` into "Snooze for", for checks.
    pub fn set_amount(&mut self, amount: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.amount.update(cx, |input, cx| input.set_value(amount.to_string(), window, cx));
    }

    /// Presses Snooze, for checks.
    pub fn submit_for_check(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    /// The validation message shown, if any.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    fn amount_value(&self, cx: &App) -> f64 {
        self.amount.read(cx).value().trim().parse::<f64>().ok().filter(|v| v.is_finite()).map_or(0., |v| v.max(0.))
    }

    /// The calendar popup (`Calendar` in a `PopoverPopup`): month caption between its arrows,
    /// weekday row, and the weeks the month touches, days before today struck through.
    fn render_calendar(&self, month: Date, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let cell = theme.sp(9.);
        let today = Zoned::now().date();
        let first = month.first_of_month();
        let start = first.checked_sub(jiff::Span::new().days(first.weekday().to_sunday_zero_offset() as i64)).unwrap_or(first);
        let days_shown = (first.weekday().to_sunday_zero_offset() as i64 + month.days_in_month() as i64 + 6) / 7 * 7;
        let muted = with_alpha(c.muted_foreground, 0.72);
        let nav = |id: &'static str, icon: IconName, delta: i64, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .size(cell)
                .items_center()
                .justify_center()
                .rounded(theme.radii.lg)
                .cursor_pointer()
                .hover(|s| s.bg(c.accent))
                .child(div().opacity(0.8).child(Icon::new(icon).size(IconSize::Custom(theme.sp(4.))).color(c.foreground())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(shown) = this.calendar {
                        this.calendar = shown.first_of_month().checked_add(jiff::Span::new().months(delta)).ok();
                        cx.notify();
                    }
                }))
        };
        let weekdays = [t!("Su"), t!("Mo"), t!("Tu"), t!("We"), t!("Th"), t!("Fr"), t!("Sa")];
        let caption = jiff::fmt::strtime::format("%B %Y", first).unwrap_or_default();
        let mut weeks = Vec::new();
        for week in 0..days_shown / 7 {
            let mut row = div().flex();
            for day in 0..7 {
                let Ok(date) = start.checked_add(jiff::Span::new().days(week * 7 + day)) else { continue };
                let outside = date.month() != month.month();
                let disabled = date < today;
                let selected = date == self.date;
                let is_today = date == today;
                let mut button = div()
                    .id(SharedString::from(format!("calendar-day-{date}")))
                    .relative()
                    .flex()
                    .size(cell)
                    .items_center()
                    .justify_center()
                    .rounded(theme.radii.lg)
                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                    .text_color(if selected {
                        c.primary_foreground()
                    } else if disabled || outside {
                        muted
                    } else {
                        c.foreground()
                    })
                    .when(selected, |d| d.bg(c.primary()))
                    // `disabled:opacity-64`, `in-data-disabled:line-through`.
                    .when(disabled, |d| d.line_through().opacity(0.64))
                    .child(date.day().to_string())
                    // `today`: a 3px dot 4px above the bottom, in the background colour when selected.
                    .when(is_today, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(theme.sp(1.))
                                .left(cell / 2. - px(1.5))
                                .size(px(3.))
                                .rounded_full()
                                .bg(if selected { c.background() } else if disabled { with_alpha(c.foreground(), 0.3) } else { c.primary() }),
                        )
                    });
                if !disabled {
                    button = button
                        .cursor_pointer()
                        .when(!selected, |d| d.hover(|s| s.bg(c.accent)))
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_date(date, cx)));
                }
                row = row.child(div().py(px(1.)).child(button));
            }
            weeks.push(row);
        }
        glass_surface(&theme)
            .id("snooze-calendar")
            .occlude()
            .rounded(theme.radii.xl)
            .p(theme.sp(2.))
            .child(
                div()
                    .relative()
                    .flex()
                    .h(cell)
                    .mb(theme.sp(1.))
                    .items_center()
                    .justify_between()
                    .child(nav("calendar-previous", IconName::ChevronLeft, -1, cx))
                    .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(caption))
                    .child(nav("calendar-next", IconName::ChevronRight, 1, cx)),
            )
            .child(div().flex().children(weekdays.into_iter().map(|day| {
                div().flex().size(cell).items_center().justify_center().ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(muted).child(day)
            })))
            .children(weeks)
    }
}

impl Render for CustomSnoozeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let weak = cx.weak_entity();
        let label = |text: SharedString| div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).line_height(theme.sp(4.)).text_color(c.foreground()).child(text);
        let field = |title: SharedString, control: gpui_kit::AnyElement| div().flex().flex_col().flex_1().min_w_0().gap(theme.sp(2.)).child(label(title)).child(control);
        // `ToggleGroup` (segmented): `gap-0.5 rounded-lg bg-input/40 p-0.5`, toggles sharing the width.
        let modes = div()
            .flex()
            .w_full()
            .gap(theme.sp(0.5))
            .p(theme.sp(0.5))
            .rounded(theme.radii.lg)
            .bg(with_alpha(c.input, 0.4))
            .children([(Mode::Date, t!("Date and time")), (Mode::Duration, t!("Duration"))].into_iter().map(|(mode, title)| {
                let weak = weak.clone();
                div().flex_1().child(
                    Toggle::new(SharedString::from(format!("snooze-mode-{mode:?}")))
                        .label(title)
                        .variant(ToggleVariant::Segmented)
                        .size(ToggleSize::Segmented)
                        .pressed(self.mode == mode)
                        .on_toggle(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.set_mode(mode, cx)).ok();
                        }),
                )
            }));
        let controls = match self.mode {
            Mode::Date => {
                let date_label = jiff::fmt::strtime::format("%b %-d, %Y", self.date).unwrap_or_default();
                let open = weak.clone();
                let date_button = Button::new("snooze-date")
                    .variant(ButtonVariant::Outline)
                    .label(date_label)
                    .accessory(div().ml_auto().child(Icon::new(IconName::Calendar).size(IconSize::Custom(theme.sp(4.))).color(c.muted_foreground)))
                    .on_click(move |_, window, cx| {
                        open.update(cx, |this, cx| this.toggle_calendar(window, cx)).ok();
                    });
                let calendar = self.calendar.map(|month| self.render_calendar(month, cx));
                div()
                    .flex()
                    .gap(theme.sp(3.))
                    .child(field(
                        t!("Date", context = "calendar date"),
                        div().relative().w_full().child(date_button).children(calendar.map(below)).into_any_element(),
                    ))
                    .child(field(t!("Time", context = "time of day"), TextInput::new(&self.time).trailing_icon(IconName::Clock).into_any_element()))
            }
            Mode::Duration => {
                let step = weak.clone();
                let units: Vec<SharedString> = vec![t!("Minutes"), t!("Hours"), t!("Days")];
                let pick = weak.clone();
                div()
                    .flex()
                    .gap(theme.sp(3.))
                    .child(field(
                        t!("Snooze for"),
                        NumberField::new("snooze-amount", &self.amount, self.amount_value(cx), 0., 100_000.)
                            .on_step(move |value, window, cx| {
                                step.update(cx, |this, cx| {
                                    this.amount.update(cx, |input, cx| input.set_value(super::ui::number_field::format_number(value), window, cx));
                                    this.error = None;
                                    cx.notify();
                                })
                                .ok();
                            })
                            .into_any_element(),
                    ))
                    .child(field(
                        t!("Unit", context = "unit of time"),
                        Select::new("snooze-unit", units, UNITS.iter().position(|u| *u == self.unit).unwrap_or(1), move |ix, _, cx| {
                            pick.update(cx, |this, cx| {
                                this.unit = UNITS[ix];
                                this.error = None;
                                cx.notify();
                            })
                            .ok();
                        })
                        .into_any_element(),
                    ))
            }
        };
        let (close, cancel, submit, edit) = (weak.clone(), weak.clone(), weak.clone(), weak.clone());
        let surface = DialogSurface::new(t!("Custom snooze"))
            .max_width_rem(28.)
            .description(t!("Choose when snoozed threads return to your inbox."))
            .on_close(move |_, cx| {
                close.update(cx, |_, cx| cx.emit(CustomSnoozeEvent::Closed)).ok();
            })
            .child(div().flex().flex_col().gap(theme.sp(4.)).child(modes).child(controls))
            .children(self.error.clone().map(|error| div().ui_text(&theme, TextSize::Base, weight::NORMAL).text_color(c.destructive()).child(error)))
            .footer(
                div().mr_auto().child(
                    Button::new("snooze-edit-options")
                        .variant(ButtonVariant::GhostMuted)
                        .icon(IconName::Settings2)
                        .label(t!("Edit snooze options"))
                        .on_click(move |_, _, cx| {
                            edit.update(cx, |_, cx| cx.emit(CustomSnoozeEvent::EditOptions)).ok();
                        }),
                ),
            )
            .footer(Button::new("snooze-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).on_click(move |_, _, cx| {
                cancel.update(cx, |_, cx| cx.emit(CustomSnoozeEvent::Closed)).ok();
            }))
            .footer(Button::new("snooze-submit").label(t!("Snooze", context = "verb")).on_click(move |_, _, cx| {
                submit.update(cx, |this, cx| this.submit(cx)).ok();
            }));
        let _ = (window, ButtonSize::Default, DISABLED_OPACITY);
        let backdrop = weak.clone();
        Modal::dialog("custom-snooze-dialog", div().id("custom-snooze-content").occlude().w_full().max_w(theme.rem(28.)).child(surface), move |_, cx| {
            backdrop
                .update(cx, |this, cx| {
                    // Escape or a press outside closes the calendar first, as its popover would.
                    if this.calendar.take().is_some() {
                        cx.notify();
                    } else {
                        cx.emit(CustomSnoozeEvent::Closed);
                    }
                })
                .ok();
        })
    }
}
