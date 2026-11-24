//! Web-styled context menus (fork: desktop draws the DOM fallback instead of a
//! native menu). Ports the item builders of `threadActionMenu.logic.ts` and
//! the project-folder menu in `Sidebar.tsx`, and the look of
//! `contextMenuFallback.ts:391-534`.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::model::{ProjectRef, ThreadKey};
use crate::pixel_grid::PixelGrid;
use super::style::{MENU_MAX_WIDTH, MENU_MIN_WIDTH, MENU_ROW_HEIGHT, ShellColors};

/// What a menu row does when picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    /// "Find in thread": only in the chat header's menu, which handles it itself.
    FindInThread,
    NewThreadOnBranch,
    Pin,
    Unpin,
    Settle,
    Unsettle,
    Snooze(SnoozePreset),
    SnoozeCustom,
    Wake,
    Rename,
    RegenerateTitle,
    MarkUnread,
    FilterByProject,
    /// "Auto-settle behavior": `true` follows the settings, `false` never auto-settles.
    AutoSettle(bool),
    CopyPath,
    CopyBranch,
    CopyThread,
    CopyThreadId,
    ProjectSettings,
    Archive,
    Delete,
    NewThread,
    PinProject,
    UnpinProject,
}

/// `DEFAULT_SNOOZE_PRESETS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnoozePreset {
    OneHour,
    ThreeHours,
    ThisEvening,
    Tomorrow,
    NextWeek,
}

/// One menu row; `children` makes it a submenu trigger.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub command: Option<MenuCommand>,
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub detail: Option<SharedString>,
    pub destructive: bool,
    pub disabled: bool,
    pub separator_before: bool,
    /// A radio row: a check when `true`, an icon-sized gap when `false`.
    pub checked: Option<bool>,
    pub children: Vec<MenuItem>,
}

impl MenuItem {
    fn new(command: MenuCommand, label: impl Into<SharedString>) -> Self {
        Self {
            command: Some(command),
            label: label.into(),
            icon: None,
            detail: None,
            destructive: false,
            disabled: false,
            separator_before: false,
            checked: None,
            children: Vec::new(),
        }
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn separated(mut self) -> Self {
        self.separator_before = true;
        self
    }

    fn submenu(mut self, children: Vec<MenuItem>) -> Self {
        self.command = None;
        self.children = children;
        self
    }
}

/// What the open menu acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    Thread(ThreadKey),
    Project(ProjectRef),
    /// A new-thread draft, by draft id.
    Draft(SharedString),
}

/// An open context menu at a window position.
#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub target: MenuTarget,
    pub position: Point<Pixels>,
    pub items: Vec<MenuItem>,
    pub highlighted: Option<usize>,
    /// The open submenu (index into `items`) and its highlighted row.
    pub submenu: Option<(usize, Option<usize>)>,
}

/// Facts about a thread the menu depends on.
pub struct ThreadMenuState<'a> {
    pub branch: Option<&'a str>,
    pub is_pinned: bool,
    pub is_settled: bool,
    pub is_snoozed: bool,
    pub is_running: bool,
    pub project_label: &'a str,
    pub project_filter_active: bool,
    /// Rows offer "Filter by <project>"; the chat header's menu does not.
    pub show_project_filter: bool,
    /// The chat header's menu starts with "Find in thread".
    pub can_find: bool,
    /// The thread follows the auto-settle setting (no `autoSettleDisabledAt`).
    pub auto_settle_enabled: bool,
}

