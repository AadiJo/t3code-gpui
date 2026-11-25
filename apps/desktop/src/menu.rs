//! The application menu and quit. Ports `DesktopApplicationMenu.ts` for a client with no
//! bundled server: the app menu (macOS), File, Edit, View, Window and Help, item by item with
//! the fork's shortcuts. Reload, Force Reload and the developer tools items are Electron's and
//! are left out; Speech and Delete have no GPUI role. Edit items use the OS roles, so they act
//! on whatever text field has focus.

use gpui_kit::*;
use t3code_web::AppRoot;
use t3code_web::app_root::{Minimize, OpenSettings, Quit, ToggleFullScreen, ZoomWindow};
use t3code_web::i18n::t;
use t3code_web::theme_palette::ThemeSettings;

actions!(
    desktop_menu,
    [
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        /// App menu → About: the system's About panel (Electron's `about` role).
        About,
        /// App menu → Check for Updates…: this build has no update feed, so it says so
        /// (`handleCheckForUpdatesMenuClick` with updates disabled).
        CheckForAppUpdates,
        /// App menu → Hide T3 Code.
        Hide,
        /// App menu → Hide Others.
        HideOthers,
        /// App menu → Show All.
        ShowAll,
        /// File → Close Window (macOS).
        CloseWindow,
        /// Edit → Paste as Text: the clipboard's text without formatting or files.
        PasteAsText,
        /// View → Actual Size.
        ActualSize,
        /// View → Zoom In.
        ZoomIn,
        /// View → Zoom Out.
        ZoomOut,
        /// Window → Bring All to Front (macOS).
        BringAllToFront,
    ]
);

/// The fork's menu accelerators. Role items (hide, minimize, full screen) take Electron's
/// default accelerators for their role.
fn bind_keys(cx: &mut App) {
    let mac = cfg!(target_os = "macos");
    let mut bindings = if mac {
        vec![
            KeyBinding::new("cmd-shift-v", PasteAsText, None),
            KeyBinding::new("cmd-0", ActualSize, None),
            KeyBinding::new("cmd-=", ZoomIn, None),
            KeyBinding::new("cmd--", ZoomOut, None),
        ]
    } else {
        vec![
            KeyBinding::new("ctrl-shift-v", PasteAsText, None),
            KeyBinding::new("ctrl-0", ActualSize, None),
            KeyBinding::new("ctrl-=", ZoomIn, None),
            KeyBinding::new("ctrl--", ZoomOut, None),
        ]
    };
    if mac {
        bindings.extend([
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        ]);
    } else {
        bindings.push(KeyBinding::new("f11", ToggleFullScreen, None));
    }
    cx.bind_keys(bindings);
}

/// The page zoom step (`webContents.setZoomLevel` ± 0.5, a factor of 1.2 per level).
#[derive(Default)]
struct PageZoom {
    level: i32,
    /// The sizes at level 0, captured on the first zoom.
    base: Option<t3code_web::appearance_fonts::FontSettings>,
}

impl Global for PageZoom {}

/// Scales every font size (and so every rem-based size) by the zoom step.
fn zoom(step: Option<i32>, cx: &mut App) {
    let current = cx.global::<ThemeSettings>().fonts.clone();
    let zoom = cx.default_global::<PageZoom>();
    let base = zoom.base.get_or_insert(current).clone();
    zoom.level = match step {
        Some(step) => (zoom.level + step).clamp(-6, 6),
        None => 0,
    };
    let factor = 1.2_f32.powf(zoom.level as f32 * 0.5);
    ThemeSettings::update(cx, |settings| {
        settings.fonts.interface = base.interface * factor;
        settings.fonts.prompt = base.prompt * factor;
        settings.fonts.code = base.code * factor;
        settings.fonts.terminal = base.terminal * factor;
    });
}

