//! The sidebar thread hover card (`ThreadHoverCard.tsx` and
//! `SidebarThreadTooltip` in `SidebarThreadRow.tsx`): a glass tooltip to the
//! right of the row with the title, then one 12px line each for the project,
//! environment, branch, model, hand-offs, terminals and the last error.

use gpui_kit::{AnyElement, App, Div, FontWeight, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use t3code_contracts::EnvironmentMachineKind;

use crate::ActiveAppTheme as _;
use crate::components::icons::{BrandIcon, Icon, IconName, IconSize};
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

/// `ICON_BY_KIND` in `EnvironmentMachineIcon.tsx` at `size`: Lucide glyphs,
/// the fork's Linux mark, and its two custom glyphs drawn from their SVG
/// geometry (24px viewBox, 1.75 stroke).
fn machine_glyph(kind: EnvironmentMachineKind, color: Hsla, colors: &ShellColors) -> AnyElement {
    let size = colors.r(12.);
    let unit = size / 24.;
    let lucide = |name| Icon::new(name).size(IconSize::Custom(size)).color(color).into_any_element();
    // A rounded rectangle outline in viewBox units.
    let frame = |x: f32, y: f32, w: f32, h: f32, r: f32| {
        div()
            .absolute()
            .left(unit * (x - 0.875))
            .top(unit * (y - 0.875))
            .w(unit * (w + 1.75))
            .h(unit * (h + 1.75))
            .rounded(unit * (r + 0.875))
            .border(unit * 1.75)
            .border_color(color)
    };
    let dot = |cx: f32, cy: f32, d: f32| {
        div().absolute().left(unit * (cx - d / 2.)).top(unit * (cy - d / 2.)).size(unit * d).rounded_full().bg(color)
    };
    match kind {
        EnvironmentMachineKind::Server => lucide(IconName::Server),
        EnvironmentMachineKind::Cloud => lucide(IconName::Cloud),
        EnvironmentMachineKind::Laptop => lucide(IconName::Laptop),
        EnvironmentMachineKind::Linux => Icon::new(BrandIcon::Linux).size(IconSize::Custom(size)).color(color).into_any_element(),
        // `MacMiniIcon`: rect 18x12 at (3,6) rx 2.5 and a dot of r 1 at (7.5,12).
        EnvironmentMachineKind::MacMini => {
            div().relative().flex_none().size(size).child(frame(3., 6., 18., 12., 2.5)).child(dot(7.5, 12., 2.)).into_any_element()
        }
        // `MacStudioIcon`: rect 18x14 at (3,5) rx 2 and round-capped dots at y 15.
        EnvironmentMachineKind::MacStudio => div()
            .relative()
            .flex_none()
            .size(size)
            .child(frame(3., 5., 18., 14., 2.))
            .children([7., 11., 15.].map(|x| dot(x, 15., 1.75)))
            .into_any_element(),
        _ => lucide(IconName::Monitor),
    }
}

/// One `flex items-center gap-2` line: a 12px icon and truncated text.
fn line(icon: impl IntoElement, text: impl IntoElement, colors: &ShellColors) -> Div {
    div().flex().min_w_0().items_center().gap(colors.r(8.)).child(div().flex_none().child(icon)).child(text)
}

fn icon(name: IconName, color: Hsla, colors: &ShellColors) -> Icon {
    Icon::new(name).size(IconSize::Custom(colors.r(12.))).color(color)
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
    let mut rows = div().flex().flex_col().gap(colors.r(6.)).pl(colors.r(2.)).text_size(colors.r(12.)).line_height(colors.r(16.)).text_color(muted);
    if let Some(label) = project_label {
        rows = rows.child(line(
            ProjectFavicon::new(label.clone()).size(colors.r(12.)),
            div().min_w_0().truncate().text_color(soft).child(label.clone()), colors));
    }
    if let Some(label) = details.environment_label.clone() {
        let kind = details.environment_machine.unwrap_or(EnvironmentMachineKind::Desktop);
        rows = rows.child(line(machine_glyph(kind, muted, colors), div().min_w_0().truncate().text_color(soft).child(label), colors));
    }
    if let Some(branch) = thread.branch.as_ref() {
        rows = rows.child(line(icon(IconName::GitBranch, muted, colors), crate::components::sidebar::sidebar_thread_row::middle_truncate(branch), colors));
    }
    if details.branch_mismatch {
        rows = rows.child(
            div()
                .flex()
                .min_w_0()
                .items_start()
                .gap(colors.r(8.))
                .text_color(colors.warning)
                .child(div().mt(colors.r(2.)).flex_none().child(icon(IconName::CircleAlert, colors.warning, colors)))
                .child(div().flex_1().min_w_0().child(t!("You're currently checked out on another branch."))),
        );
    }
    if let Some(model) = details.model_label.clone() {
        let mark = thread.provider.as_ref().map(|p| crate::components::sidebar::sidebar_thread_row::provider_mark(p, colors.r(12.), colors, cx));
        rows = rows.child(line(
            div().opacity(0.6).children(mark),
            div().min_w_0().truncate().text_color(soft).child(model), colors));
    }
    if !details.handed_off_from.is_empty() {
        let names = details.handed_off_from.join(", ");
        rows = rows.child(line(
            icon(IconName::ArrowRightLeft, muted, colors),
            div().min_w_0().truncate().text_color(soft).child(format!("{}{names}", t!("Handed off from "))), colors));
    }
    if thread.running_terminal_count > 0 {
        let count = thread.running_terminal_count;
        rows = rows.child(line(
            icon(IconName::Terminal, colors.terminal_running, colors),
            div().min_w_0().truncate().text_color(soft).child(t!(
                "{count, plural, one {{count} terminal process running} other {{count} terminal processes running}}",
                count = count
            )), colors));
    }
    if let Some(class) = thread.runtime.as_ref().and_then(|r| r.last_error_class) {
        let (color, label) = match class {
            ErrorClass::UsageLimit => (colors.warning, t!("Usage limit reached")),
            ErrorClass::Other => (theme.colors.error, t!("Error occurred")),
        };
        rows = rows.child(line(icon(IconName::CircleAlert, color, colors), div().min_w_0().truncate().text_color(color).child(label), colors));
    }
    div()
        .flex()
        .flex_col()
        .max_w(colors.r(320.))
        .gap(colors.r(8.))
        .px(colors.r(4.))
        .py(colors.r(8.))
        .child(
            div()
                .truncate()
                .text_size(colors.r(12.))
                .line_height(colors.r(15.))
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
        .max_w(colors.r(320.))
        .rounded(theme.radii.md)
        .border_1()
        .border_color(colors.glass_border)
        .bg(colors.glass)
        .shadow(theme.shadows.xl25.clone())
        .child(div().min_w_0().px(colors.r(8.)).py(colors.r(4.)).child(card))
}
