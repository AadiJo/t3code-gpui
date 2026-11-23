//! Translation fixture: a language picker over strings, a plural, a `<T>` block with
//! code spans, a live relative time and regional formats. Clicking a language
//! switches the whole window at runtime.

use gpui_kit::{TestSupportExt as _, prelude::FluentBuilder as _, *};

use crate::{
    ActiveAppTheme as _,
    components::relative_time::RelativeTime,
    i18n::{
        Locale, RichText, active_locale, duration_format::format_duration, list_format::format_conjunction_list,
        number_format::format_count, runtime::apply_language_setting, t, t_rich,
    },
    timestamp_format::{TimestampFormat, format_chat_timestamp_tooltip, now_ms},
};

pub fn build(_: &mut Window, cx: &mut App) -> AnyView {
    cx.new(I18nScene::new).into()
}

struct I18nScene {
    relative: Entity<RelativeTime>,
    sent_at: SharedString,
}

impl I18nScene {
    fn new(cx: &mut Context<Self>) -> Self {
        let ms = now_ms() - 5 * 60_000;
        let sent_at: SharedString = jiff::Timestamp::from_millisecond(ms).unwrap().to_string().into();
        Self {
            relative: cx.new(|cx| RelativeTime::new(sent_at.clone(), cx)),
            sent_at,
        }
    }
}

fn rich(text: RichText, code: Hsla, code_bg: Hsla) -> StyledText {
    text.styled(|tag| {
        (tag == "code").then(|| HighlightStyle {
            color: Some(code),
            background_color: Some(code_bg),
            ..Default::default()
        })
    })
}

impl Render for I18nScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let active = active_locale();
        let picker = div().flex().flex_wrap().gap_2().children(Locale::ALL.map(|locale| {
            div()
                .id(SharedString::from(format!("lang-{}", locale.tag())))
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .when(locale == active, |d| d.bg(colors.accent).text_color(colors.accent_foreground))
                .child(locale.native_name())
                .on_click(move |_, _, cx| apply_language_setting(locale.tag(), cx))
                .test_support()
        }));
        let row = |label: SharedString, value: AnyElement| {
            div()
                .flex()
                .gap_4()
                .child(div().flex_none().w(px(180.)).text_color(colors.text_muted).child(label))
                .child(div().flex_1().min_w_0().child(value))
        };
        div()
            .size_full()
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .bg(colors.canvas)
            .text_color(colors.text)
            .text_sm()
            .child(picker)
            .child(row("t!".into(), t!("Open settings").into_any_element()))
            .child(row("context".into(), t!("Open", context = "verb, opens a URL").into_any_element()))
            .child(row(
                "plural (1, 4)".into(),
                div()
                    .child(t!("{count, plural, one {Updated {count} thread} other {Updated {count} threads}}", count = 1))
                    .child(t!("{count, plural, one {Updated {count} thread} other {Updated {count} threads}}", count = 4))
                    .into_any_element(),
            ))
            .child(row(
                "<T> with code".into(),
                rich(
                    t_rich!("Press a shortcut. Use <code>Backspace</code> to clear. Shortcuts are environment-wide. Projects using the same action share its shortcut."),
                    colors.accent,
                    colors.surface_raised,
                )
                .into_any_element(),
            ))
            .child(row("<RelativeTime>".into(), self.relative.clone().into_any_element()))
            .child(row(
                "timestamp tooltip".into(),
                format_chat_timestamp_tooltip(&self.sent_at, TimestampFormat::Locale).into_any_element(),
            ))
            .child(row(
                "list, count, duration".into(),
                format!(
                    "{} · {} · {}",
                    format_conjunction_list(&["Codex", "Claude", "Cursor"], active.tag()),
                    format_count(1_234_567.0, Some(active.tag())),
                    format_duration(3_661_000.0)
                )
                .into_any_element(),
            ))
    }
}
