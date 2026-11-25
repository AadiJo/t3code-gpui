//! Prompt links (`components/promptLinks/`): the `/new` page and the toasts that come with a
//! prompt placed from a link (`linkedPrompt.ts`).

pub mod new_thread_link_view;

use gpui_kit::{App, SharedString};
use t3code_shared::prompt_links::LinkedPrompt;

use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;

/// `warnAboutHiddenCharacters`: a toast that stays until closed when the link's prompt had
/// hidden characters removed.
pub fn warn_about_hidden_characters(prompt: &LinkedPrompt, cx: &mut App) {
    if prompt.hidden_characters == 0 {
        return;
    }
    Toaster::push(
        ToastData::new(ToastKind::Warning, t!("This prompt had hidden characters, it may be malicious"))
            .description(t!(
                "{count, plural, one {T3 Code removed {count} hidden character. Read the prompt before you send it.} other {T3 Code removed {count} hidden characters. Read the prompt before you send it.}}",
                count = prompt.hidden_characters
            ))
            .persistent(),
        cx,
    );
}

/// The toast after `placeLinkedPrompt` put the prompt in a composer.
pub fn prompt_placed(notes: &[SharedString], cx: &mut App) {
    let description = std::iter::once(t!("Read it before you send.")).chain(notes.iter().cloned()).collect::<Vec<_>>().join(" ");
    Toaster::push(ToastData::new(ToastKind::Info, t!("Prompt added from a link")).description(description), cx);
}

/// CSS `text-wrap: balance` for a centred block (`Empty`'s `text-balance`): the narrowest width
/// at most `max` that wraps `text` into as few lines as `max` does.
pub fn balanced_width(text: &str, font: gpui_kit::Font, size: gpui_kit::Pixels, max: gpui_kit::Pixels, window: &gpui_kit::Window) -> gpui_kit::Pixels {
    use gpui_kit::{IndentAdjustment, LineFragment, px};
    let mut wrapper = window.text_system().line_wrapper(font, size);
    let fragments = [LineFragment::text(text)];
    let mut lines = |width| wrapper.wrap_line(&fragments, width, IndentAdjustment::NoIndent).count() + 1;
    let target = lines(max);
    if target <= 1 {
        return max;
    }
    let (mut lo, mut hi) = (px(0.), max);
    while hi - lo > px(0.25) {
        let mid = (lo + hi) / 2.;
        if lines(mid) <= target { hi = mid } else { lo = mid }
    }
    hi.ceil()
}
