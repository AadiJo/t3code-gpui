//! The thread error pinned over the top of the timeline (`ThreadErrorBanner.tsx` in
//! `ChatBannerLane.tsx`): a send failure or the server's last error for the thread, in a glass
//! alert with an expandable message and a dismiss button; ChatGPT usage limits get their own copy.

use gpui_kit::{
    App, ClipboardItem, Context, EventEmitter, Hsla, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px,
};

use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::{GlassExt as _, TextStyleExt as _};
use crate::i18n::t;
use crate::theme_color::{flatten, mix_srgb, with_alpha};
use crate::theme_tokens::Glass;

/// `CHATGPT_USAGE_LIMIT_CODE` in `packages/shared/src/usageLimits.ts`.
pub const CHATGPT_USAGE_LIMIT_CODE: &str = "subscription_sharing_usage_limit_exceeded";
/// `CHATGPT_WORKSPACE_USAGE_LIMIT_CODE`.
pub const CHATGPT_WORKSPACE_USAGE_LIMIT_CODE: &str = "subscription_sharing_workplace_individual_usage_limit_exceeded";
const CHATGPT_WORKSPACE_ADMIN_REQUEST: &str = "I've reached the limit for using my ChatGPT plan in T3 Code. Could you increase our workspace's usage limits so I can continue?\n\nGo to admin.openai.com → Workspace → Usage limits. Thank you!";
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/#settings/Usage";

/// What the banner shows: the error text, whether it is a usage limit (the warning variant) and
/// the provider's error code.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadErrorBannerInput {
    pub error: SharedString,
    pub usage_limit: bool,
    pub code: Option<SharedString>,
}

/// The banner asks its host to clear the error and hide this message for the session.
#[derive(Clone, Debug)]
pub enum ThreadErrorBannerEvent {
    Dismiss,
}

/// `getThreadErrorBannerMessage`: the nested `error.message` of a JSON error body, else the text.
pub fn banner_message(error: &str) -> SharedString {
    let nested = serde_json::from_str::<serde_json::Value>(error).ok().and_then(|payload| {
        let message = payload.get("error")?.get("message")?.as_str()?.trim().to_owned();
        (!message.is_empty()).then_some(message)
    });
    nested.map(SharedString::from).unwrap_or_else(|| SharedString::from(error.to_owned()))
}

/// The banner view. The host sets its input and hides it when there is none.
pub struct ThreadErrorBanner {
    input: Option<ThreadErrorBannerInput>,
    expanded: bool,
}

impl ThreadErrorBanner {
    pub fn new() -> Self {
        Self { input: None, expanded: false }
    }

    pub fn input(&self) -> Option<&ThreadErrorBannerInput> {
        self.input.as_ref()
    }

    pub fn set_input(&mut self, input: Option<ThreadErrorBannerInput>, cx: &mut Context<Self>) {
        if self.input != input {
            self.expanded = false;
            self.input = input;
            cx.notify();
        }
    }
}

impl Default for ThreadErrorBanner {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<ThreadErrorBannerEvent> for ThreadErrorBanner {}

/// `copyAdminRequest`.
fn copy_admin_request(cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(CHATGPT_WORKSPACE_ADMIN_REQUEST.to_owned()));
    Toaster::push(ToastData::new(ToastKind::Success, t!("Request copied")).description(t!("Send it to your workspace admin.")), cx);
}

/// `alert-glass` over the variant's tint: the background at the glass opacity with 4% of the tint.
fn alert_glass(theme: &AppTheme, tint: Hsla) -> Glass {
    let base = mix_srgb(theme.colors.background(), theme.glass_opacity.alpha(), Hsla::transparent_black());
    let top = with_alpha(tint, 0.04);
    let a = top.a + base.a * (1. - top.a);
    let (t, b) = (top.to_rgb(), base.to_rgb());
    let channel = |x: f32, y: f32| if a > 0. { (x * t.a + y * b.a * (1. - t.a)) / a } else { 0. };
    let fill: Hsla = gpui_kit::Rgba { r: channel(t.r, b.r), g: channel(t.g, b.g), b: channel(t.b, b.b), a }.into();
    Glass { fill, opaque: flatten(fill, theme.colors.canvas), blur: theme.derived.glass_surface.blur }
}

