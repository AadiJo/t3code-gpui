//! Pieces shared by timeline rows: the work-log line (`WorkLog.tsx`), the
//! hover timestamp and the stepped shimmer (`TimelineRowParts.tsx`), tool
//! icons, and the small ghost buttons rows use.

use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::chat::chat_style::chat_icon_stroke;
use gpui_kit::{
    AnyElement, Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Stateful, Styled as _, StyledText, div, px,
};

use super::{
    chat_style::{ChatColors, TEXT_SM_RELAXED, TEXT_XS, alpha, sans},
    messages_timeline::SHIMMER_STEPS,
    work_log_presentation::format_clock,
};

/// The group name rows use for hover-revealed parts.
pub const ROW_GROUP: &str = "timeline-row";
/// `min-h-6`: a work-log line is at least 24px tall.
pub const WORK_LINE_MIN_HEIGHT: Pixels = px(24.);
/// The 24px icon slot and the 6px gap after it.
pub const WORK_ICON_SLOT: Pixels = px(24.);
pub const WORK_LINE_GAP: Pixels = px(6.);
/// Tool icons are 16px; chevrons and failure marks 12px.
pub const TOOL_ICON_SIZE: Pixels = px(16.);
pub const SMALL_ICON: Pixels = px(12.);
/// The shimmer band is 4.5rem wide.
const SHIMMER_BAND: Pixels = px(72.);
/// Average Inter advance at 14px, for placing the band by character.
const AVERAGE_CHAR_WIDTH: Pixels = px(7.2);

/// A work-log row container: `rounded-md px-0.5`, `py-0.5` unless grouped.
pub fn work_log_row(id: impl Into<ElementId>, grouped: bool) -> Stateful<Div> {
    div()
        .id(id)
        .group(ROW_GROUP)
        .relative()
        .w_full()
        .min_w_0()
        .rounded(px(6.))
        .px(px(2.))
        .py(if grouped { px(0.) } else { px(2.) })
}

/// `WorkLogLine`: icon slot, secondary-label text that truncates, trailing parts.
pub fn work_log_line(
    glyph: Option<AnyElement>,
    label: AnyElement,
    trailing: Vec<AnyElement>,
    colors: &ChatColors,
) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(WORK_LINE_MIN_HEIGHT)
        .min_w_0()
        .gap(WORK_LINE_GAP)
        .text_size(TEXT_SM_RELAXED.size)
        .line_height(TEXT_SM_RELAXED.line_height)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(WORK_LINE_GAP)
                .children(glyph.map(|glyph| {
                    div().w(WORK_ICON_SLOT).h(px(24.)).flex_none().flex().items_center().justify_center().child(glyph)
                }))
                .child(div().flex_1().min_w_0().text_color(colors.theme.secondary_label).truncate().child(label)),
        )
        .children(trailing)
}

/// A 16px tool icon in `iconMuted` at 70%, tinted when the call failed.
pub fn tool_icon(glyph: IconName, colors: &ChatColors, failed: bool) -> AnyElement {
    let color = if failed { alpha(colors.theme.error_foreground, 0.4) } else { colors.theme.icon_muted };
    let svg = Icon::new(glyph).size(IconSize::Custom(TOOL_ICON_SIZE)).color(color).stroke(chat_icon_stroke(glyph));
    if failed { svg.into_any_element() } else { div().opacity(0.7).child(svg).into_any_element() }
}

/// The trailing `X` of a failed call.
pub fn failed_mark(colors: &ChatColors) -> AnyElement {
    Icon::new(IconName::X).size(IconSize::Custom(SMALL_ICON)).color(alpha(colors.theme.error_foreground, 0.4)).into_any_element()
}

/// A disclosure chevron: right when collapsed, down when open, in a 16px box.
pub fn disclosure(open: bool, size: Pixels, color: Hsla) -> AnyElement {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::Custom(size)).color(color).stroke(chat_icon_stroke(if open { IconName::ChevronDown } else { IconName::ChevronRight })))
        .into_any_element()
}

/// The row timestamp, shown only while the row is hovered.
pub fn hover_timestamp(millis: Option<i64>, colors: &ChatColors) -> Option<AnyElement> {
    let millis = millis?;
    Some(
        div()
            .absolute()
            .right_0()
            .top_0()
            .h_full()
            .flex()
            .items_center()
            .pl_2()
            .pr_1()
            .bg(colors.theme.canvas)
            .text_size(TEXT_XS.size)
            .line_height(TEXT_XS.line_height)
            .text_color(colors.theme.muted_foreground)
            .opacity(0.)
            .group_hover(ROW_GROUP, |s| s.opacity(1.))
            .child(format_clock(millis))
            .into_any_element(),
    )
}

/// A label with the stepped `live-tool-shine`: a 72px band of `foreground`
/// sweeps over `secondaryLabel` text in 30 steps. Colours are set per
/// character, so the sweep costs one text layout per step and no per-frame work.
pub fn shimmer_label(text: SharedString, step: u64, base: Hsla, highlight: Hsla) -> AnyElement {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let band_chars = (SHIMMER_BAND / AVERAGE_CHAR_WIDTH).max(1.);
    let span = chars.len() as f32 + band_chars;
    let center = (step % SHIMMER_STEPS) as f32 / SHIMMER_STEPS as f32 * span - band_chars / 2.;
    let mut runs: Vec<gpui_kit::TextRun> = Vec::with_capacity(chars.len());
    for (position, (offset, c)) in chars.iter().enumerate() {
        let distance = ((position as f32 - center).abs() / (band_chars / 2.)).min(1.);
        // The band's soft profile: full in the middle, fading to the edges.
        let weight = (1. - distance).powf(0.8);
        let color = mix(base, highlight, weight);
        let len = c.len_utf8();
        let _ = offset;
        match runs.last_mut() {
            Some(last) if last.color == color => last.len += len,
            _ => runs.push(gpui_kit::TextRun {
                len,
                font: sans(FontWeight::NORMAL),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }),
        }
    }
    if runs.is_empty() {
        return div().into_any_element();
    }
    StyledText::new(text).with_runs(runs).into_any_element()
}

/// Linear blend of two colours in RGB.
pub fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    gpui_kit::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// `Button size=xs variant=ghost(-muted)` holding an icon and an optional label:
/// 24px tall, 7px side padding, radius 8px.
pub fn xs_button(id: impl Into<ElementId>, glyph: Option<IconName>, label: Option<SharedString>, color: Hsla, colors: &ChatColors) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .px(px(if label.is_some() { 7. } else { 6. }))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(TEXT_XS.size)
        .text_color(color)
        .hover(|s| s.bg(colors.theme.accent_surface).text_color(colors.theme.text))
        .children(glyph.map(|g| Icon::new(g).size(IconSize::Custom(SMALL_ICON)).color(color).stroke(chat_icon_stroke(g))))
        .children(label)
}

/// Diff counts `+A -D` in mono (`DiffStatLabel` inline).
pub fn diff_stat(additions: u32, deletions: u32, colors: &ChatColors, size: Pixels) -> Div {
    div()
        .flex()
        .gap_1()
        .font_family(super::chat_style::mono_family())
        .text_size(size)
        .child(div().text_color(colors.diff_addition).child(format!("+{additions}")))
        .child(div().text_color(colors.diff_deletion).child(format!("-{deletions}")))
}
