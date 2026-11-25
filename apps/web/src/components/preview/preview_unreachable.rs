//! A tab whose page failed to load. Ports `PreviewUnreachable.tsx`: "This site can't be
//! reached" with the friendly error, details and Reload, and `PreviewFileNotShown` for a
//! navigation the server's browser downloaded instead of showing.

use gpui_kit::{App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px, svg};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::{t, t_rich};
use crate::theme_color::with_alpha;

type Action = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// `describePreviewError`: Chromium's error name in words.
pub fn describe_preview_error(description: &str) -> SharedString {
    match description {
        "ERR_NAME_NOT_RESOLVED" | "ERR_NAME_RESOLUTION_FAILED" => t!("DNS address could not be found"),
        "ERR_CONNECTION_REFUSED" => t!("Connection refused"),
        "ERR_SOCKS_CONNECTION_FAILED" => t!("Could not reach this port on the environment"),
        "ERR_CONNECTION_RESET" => t!("Connection was reset"),
        "ERR_CONNECTION_CLOSED" => t!("Connection was closed"),
        "ERR_CONNECTION_TIMED_OUT" | "ERR_TIMED_OUT" => t!("Connection timed out"),
        "ERR_INTERNET_DISCONNECTED" => t!("No internet connection"),
        "ERR_CERT_AUTHORITY_INVALID" => t!("Certificate authority is not trusted"),
        "ERR_CERT_COMMON_NAME_INVALID" => t!("Certificate hostname mismatch"),
        "ERR_CERT_DATE_INVALID" => t!("Certificate is expired or not yet valid"),
        "ERR_TOO_MANY_REDIRECTS" => t!("Too many redirects"),
        "" => t!("Network error"),
        other => other.to_owned().into(),
    }
}

fn safe_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| match u.port() {
            Some(port) => format!("{h}:{port}"),
            None => h.to_owned(),
        }))
        .unwrap_or_else(|| url.to_owned())
}

/// The `<span className="font-semibold text-foreground">` run inside a muted sentence.
fn emphasized(theme: &crate::AppTheme, text: crate::i18n::RichText) -> gpui_kit::StyledText {
    let foreground = theme.colors.foreground();
    text.styled(move |tag| {
        (tag == "span").then(|| gpui_kit::HighlightStyle { color: Some(foreground), font_weight: Some(weight::SEMIBOLD), ..Default::default() })
    })
}

/// The 48px document-with-slash mark both states lead with (`ErrorIcon`).
fn error_icon(cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    svg().path("icons/ui/preview-error.svg").size(theme.sp(12.)).mb(theme.sp(6.)).text_color(with_alpha(theme.colors.muted_foreground, 0.7))
}

/// The column both states share: `max-w-xl px-8 py-12`, actions pinned to the bottom.
fn page(cx: &App, title: SharedString, body: impl IntoElement, middle: Option<gpui_kit::AnyElement>, actions: impl IntoElement) -> gpui_kit::Div {
    let theme = cx.app_theme();
    let c = &theme.colors;
    div().relative().flex().size_full().min_h_0().bg(c.background()).child(
        div()
            .mx_auto()
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .max_w(theme.rem(36.))
            .px(theme.sp(8.))
            .py(theme.sp(12.))
            .child(error_icon(cx))
            .child(div().mb(theme.sp(3.)).ui_text(theme, TextSize::Xl2, weight::SEMIBOLD).line_height(px(1.25 * f32::from(theme.rem(1.5)))).text_color(c.foreground()).child(title))
            .child(body)
            .children(middle)
            .child(div().mt_auto().flex().items_center().gap(theme.sp(2.)).pt(theme.sp(8.)).child(actions)),
    )
}

/// `PreviewUnreachable` for a `LoadFailed` tab.
#[derive(IntoElement)]
pub struct PreviewUnreachable {
    url: String,
    code: i64,
    description: String,
    show_details: bool,
    on_toggle_details: Action,
    on_reload: Action,
}

