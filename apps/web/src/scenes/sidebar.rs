//! The sidebar alone, four times over: the inbox layout, the shelves with a
//! settled page and the keyboard cursor, project folders, and the empty and
//! search-empty states. Together they put every row state on screen.

use gpui_kit::*;

use crate::ActiveAppTheme as _;
use crate::components::sidebar::fixtures;
use crate::components::sidebar::style::{SIDEBAR_DEFAULT_WIDTH, ShellColors};
use crate::components::sidebar::surface_grain::surface_grain;
use crate::components::sidebar::Sidebar;
use crate::components::sidebar_logic::{SidebarListEntry, Shelf};

/// Root view of the scene.
pub struct SidebarScene {
    columns: Vec<Entity<Sidebar>>,
}

/// Index of the first thread entry whose title starts with `prefix`.
fn entry_for(sidebar: &Sidebar, prefix: &str) -> Option<usize> {
    sidebar.list().entries.iter().position(|entry| match entry {
        SidebarListEntry::Thread { thread, .. } => thread.title.starts_with(prefix),
        _ => false,
    })
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    let inbox = cx.new(|cx| {
        let mut sidebar = Sidebar::new(fixtures::showcase_snapshot(), window, cx);
        sidebar.use_snapshot_settings(cx);
        let hovered = entry_for(&sidebar, "Fix hero image");
        sidebar.set_hovered(hovered, cx);
        sidebar
    });
    let shelves = cx.new(|cx| {
        // Fewer active rows, so the shelves sit at the bottom on screen.
        let mut snapshot = fixtures::showcase_snapshot();
        snapshot.threads.retain(|t| {
            let id = t.key.thread_id.0.as_ref();
            !id.starts_with('a') || ["a1", "a8", "a10"].contains(&id)
        });
        let mut sidebar = Sidebar::new(snapshot, window, cx);
        sidebar.use_snapshot_settings(cx);
        sidebar.options_mut(cx, |o| {
            o.settled_expanded = true;
        });
        let hovered = sidebar
            .list()
            .entries
            .iter()
            .position(|e| matches!(e, SidebarListEntry::ShelfHeader { shelf: Shelf::Settled, .. }))
            .map(|ix| ix + 2);
        sidebar.set_hovered(hovered, cx);
        sidebar
    });
    let folders = cx.new(|cx| {
        let mut sidebar = Sidebar::new(fixtures::folders_snapshot(), window, cx);
        sidebar.use_snapshot_settings(cx);
        let infra = sidebar
            .list()
            .groups
            .iter()
            .find(|g| g.label.as_ref() == "infra")
            .map(|g| g.key.clone());
        sidebar.options_mut(cx, |o| {
            o.collapsed_folders.extend(infra);
        });
        sidebar
    });
    let empty = cx.new(|cx| Sidebar::new(fixtures::empty_snapshot(), window, cx));
    let search_empty = cx.new(|cx| {
        let mut sidebar = Sidebar::new(fixtures::showcase_snapshot(), window, cx);
        sidebar.use_snapshot_settings(cx);
        sidebar.options_mut(cx, |o| o.search = "zzz".into());
        sidebar
    });
    // The keyboard cursor needs focus: put it on the shelves column.
    shelves.update(cx, |sidebar, cx| {
        let row = entry_for(sidebar, "Release checklist");
        sidebar.cursor = row;
        sidebar.keyboard_nav = true;
        window.focus(&sidebar.focus_handle, cx);
    });
    cx.new(|_| SidebarScene {
        columns: vec![inbox, shelves, folders, empty, search_empty],
    })
    .into()
}

impl Render for SidebarScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ShellColors::from_theme(cx.app_theme());
        let column = |content: AnyElement| {
            div()
                .relative()
                .w(SIDEBAR_DEFAULT_WIDTH + px(1.))
                .h_full()
                .flex_none()
                .bg(colors.sidebar)
                .border_r_1()
                .border_color(colors.sidebar_edge)
                .child(surface_grain())
                .child(content)
        };
        let [inbox, shelves, folders, empty, search_empty] = &self.columns[..] else {
            return div();
        };
        div()
            .size_full()
            .flex()
            .bg(colors.chrome)
            .child(column(inbox.clone().into_any_element()))
            .child(column(shelves.clone().into_any_element()))
            .child(column(folders.clone().into_any_element()))
            .child(
                div()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(div().h_1_2().child(column(empty.clone().into_any_element())))
                    .child(div().h_1_2().child(column(search_empty.clone().into_any_element()))),
            )
    }
}
