//! The composer's icon strokes (send and attach at 1.8, stop at 2.2, as the
//! fork draws them) and the provider marks the model picker needs. The
//! glyphs themselves are the shared [`IconName`] set.

use gpui_kit::{App, Hsla, IntoElement, ParentElement as _, Pixels, Styled as _, div, img, px, rgb, svg};

use crate::components::chat::composer_theme::ComposerColors;
use crate::components::icons::IconName;
use crate::model_selection::ProviderIcon;

/// The stroke width the composer draws `icon` at: the fork draws send,
/// attach, steer, queue and previous at 1.8 and stop at 2.2.
pub fn composer_icon_stroke(icon: IconName) -> f32 {
    match icon {
        IconName::Paperclip
        | IconName::CornerDownLeft
        | IconName::CornerUpRight
        | IconName::ListPlus
        | IconName::Check
        | IconName::PlayFilled
        | IconName::ArrowLeft => 1.8,
        IconName::Square => 2.2,
        _ => 1.75,
    }
}

/// A provider or model mark. Monochrome marks take the theme's foreground;
/// Claude keeps its orange, Gemini and Meta their own colours.
pub fn provider_icon(mark: ProviderIcon, size: Pixels, initials: &str, colors: &ComposerColors, _cx: &App) -> gpui_kit::AnyElement {
    let dark = colors.is_dark();
    let mono = |path: &'static str, color: Hsla| svg().path(path).size(size).flex_none().text_color(color).into_any_element();
    match mark {
        ProviderIcon::OpenAi => mono("icons/composer/brand/openai.svg", if dark { rgb(0xffffff).into() } else { rgb(0x000000).into() }),
        ProviderIcon::Claude => mono("icons/composer/brand/claude.svg", rgb(0xd97757).into()),
        ProviderIcon::Cursor => mono("icons/composer/brand/cursor.svg", if dark { rgb(0xedecec).into() } else { rgb(0x26251e).into() }),
        ProviderIcon::Grok => mono("icons/composer/brand/grok.svg", if dark { rgb(0xf5f5f5).into() } else { rgb(0x0f0f0f).into() }),
        ProviderIcon::OpenCode => img(if dark { "icons/composer/brand/opencode-dark.svg" } else { "icons/composer/brand/opencode-light.svg" })
            .size(size)
            .flex_none()
            .into_any_element(),
        ProviderIcon::Gemini => img("icons/composer/brand/gemini.svg").size(size).flex_none().into_any_element(),
        ProviderIcon::Meta => img("icons/composer/brand/meta.svg").size(size).flex_none().into_any_element(),
        ProviderIcon::Initials => div()
            .size(size)
            .flex_none()
            .rounded(px(4.))
            .bg(colors.input)
            .flex()
            .items_center()
            .justify_center()
            .text_size(size * 0.5)
            .text_color(colors.foreground)
            .child(initials.chars().take(2).collect::<String>())
            .into_any_element(),
    }
}