/// `buildThreadActionMenuItems` for a sidebar row.
pub fn thread_menu_items(state: &ThreadMenuState<'_>) -> Vec<MenuItem> {
    use MenuCommand as C;
    let mut items = Vec::new();
    if state.can_find {
        items.push(MenuItem::new(C::FindInThread, "Find in thread").icon(IconName::TextSearch));
    }
    if let Some(branch) = state.branch {
        items.push(
            MenuItem::new(C::NewThreadOnBranch, format!("New thread on {branch}"))
                .icon(IconName::MessageSquarePlus),
        );
    }
    items.push(if state.is_pinned {
        MenuItem::new(C::Unpin, "Unpin thread").icon(IconName::PinOff)
    } else {
        MenuItem::new(C::Pin, "Pin thread").icon(IconName::Pin)
    });
    items.push(if state.is_settled {
        MenuItem::new(C::Unsettle, "Un-settle thread").icon(IconName::CircleCheck)
    } else {
        MenuItem::new(C::Settle, "Settle thread").icon(IconName::CircleCheck)
    });
    items.push(if state.is_snoozed {
        MenuItem::new(C::Wake, "Wake thread").icon(IconName::Clock)
    } else {
        MenuItem::new(C::SnoozeCustom, "Snooze")
            .icon(IconName::Clock)
            .submenu(snooze_items())
    });
    items.push(
        MenuItem::new(C::Rename, "Rename thread")
            .icon(IconName::Pencil)
            .separated(),
    );
    items.push(MenuItem::new(C::RegenerateTitle, "Regenerate title").icon(IconName::RefreshCw));
    items.push(MenuItem::new(C::MarkUnread, "Mark unread").icon(IconName::MailOpen));
    if state.show_project_filter {
        items.push(
            MenuItem::new(
                C::FilterByProject,
                if state.project_filter_active {
                    "Show all projects".to_string()
                } else {
                    format!("Filter by {}", state.project_label)
                },
            )
            .icon(IconName::FolderTree),
        );
    }
    items.push(
        MenuItem::new(C::AutoSettle(true), "Auto-settle behavior")
            .icon(IconName::Timer)
            .submenu(vec![
                MenuItem::new(C::AutoSettle(true), "Follow settings").checked(state.auto_settle_enabled),
                MenuItem::new(C::AutoSettle(false), "Never").checked(!state.auto_settle_enabled),
            ]),
    );
    let mut copy = vec![MenuItem::new(C::CopyPath, "Path").icon(IconName::Folder)];
    if state.branch.is_some() {
        copy.push(MenuItem::new(C::CopyBranch, "Branch").icon(IconName::GitBranch));
    }
    copy.push(MenuItem::new(C::CopyThread, "Thread").icon(IconName::MessageSquare));
    copy.push(MenuItem::new(C::CopyThreadId, "Thread ID").icon(IconName::Hash));
    items.push(
        MenuItem::new(C::CopyPath, "Copy")
            .icon(IconName::Copy)
            .separated()
            .submenu(copy),
    );
    items.push(MenuItem::new(C::ProjectSettings, "Project settings").icon(IconName::Settings));
    let mut archive = MenuItem::new(C::Archive, "Archive thread")
        .icon(IconName::Archive)
        .separated();
    archive.disabled = state.is_running;
    items.push(archive);
    let mut delete = MenuItem::new(C::Delete, "Delete").icon(IconName::Trash);
    delete.destructive = true;
    items.push(delete);
    items
}

/// Snooze presets (`DEFAULT_SNOOZE_PRESETS`), for the row's snooze button.
/// `buildDraftActionMenuItems` for a draft without a branch: Copy > Path,
/// Project settings, then the destructive "Discard draft" (`Delete`).
pub fn draft_menu_items() -> Vec<MenuItem> {
    use MenuCommand as C;
    let mut discard = MenuItem::new(C::Delete, "Discard draft").icon(IconName::Trash).separated();
    discard.destructive = true;
    vec![
        MenuItem::new(C::CopyPath, "Copy")
            .icon(IconName::Copy)
            .submenu(vec![MenuItem::new(C::CopyPath, "Path").icon(IconName::Folder)]),
        MenuItem::new(C::ProjectSettings, "Project settings").icon(IconName::Settings),
        discard,
    ]
}

pub fn snooze_items() -> Vec<MenuItem> {
    use MenuCommand as C;
    use SnoozePreset as P;
    vec![
        MenuItem::new(C::Snooze(P::OneHour), "1 hour"),
        MenuItem::new(C::Snooze(P::ThreeHours), "3 hours"),
        MenuItem::new(C::Snooze(P::ThisEvening), "This evening").detail("6:00 PM"),
        MenuItem::new(C::Snooze(P::Tomorrow), "Tomorrow").detail("9:00 AM"),
        MenuItem::new(C::Snooze(P::NextWeek), "Next week").detail("Mon 9:00 AM"),
        MenuItem::new(C::SnoozeCustom, "Custom…").separated(),
    ]
}

