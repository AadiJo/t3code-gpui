//! The application menu and quit. Ports `DesktopApplicationMenu.ts`: the app
//! menu (macOS), File, Edit, View and Window, with the web app's items bound
//! to the same actions the shortcuts use. Edit items use the OS roles, so
//! they act on whatever text field has focus.

use gpui_kit::*;
use t3code_web::AppRoot;
use t3code_web::app_root::{
    Minimize, NavigateBack, NavigateForward, NewThread, OpenSettings, Quit, ToggleFullScreen, ToggleRightPanel,
    ToggleSidebar, ToggleTerminal, ZoomWindow,
};
use t3code_web::i18n::t;

actions!(desktop_menu, [Undo, Redo, Cut, Copy, Paste, SelectAll]);

/// Sets the menus and makes Quit write state before exiting, whether it
/// comes from the menu, mod+q or the last window closing.
pub fn install(handle: AnyWindowHandle, root: Entity<AppRoot>, cx: &mut App) {
    let quit_root = root.downgrade();
    cx.on_action(move |_: &Quit, cx| {
        if let Some(root) = quit_root.upgrade() {
            root.update(cx, |root, cx| root.flush_state(cx));
        }
        cx.quit();
    });
    let close_root = root.downgrade();
    cx.on_window_closed(move |cx, _| {
        if cx.windows().is_empty() {
            if let Some(root) = close_root.upgrade() {
                root.update(cx, |root, cx| root.flush_state(cx));
            }
            cx.quit();
        }
    })
    .detach();
    let _ = handle;
    cx.set_menus(menus());
}

fn menus() -> Vec<Menu> {
    let app_name = "T3 Code";
    let mut menus = Vec::new();
    if cfg!(target_os = "macos") {
        menus.push(Menu {
            name: app_name.into(),
            items: vec![
                MenuItem::action(t!("Settings..."), OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu(t!("Services", context = "macOS app menu item"), SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(t!("Quit {appName}", appName = app_name), Quit),
            ],
            disabled: false,
        });
    }
    let mut file = Vec::new();
    if !cfg!(target_os = "macos") {
        file.push(MenuItem::action(t!("Settings..."), OpenSettings));
        file.push(MenuItem::separator());
    }
    file.push(MenuItem::action(t!("New thread"), NewThread));
    file.push(MenuItem::separator());
    file.push(MenuItem::action(
        if cfg!(target_os = "windows") { t!("Exit", context = "File menu item, quits the app") } else { t!("Quit") },
        Quit,
    ));
    menus.push(Menu { name: t!("File", context = "menu bar title"), items: file, disabled: false });
    menus.push(Menu {
        name: t!("Edit", context = "menu bar title"),
        items: vec![
            MenuItem::os_action(t!("Undo", context = "Edit menu item"), Undo, OsAction::Undo),
            MenuItem::os_action(t!("Redo", context = "Edit menu item"), Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t!("Cut", context = "Edit menu item"), Cut, OsAction::Cut),
            MenuItem::os_action(t!("Copy", context = "Edit menu item"), Copy, OsAction::Copy),
            MenuItem::os_action(t!("Paste", context = "Edit menu item"), Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action(t!("Select All", context = "Edit menu item"), SelectAll, OsAction::SelectAll),
        ],
        disabled: false,
    });
    menus.push(Menu {
        name: t!("View", context = "menu bar title"),
        items: vec![
            MenuItem::action(t!("Toggle Sidebar"), ToggleSidebar),
            MenuItem::action(t!("Toggle terminal drawer"), ToggleTerminal),
            MenuItem::action(t!("Toggle right panel"), ToggleRightPanel),
            MenuItem::separator(),
            MenuItem::action(t!("Back"), NavigateBack),
            MenuItem::action(t!("Forward", context = "browser navigation"), NavigateForward),
            MenuItem::separator(),
            MenuItem::action(t!("Toggle Full Screen"), ToggleFullScreen),
        ],
        disabled: false,
    });
    menus.push(Menu {
        name: t!("Window", context = "menu bar title"),
        items: vec![MenuItem::action(t!("Minimize", context = "Window menu item"), Minimize),
            MenuItem::action(t!("Zoom", context = "Window menu item, resizes the window"), ZoomWindow),
        ],
        disabled: false,
    });
    menus
}