/// Sets the menus and makes Quit write state before exiting, whether it
/// comes from the menu, the quit shortcut (after its confirmation) or the last window closing.
pub fn install(root: Entity<AppRoot>, cx: &mut App) {
    let quit_root = root.downgrade();
    cx.on_action(move |_: &Quit, cx| {
        if let Some(root) = quit_root.upgrade() {
            root.update(cx, |root, cx| root.flush_state(cx));
        }
        cx.quit();
    });
    cx.on_action(|_: &About, cx| show_about_panel(cx));
    cx.on_action(|_: &CheckForAppUpdates, cx| {
        prompt_info(
            t!("Automatic updates are not available right now."),
            // `getAutoUpdateDisabledReason` without an update feed.
            "Automatic updates are not available because no update feed is configured.".into(),
            cx,
        )
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            _ = window.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &ActualSize, cx| zoom(None, cx));
    cx.on_action(|_: &ZoomIn, cx| zoom(Some(1), cx));
    cx.on_action(|_: &ZoomOut, cx| zoom(Some(-1), cx));
    // `pasteAndMatchStyle`: the focused field pastes the clipboard's text alone. The field's own
    // paste runs on a text-only clipboard, which then gets its images and files back.
    cx.on_action(|_: &PasteAsText, cx| {
        let Some(item) = cx.read_from_clipboard() else { return };
        let Some(text) = item.text() else { return };
        let Some(window) = cx.active_window() else { return };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        let paste = gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") { "cmd-v" } else { "ctrl-v" }).expect("static keystroke");
        _ = window.update(cx, |_, window, cx| {
            window.dispatch_keystroke(paste, cx);
        });
        cx.write_to_clipboard(item);
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
    bind_keys(cx);
    cx.set_menus(menus());
}

/// An information box with an OK button on the active window (`showMessageBox` type info).
fn prompt_info(message: gpui_kit::SharedString, detail: gpui_kit::SharedString, cx: &mut App) {
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else { return };
    _ = window.update(cx, |_, window, cx| {
        let answer = window.prompt(gpui_kit::PromptLevel::Info, &message, Some(&detail), &[gpui_kit::PromptButton::Ok(t!("OK"))], cx);
        cx.background_executor().spawn(async move { _ = answer.await }).detach();
    });
}

/// Electron's `about` role: the standard About panel on macOS, the app's name and version
/// elsewhere.
fn show_about_panel(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        if let Some(mtm) = MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            // SAFETY: an AppKit method on the shared application, on the main thread.
            unsafe {
                let _: () = objc2::msg_send![&*app, orderFrontStandardAboutPanel: Option::<&objc2::runtime::AnyObject>::None];
            }
            return;
        }
    }
    prompt_info("T3 Code".into(), format!("Version {}", env!("CARGO_PKG_VERSION")).into(), cx);
}

fn menus() -> Vec<Menu> {
    let app_name = "T3 Code";
    let mac = cfg!(target_os = "macos");
    let mut menus = Vec::new();
    if mac {
        menus.push(Menu {
            name: app_name.into(),
            items: vec![
                MenuItem::action(t!("About {appName}", appName = app_name), About),
                MenuItem::action(t!("Check for Updates..."), CheckForAppUpdates),
                MenuItem::separator(),
                MenuItem::action(t!("Settings..."), OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu(t!("Services", context = "macOS app menu item"), SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(t!("Hide {appName}", appName = app_name), Hide),
                MenuItem::action(t!("Hide Others"), HideOthers),
                MenuItem::action(t!("Show All", context = "macOS app menu item, shows hidden apps"), ShowAll),
                MenuItem::separator(),
                MenuItem::action(t!("Quit {appName}", appName = app_name), Quit),
            ],
            disabled: false,
        });
    }
    let mut file = Vec::new();
    if !mac {
        file.push(MenuItem::action(t!("Settings..."), OpenSettings));
        file.push(MenuItem::separator());
    }
    // Cmd/Ctrl+W belongs to closing the right panel's tab, so Close Window has no shortcut.
    file.push(if mac {
        MenuItem::action(t!("Close Window"), CloseWindow)
    } else {
        MenuItem::action(if cfg!(target_os = "windows") { t!("Exit", context = "File menu item, quits the app") } else { t!("Quit") }, Quit)
    });
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
            MenuItem::action(t!("Paste as Text"), PasteAsText),
            MenuItem::separator(),
            MenuItem::os_action(t!("Select All", context = "Edit menu item"), SelectAll, OsAction::SelectAll),
        ],
        disabled: false,
    });
    menus.push(Menu {
        name: t!("View", context = "menu bar title"),
        items: vec![
            MenuItem::action(t!("Actual Size"), ActualSize),
            MenuItem::action(t!("Zoom In"), ZoomIn),
            MenuItem::action(t!("Zoom Out"), ZoomOut),
            MenuItem::separator(),
            MenuItem::action(t!("Toggle Full Screen"), ToggleFullScreen),
        ],
        disabled: false,
    });
    let mut window = vec![
        MenuItem::action(t!("Minimize", context = "Window menu item"), Minimize),
        MenuItem::action(t!("Zoom", context = "Window menu item, resizes the window"), ZoomWindow),
    ];
    if mac {
        window.push(MenuItem::separator());
        window.push(MenuItem::action(t!("Bring All to Front"), BringAllToFront));
    }
    menus.push(Menu { name: t!("Window", context = "menu bar title"), items: window, disabled: false });
    menus.push(Menu {
        name: t!("Help", context = "menu bar title"),
        items: vec![MenuItem::action(t!("Check for Updates..."), CheckForAppUpdates)],
        disabled: false,
    });
    menus
}