/// The project folder menu (`Sidebar.tsx:1263-1300`).
pub fn project_menu_items(pinned: bool) -> Vec<MenuItem> {
    use MenuCommand as C;
    vec![
        MenuItem::new(C::NewThread, "New thread"),
        MenuItem::new(C::CopyPath, "Copy path"),
        if pinned {
            MenuItem::new(C::UnpinProject, "Unpin project").icon(IconName::PinOff)
        } else {
            MenuItem::new(C::PinProject, "Pin project").icon(IconName::Pin)
        },
        MenuItem::new(C::ProjectSettings, "Project settings").icon(IconName::Settings),
    ]
}

/// Estimated menu height, for clamping it inside the window.
pub fn menu_height(items: &[MenuItem], colors: &ShellColors) -> Pixels {
    // `p-1`, the rows and each separator's `my-1` scale with the rem; the
    // 1px border and the separators' 1px rules do not.
    let separators = items.iter().filter(|i| i.separator_before).count() as f32;
    colors.r(8. + items.len() as f32 * f32::from(MENU_ROW_HEIGHT) + separators * 8.) + px(2. + separators)
}

/// One menu row's look (highlighted rows use `accent`; destructive ones a red tint).
pub fn menu_row(item: &MenuItem, highlighted: bool, colors: &ShellColors) -> Div {
    let (fg, bg) = match (item.destructive, highlighted && !item.disabled) {
        (true, true) => (colors.destructive_foreground, Some(colors.destructive_highlight)),
        (true, false) => (colors.destructive_foreground, None),
        (false, true) => (colors.accent_foreground, Some(colors.accent)),
        (false, false) => (colors.foreground, None),
    };
    let fg = if item.disabled { colors.muted_foreground } else { fg };
    let icon_color = if item.destructive { fg } else { colors.muted_foreground };
    let icon = match item.checked {
        Some(true) => Some(Some(IconName::Check)),
        Some(false) => Some(None),
        None => item.icon.map(Some),
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        // One line: the row fills the slot its menu's grid gives it (`pixel_grid`).
        .h_full()
        .px(colors.r(8.))
        .rounded(colors.r(4.))
        .text_sm()
        .line_height(colors.r(20.))
        .text_color(fg)
        .when_some(bg, |row, bg| row.bg(bg))
        .when(item.disabled, |row| row.opacity(0.64))
        .when_some(icon, |row, icon| match icon {
            Some(icon) => row.child(Icon::new(icon).size(IconSize::Custom(colors.r(16.))).color(icon_color).stroke(2.)),
            None => row.child(div().size(colors.r(16.)).flex_none()),
        })
        .child(div().flex_1().min_w_0().truncate().child(item.label.clone()))
        .when_some(item.detail.clone(), |row, detail| {
            row.child(
                div()
                    .ml_auto()
                    .pl_4()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(detail),
            )
        })
        .when(!item.children.is_empty(), |row| {
            row.child(
                div().mr(colors.r(-2.)).child(
                    Icon::new(IconName::ChevronRight)
                        .size(IconSize::Custom(colors.r(16.)))
                        .color(colors.muted_foreground.opacity(0.8))
                        .stroke(2.),
                ),
            )
        })
}

/// The menu surface: `dropdown-glass`, radius 8px, 4px padding.
pub fn menu_surface(colors: &ShellColors) -> Div {
    div()
        .occlude()
        .flex()
        .flex_col()
        .min_w(colors.s(MENU_MIN_WIDTH))
        .max_w(colors.s(MENU_MAX_WIDTH))
        .p_1()
        .font_weight(FontWeight::NORMAL)
        .rounded(colors.r(8.))
        .border_1()
        .border_color(colors.glass_border)
        .bg(colors.glass)
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., if colors.dark { 0.8 } else { 0.55 }),
            offset: point(px(0.), px(if colors.dark { 18. } else { 16. })),
            blur_radius: px(if colors.dark { 44. } else { 40. }),
            spread_radius: px(-18.),
            inset: false,
        }])
}

/// The 1px separator (`margin 4px 8px`), its margins taken from the menu's grid.
fn menu_separator(colors: &ShellColors, grid: &mut PixelGrid) -> Div {
    let above = grid.next(colors.r(4.));
    let below = grid.next(colors.r(4.));
    div().mx(colors.r(8.)).mt(above).mb(below).h(px(1.)).flex_none().bg(colors.border)
}