impl Render for ThreadErrorBanner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(input) = self.input.clone() else { return div().into_any_element() };
        let theme = cx.app_theme();
        let c = &theme.colors;
        let code = input.code.as_deref();
        let workspace_limit = code == Some(CHATGPT_WORKSPACE_USAGE_LIMIT_CODE);
        let chatgpt_limit = workspace_limit || code == Some(CHATGPT_USAGE_LIMIT_CODE);
        let warning = input.usage_limit || chatgpt_limit;
        let (tint, border, text, icon_color) = if warning {
            (c.warning, with_alpha(c.warning, 0.32), c.warning_foreground, c.warning)
        } else {
            (c.destructive(), with_alpha(c.error, 0.32), c.error_foreground, c.error)
        };
        let line = TextSize::Sm.line_height(theme.fonts.interface);
        let icon = if chatgpt_limit {
            Icon::new(BrandIcon::OpenAi).size(IconSize::Md).color(c.text)
        } else {
            Icon::new(IconName::CircleAlert).size(IconSize::Md).color(icon_color)
        };
        let description = if chatgpt_limit {
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(1.))
                .child(div().font_weight(weight::MEDIUM).child(t!("ChatGPT usage limit reached")))
                .when(workspace_limit, |d| {
                    d.child(div().child(t!("You've reached your workspace's limit for using your ChatGPT plan in other apps. Copy a request to ask your workspace admin for increased usage limits.")))
                })
                .into_any_element()
        } else {
            // `ExpandableText`: the short message, line-clamped, with "Show full error".
            let collapsed = banner_message(&input.error);
            let can_expand = collapsed != input.error || input.error.chars().count() > 180 || input.error.contains('\n');
            let expanded = self.expanded;
            div()
                .min_w_0()
                .child(
                    div()
                        .whitespace_normal()
                        .when(!expanded && can_expand, |d| d.line_clamp(3))
                        .child(if expanded { input.error.clone() } else { collapsed }),
                )
                .when(can_expand, |d| {
                    d.child(
                        div()
                            .id("thread-error-expand")
                            .mt(theme.sp(1.))
                            .font_weight(weight::MEDIUM)
                            .text_color(c.muted_foreground)
                            .cursor_pointer()
                            .hover(|s| s.text_color(c.text).underline())
                            .child(if expanded { t!("Show less") } else { t!("Show full error") })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.expanded = !this.expanded;
                                cx.notify();
                            })),
                    )
                })
                .into_any_element()
        };
        let action = div()
            .flex()
            .flex_shrink_0()
            .gap(theme.sp(1.))
            .items_center()
            // `h-lh self-start` with taller buttons centred on the first line: CSS centres the
            // overflow evenly, so the row starts half the overflow above the line.
            .mt((px(line) - if chatgpt_limit { theme.sp(7.) } else { theme.sp(6.) }) / 2.)
            .when(workspace_limit, |d| {
                d.child(
                    Button::new("thread-error-copy-request")
                        .label(t!("Copy request for admin"))
                        .size(ButtonSize::Sm)
                        .on_click(|_, _, cx| copy_admin_request(cx)),
                )
            })
            .when(chatgpt_limit && !workspace_limit, |d| {
                d.child(
                    Button::new("thread-error-manage-usage")
                        .label(t!("Manage usage"))
                        .trailing_icon(IconName::ExternalLink)
                        .size(ButtonSize::Sm)
                        .on_click(|_, _, cx| cx.open_url(CHATGPT_USAGE_URL)),
                )
            })
            .child(
                Button::new("thread-error-dismiss")
                    .icon(IconName::X)
                    .size(ButtonSize::IconXs)
                    .variant(ButtonVariant::Ghost)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ThreadErrorBannerEvent::Dismiss))),
            );
        // `mx-auto w-fit max-w-[min(48rem,calc(100%-2rem))] pt-3`
        div()
            .w_full()
            .flex()
            .justify_center()
            .pt(theme.sp(3.))
            .px(theme.rem(1.))
            .child(
                div()
                    .id("thread-error-banner")
                    .occlude()
                    .max_w(theme.rem(48.))
                    .rounded(theme.radii.xl)
                    .border_1()
                    .border_color(border)
                    .glass(alert_glass(theme, tint))
                    .px(theme.sp(3.5))
                    .py(theme.sp(3.))
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .text_color(text)
                    .child(
                        // `controlAlignment="first-line"` with an action: `sm:min-h-6 sm:pt-0.5`.
                        div()
                            .flex()
                            .items_start()
                            .gap(theme.sp(2.))
                            .min_h(theme.sp(6.))
                            .pt(theme.sp(0.5))
                            .child(div().flex().flex_shrink_0().items_center().justify_center().h(px(line)).w(px(16.)).child(icon))
                            .child(div().flex().flex_col().flex_1().min_w_0().gap(theme.sp(0.5)).text_color(with_alpha(text, 0.8)).child(description))
                            .child(action),
                    ),
            )
            .into_any_element()
    }
}