impl PreviewUnreachable {
    pub fn new(url: String, code: i64, description: String, show_details: bool) -> Self {
        Self { url, code, description, show_details, on_toggle_details: std::rc::Rc::new(|_, _| {}), on_reload: std::rc::Rc::new(|_, _| {}) }
    }

    pub fn on_toggle_details(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle_details = std::rc::Rc::new(action);
        self
    }

    pub fn on_reload(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reload = std::rc::Rc::new(action);
        self
    }
}

impl RenderOnce for PreviewUnreachable {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let host = safe_host(&self.url);
        let friendly = describe_preview_error(&self.description);
        let error_label = if self.description.is_empty() {
            format!("ERR_{}", if self.code == 0 { "FAILED".to_owned() } else { self.code.abs().to_string() })
        } else {
            self.description.clone()
        };
        let body = div()
            .ui_text(&theme, TextSize::Sm, weight::NORMAL)
            .line_height(px(1.625 * f32::from(theme.rem(0.875))))
            .text_color(c.muted_foreground)
            .child(emphasized(&theme, t_rich!("<span>{host}</span>: {friendly}.", host = host, friendly = friendly)));
        let details = self.show_details.then(|| {
            let item = |text: SharedString| {
                div().flex().gap(theme.sp(2.)).child(div().child("•")).child(div().child(text))
            };
            div()
                .mt(theme.sp(6.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(c.border)
                .bg(with_alpha(c.muted, 0.4))
                .p(theme.sp(4.))
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .child(div().mb(theme.sp(2.)).ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(t!("Try:")))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(1.))
                        .pl(theme.sp(1.))
                        .text_color(c.muted_foreground)
                        .child(item(t!("Checking your connection")))
                        .child(item(t!("Confirming the dev server is running")))
                        .child(item(t!("Checking the proxy and the firewall"))),
                )
                .into_any_element()
        });
        let label = div()
            .mt(theme.sp(8.))
            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
            .text_color(with_alpha(c.muted_foreground, 0.7))
            .child(error_label.to_uppercase());
        let middle = div().children(details).child(label).into_any_element();
        let (toggle, reload) = (self.on_toggle_details.clone(), self.on_reload.clone());
        let actions = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(theme.sp(2.))
            .child(
                Button::new("preview-unreachable-details")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(if self.show_details { t!("Hide details") } else { t!("Details") })
                    .on_click(move |_, window, cx| toggle(window, cx)),
            )
            .child(div().flex_1())
            .child(Button::new("preview-unreachable-reload").size(ButtonSize::Sm).label(t!("Reload")).on_click(move |_, window, cx| reload(window, cx)));
        page(cx, SharedString::from(t_rich!("This site can’t be reached").plain()), body, Some(middle), actions)
    }
}

/// `PreviewFileNotShown`: the navigation downloaded a file. "Download" saves it from the
/// server; "Open in browser" opens the page's URL in the system browser.
#[derive(IntoElement)]
pub struct PreviewFileNotShown {
    pub url: String,
    pub file_name: String,
    pub download_url: Option<String>,
}

impl RenderOnce for PreviewFileNotShown {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let host = safe_host(&self.url);
        let body = div()
            .ui_text(&theme, TextSize::Sm, weight::NORMAL)
            .line_height(px(1.625 * f32::from(theme.rem(0.875))))
            .text_color(c.muted_foreground)
            .child(
                emphasized(&theme, t_rich!("<span>{fileName}</span>{\" \"}from {host} was downloaded instead.", fileName = self.file_name.clone(), host = host)),
            );
        let open_url = self.url.clone();
        let actions = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(theme.sp(2.))
            .child(div().flex_1())
            .child(
                Button::new("preview-file-open")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t!("Open in browser"))
                    .on_click(move |_, _, cx| cx.open_url(&open_url)),
            )
            .children(self.download_url.map(|url| {
                Button::new("preview-file-download").size(ButtonSize::Sm).label(t!("Download", context = "verb")).on_click(move |_, _, cx| cx.open_url(&url))
            }));
        page(cx, SharedString::from(t_rich!("This file can’t be shown here").plain()), body, None, actions)
    }
}
