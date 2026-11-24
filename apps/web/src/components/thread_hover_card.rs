//! The sidebar thread hover card (`ThreadHoverCard.tsx` and
//! `SidebarThreadTooltip` in `SidebarThreadRow.tsx`): a glass tooltip to the
//! right of the row with the title, then one 12px line each for the project,
//! environment, branch, model, hand-offs, terminals and the last error.

use crate::components::ui::GlassExt as _;
use gpui_kit::{AnyElement, App, Div, FontWeight, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, div, px};
use t3code_contracts::EnvironmentMachineKind;

use crate::ActiveAppTheme as _;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::project_favicon::ProjectFavicon;
use crate::components::sidebar::model::{ErrorClass, SidebarThread};
use crate::components::sidebar::style::ShellColors;
use crate::i18n::t;

/// What the card shows that the sidebar row does not carry. The live mapping
/// fills it per thread in [`crate::components::sidebar::model::SidebarSnapshot::hover_details`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadHoverDetails {
    /// The environment's label, when the window shows more than one.
    pub environment_label: Option<SharedString>,
    pub environment_machine: Option<EnvironmentMachineKind>,
    /// `modelLabel`, or `"<model> · <instance>"` when instances share a driver.
    pub model_label: Option<SharedString>,
    /// Provider instances the thread ran on before, by display name.
    pub handed_off_from: Vec<SharedString>,
    /// The checkout is on a branch other than the thread's.
    pub branch_mismatch: bool,
}

/// `ICON_BY_KIND` in `EnvironmentMachineIcon.tsx`. Linux, Mini PC and
/// Workstation are custom glyphs in the web app; Lucide's nearest stand in.
fn machine_icon(kind: EnvironmentMachineKind) -> IconName {
    match kind {
        EnvironmentMachineKind::Server => IconName::Server,
        EnvironmentMachineKind::Cloud => IconName::Cloud,
        EnvironmentMachineKind::Laptop => IconName::Laptop,
        _ => IconName::Monitor,
    }
}

/// One `flex items-center gap-2` line: a 12px icon and truncated text.
fn line(icon: impl IntoElement, text: impl IntoElement) -> Div {
    div().flex().min_w_0().items_center().gap(px(8.)).child(div().flex_none().child(icon)).child(text)
}

fn icon(name: IconName, color: Hsla) -> Icon {
    Icon::new(name).size(IconSize::Custom(px(12.))).color(color)
}

/// The card's content inside the tooltip surface (`px-1 py-2`, `gap-2`, rows `gap-1.5`).
pub fn thread_hover_card(
    thread: &SidebarThread,
    project_label: Option<&SharedString>,
    details: Option<&ThreadHoverDetails>,
    colors: &ShellColors,
    cx: &App,
) -> AnyElement {
    let theme = cx.app_theme();
    let soft = colors.foreground.opacity(0.75);
    let muted = colors.muted_foreground;
    let details = details.cloned().unwrap_or_default();
    let mut rows = div().flex().flex_col().gap(px(6.)).pl(px(2.)).text_size(px(12.)).line_height(px(16.)).text_color(muted);
    if let Some(label) = project_label {
        rows = rows.child(line(
            ProjectFavicon::new(label.clone()).size(px(12.)),
            div().min_w_0().truncate().text_color(soft).child(label.clone()),
        ));
    }
    if let Some(label) = details.environment_label.clone() {
        let kind = details.environment_machine.unwrap_or(EnvironmentMachineKind::Desktop);
        rows = rows.child(line(icon(machine_icon(kind), muted), div().min_w_0().truncate().text_color(soft).child(label)));
    }
    if let Some(branch) = thread.branch.as_ref() {
        rows = rows.child(line(icon(IconName::GitBranch, muted), crate::components::sidebar::sidebar_thread_row::middle_truncate(branch)));
    }
    if details.branch_mismatch {
        rows = rows.child(
            div()
                .flex()
                .min_w_0()
                .items_start()
                .gap(px(8.))
                .text_color(colors.warning)
                .child(div().mt(px(2.)).flex_none().child(icon(IconName::CircleAlert, colors.warning)))
                .child(div().flex_1().min_w_0().child(t!("You're currently checked out on another branch."))),
        );
    }
    if let Some(model) = details.model_label.clone() {
        let mark = thread.provider.as_ref().map(|p| crate::components::sidebar::sidebar_thread_row::provider_mark(p, px(12.), colors, cx));
        rows = rows.child(line(
            div().opacity(0.6).children(mark),
            div().min_w_0().truncate().text_color(soft).child(model),
        ));
    }
    if !details.handed_off_from.is_empty() {
        let names = details.handed_off_from.join(", ");
        rows = rows.child(line(
            icon(IconName::ArrowRightLeft, muted),
            div().min_w_0().truncate().text_color(soft).child(format!("{}{names}", t!("Handed off from "))),
        ));
    }
    if thread.running_terminal_count > 0 {
        let count = thread.running_terminal_count;
        rows = rows.child(line(
            icon(IconName::Terminal, colors.terminal_running),
            div().min_w_0().truncate().text_color(soft).child(t!(
                "{count, plural, one {{count} terminal process running} other {{count} terminal processes running}}",
                count = count
            )),
        ));
    }
    if let Some(class) = thread.runtime.as_ref().and_then(|r| r.last_error_class) {
        let (color, label) = match class {
            ErrorClass::UsageLimit => (colors.warning, t!("Usage limit reached")),
            ErrorClass::Other => (theme.colors.error, t!("Error occurred")),
        };
        rows = rows.child(line(icon(IconName::CircleAlert, color), div().min_w_0().truncate().text_color(color).child(label)));
    }
    div()
        .flex()
        .flex_col()
        .max_w(px(320.))
        .gap(px(8.))
        .px(px(4.))
        .py(px(8.))
        .child(
            div()
                .truncate()
                .text_size(px(12.))
                .line_height(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.foreground)
                .child(thread.title.clone()),
        )
        .child(rows)
        .into_any_element()
}

/// The glass tooltip surface around the card (`TooltipPopup variant="glass"`,
/// `max-w-80`, viewport `px-2 py-1`).
pub fn hover_card_surface(card: AnyElement, colors: &ShellColors, cx: &App) -> Div {
    let theme = cx.app_theme();
    div()
        .max_w(px(320.))
        .rounded(theme.radii.md)
        .border_1()
        .border_color(colors.glass_border)
        .glass(colors.glass)
        .shadow(theme.shadows.xl25.clone())
        .child(div().min_w_0().px(px(8.)).py(px(4.)).child(card))
}
