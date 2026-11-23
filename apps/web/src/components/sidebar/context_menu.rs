//! Web-styled context menus (fork: desktop draws the DOM fallback instead of a
//! native menu). Ports the item builders of `threadActionMenu.logic.ts` and
//! the project-folder menu in `Sidebar.tsx`, and the look of
//! `contextMenuFallback.ts:391-534`.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::model::{ProjectRef, ThreadKey};
use super::style::{MENU_MAX_WIDTH, MENU_MIN_WIDTH, MENU_ROW_HEIGHT, ShellColors};

/// What a menu row does when picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
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
            children: Vec::new(),
        }
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
}

/// `buildThreadActionMenuItems` for a sidebar row.
pub fn thread_menu_items(state: &ThreadMenuState<'_>) -> Vec<MenuItem> {
    use MenuCommand as C;
    let mut items = Vec::new();
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
pub fn menu_height(items: &[MenuItem]) -> Pixels {
    let separators = items.iter().filter(|i| i.separator_before).count() as f32;
    px(8. + items.len() as f32 * f32::from(MENU_ROW_HEIGHT) + separators * 9.)
}

/// One menu row's look (highlighted rows use `accent`; destructive ones a red tint).
pub fn menu_row(item: &MenuItem, highlighted: bool, colors: &ShellColors) -> Div {
    let (fg, bg) = match (item.destructive, highlighted && !item.disabled) {
        (true, true) => (colors.destructive_foreground, Some(colors.destructive_highlight)),
        (true, false) => (colors.destructive_foreground, None),
        (false, true) => (colors.accent_foreground, Some(colors.accent)),
        (false, false) => (colors.foreground, None),
    };
    let icon_color = if item.destructive { fg } else { colors.muted_foreground };
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_h(MENU_ROW_HEIGHT)
        .px_2()
        .py_1()
        .rounded(px(4.))
        .text_sm()
        .line_height(px(20.))
        .text_color(fg)
        .when_some(bg, |row, bg| row.bg(bg))
        .when(item.disabled, |row| row.opacity(0.64))
        .when_some(item.icon, |row, icon| row.child(Icon::new(icon).size(IconSize::Custom(px(16.))).color(icon_color).stroke(2.)))
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
            row.child(Icon::new(IconName::ChevronRight).size(IconSize::Custom(px(16.))).color(fg.opacity(0.8)).stroke(2.))
        })
}

/// The menu surface: `dropdown-glass`, radius 8px, 4px padding.
pub fn menu_surface(colors: &ShellColors) -> Div {
    div()
        .occlude()
        .flex()
        .flex_col()
        .min_w(MENU_MIN_WIDTH)
        .max_w(MENU_MAX_WIDTH)
        .p_1()
        .rounded(px(8.))
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

/// The 1px separator (`margin 4px 8px`).
pub fn menu_separator(colors: &ShellColors) -> Div {
    div().mx_2().my_1().h(px(1.)).bg(colors.border)
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
        let height = menu_height(&menu.items);
        let x = menu.position.x.min(viewport.width - MENU_MAX_WIDTH.min(px(240.)) - px(4.)).max(px(4.));
        let y = menu.position.y.min(viewport.height - height - px(4.)).max(px(4.));
        let mut surface = menu_surface(colors)
            .id("context-menu")
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }));
        for (ix, item) in menu.items.iter().enumerate() {
            if item.separator_before {
                surface = surface.child(menu_separator(colors));
            }
            let highlighted = menu.highlighted == Some(ix)
                || menu.submenu.is_some_and(|(open, _)| open == ix);
            let has_children = !item.children.is_empty();
            let mut row = div()
                .id(("menu-item", ix))
                .relative()
                .child(menu_row(item, highlighted, colors))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if !*hovered {
                        return;
                    }
                    if let Some(menu) = this.context_menu.as_mut() {
                        menu.highlighted = Some(ix);
                        menu.submenu = has_children.then_some((ix, None));
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if has_children {
                        if let Some(menu) = this.context_menu.as_mut() {
                            menu.submenu = Some((ix, None));
                            cx.notify();
                        }
                    } else {
                        this.run_menu_item(ix, None, cx);
                    }
                }));
            if let Some((open, child_highlight)) = menu.submenu
                && open == ix
            {
                let mut sub = menu_surface(colors).id("context-submenu");
                for (cix, child) in item.children.iter().enumerate() {
                    if child.separator_before {
                        sub = sub.child(menu_separator(colors));
                    }
                    sub = sub.child(
                        div()
                            .id(("submenu-item", cix))
                            .child(menu_row(child, child_highlight == Some(cix), colors))
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if let Some(menu) = this.context_menu.as_mut()
                                    && *hovered
                                {
                                    menu.submenu = Some((ix, Some(cix)));
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_menu_item(ix, Some(cix), cx);
                            })),
                    );
                }
                // Opens inline-end, aligned 5px above the trigger row.
                row = row.child(
                    deferred(div().absolute().left_full().top(px(-5.)).pl(px(8.)).child(sub))
                        .with_priority(3),
                );
            }
            surface = surface.child(row);
        }
        Some(
            deferred(anchored().position(point(x, y)).child(surface))
                .with_priority(2)
                .into_any_element(),
        )
    }
}
