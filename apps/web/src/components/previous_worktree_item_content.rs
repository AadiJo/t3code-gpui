//! The "Previous worktree" menu item: a history icon, the label, and the
//! worktree's branch on a second muted line. Ports `PreviousWorktreeItemContent.tsx`.

use gpui_kit::{IntoElement, ParentElement as _, Styled as _, div, px};

use super::icons::{Icon, IconName, IconSize};
use super::ui::TextStyleExt as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::i18n::t;

/// The item's content for a worktree last used on `branch`.
pub fn previous_worktree_item_content(theme: &AppTheme, branch: Option<&str>) -> impl IntoElement {
    div()
        .flex()
        .min_w_0()
        .items_start()
        .gap(px(6.))
        .child(div().mt(px(4.)).child(Icon::new(IconName::History).size(IconSize::Xs)))
        .child(
            div().flex().flex_col().min_w_0().child(t!("Previous worktree")).children(branch.map(|branch| {
                div()
                    .min_w_0()
                    .truncate()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(branch.to_owned())
            })),
        )
}