/// The rows of an open menu and its open submenu, for any view that keeps a
/// [`ContextMenu`]. `menu_of` finds the menu on the view; `pick` runs a row
/// (top-level index, submenu index).
pub fn menu_body<V: 'static>(
    menu: &ContextMenu,
    colors: &ShellColors,
    window: &Window,
    menu_of: fn(&mut V) -> Option<&mut ContextMenu>,
    pick: fn(&mut V, usize, Option<usize>, &mut Window, &mut Context<V>),
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let row_height = colors.s(MENU_ROW_HEIGHT);
    let mut grid = PixelGrid::new(window);
    let top = grid.next(colors.r(4.));
    let mut rows: Vec<AnyElement> = Vec::new();
    for (ix, item) in menu.items.iter().enumerate() {
        if item.separator_before {
            rows.push(menu_separator(colors, &mut grid).into_any_element());
        }
        let slot = grid.next(row_height);
        let highlighted = menu.highlighted == Some(ix) || menu.submenu.is_some_and(|(open, _)| open == ix);
        let has_children = !item.children.is_empty();
        let mut row = div()
            .id(("menu-item", ix))
            .relative()
            .flex_none()
            .h(slot)
            .child(menu_row(item, highlighted, colors))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered {
                    return;
                }
                if let Some(menu) = menu_of(this) {
                    menu.highlighted = Some(ix);
                    menu.submenu = has_children.then_some((ix, None));
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                if has_children {
                    if let Some(menu) = menu_of(this) {
                        menu.submenu = Some((ix, None));
                        cx.notify();
                    }
                } else {
                    pick(this, ix, None, window, cx);
                }
            }));
        if let Some((open, child_highlight)) = menu.submenu
            && open == ix
        {
            let mut sub_grid = PixelGrid::new(window);
            let sub_top = sub_grid.next(colors.r(4.));
            let mut sub_rows: Vec<AnyElement> = Vec::new();
            for (cix, child) in item.children.iter().enumerate() {
                if child.separator_before {
                    sub_rows.push(menu_separator(colors, &mut sub_grid).into_any_element());
                }
                let slot = sub_grid.next(row_height);
                sub_rows.push(
                    div()
                        .id(("submenu-item", cix))
                        .flex_none()
                        .h(slot)
                        .child(menu_row(child, child_highlight == Some(cix), colors))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if let Some(menu) = menu_of(this)
                                && *hovered
                            {
                                menu.submenu = Some((ix, Some(cix)));
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| pick(this, ix, Some(cix), window, cx)))
                        .into_any_element(),
                );
            }
            let sub_bottom = sub_grid.next(colors.r(4.));
            let sub = menu_surface(colors).pt(sub_top).pb(sub_bottom).children(sub_rows).id("context-submenu");
            // `contextMenuFallback.ts`: the submenu opens at the trigger's right + 4px, at its top.
            row = row.child(deferred(div().absolute().left_full().top_0().pl(colors.r(4.)).child(sub)).with_priority(3));
        }
        rows.push(row.into_any_element());
    }
    let bottom = grid.next(colors.r(4.));
    menu_surface(colors).pt(top).pb(bottom).children(rows).id("context-menu")
}

/// The picked row's command, if it is enabled.
pub fn picked_command(menu: &ContextMenu, ix: usize, child: Option<usize>) -> Option<MenuCommand> {
    let item = match child {
        Some(c) => menu.items.get(ix).and_then(|i| i.children.get(c)),
        None => menu.items.get(ix),
    };
    item.filter(|i| !i.disabled).and_then(|i| i.command)
}

impl super::Sidebar {
    /// The open context menu, drawn above everything at the pointer.
    pub(crate) fn render_context_menu(
        &mut self,
        colors: &ShellColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.clone()?;
        let viewport = window.viewport_size();
        // Clamp 4px inside the window, like `contextMenuFallback.ts:202-214`.
        let height = menu_height(&menu.items, colors);
        let x = menu.position.x.min(viewport.width - MENU_MAX_WIDTH.min(px(240.)) - px(4.)).max(px(4.));
        let y = menu.position.y.min(viewport.height - height - px(4.)).max(px(4.));
        let surface = menu_body(&menu, colors, window, |this: &mut Self| this.context_menu.as_mut(), |this, ix, child, window, cx| this.run_menu_item(ix, child, window, cx), cx)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }));
        Some(
            deferred(anchored().position(point(x, y)).child(surface))
                .with_priority(2)
                .into_any_element(),
        )
    }
}
