//! The main window: client-drawn titlebar, the web app's default and minimum
//! sizes, and bounds plus maximized state restored from `window-state.json`
//! in the app's data directory. Ports the window parts of
//! `apps/desktop/src/window/DesktopWindow.ts` and `DesktopAppSettings.ts`.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{component::TitleBar, *};
use serde::{Deserialize, Serialize};
use t3code_web::app_root::{AppRootOptions, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::ui::motion::PrefersReducedMotion;
use t3code_web::theme_palette::ThemeSettings;
use t3code_web::ui_state_store::StateDir;

const DEFAULT_SIZE: Size<Pixels> = Size {
    width: px(1100.),
    height: px(780.),
};
const MIN_SIZE: Size<Pixels> = Size {
    width: px(400.),
    height: px(360.),
};
/// Saved bounds may overhang a display edge by this much and still count as on it.
const EDGE_TOLERANCE: f32 = 1.;
const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

/// What `window-state.json` holds (`mainWindowBounds`, `mainWindowMaximized`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedWindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl SavedWindowState {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(point(px(self.x), px(self.y)), size(px(self.width), px(self.height)))
    }
}

/// `<data dir>/t3code-gpui`: window bounds, layout, panels and UI state.
pub fn data_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("t3code-gpui"))
}

/// `<data dir>/t3code-gpui/window-state.json`.
fn state_path() -> Option<PathBuf> {
    Some(data_dir()?.join("window-state.json"))
}

fn load_state() -> Option<SavedWindowState> {
    let bytes = std::fs::read(state_path()?).ok()?;
    let state: SavedWindowState = serde_json::from_slice(&bytes).ok()?;
    (state.width >= f32::from(MIN_SIZE.width) && state.height >= f32::from(MIN_SIZE.height))
        .then_some(state)
}

fn save_state(state: &SavedWindowState) {
    let Some(path) = state_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let _ = std::fs::write(path, json);
    }
}

/// `resolveInitialMainWindowBounds`: saved bounds only if they fit on a display.
fn initial_bounds(cx: &App) -> WindowBounds {
    let fits = |bounds: Bounds<Pixels>| {
        cx.displays().iter().any(|display| {
            let d = display.bounds();
            let t = px(EDGE_TOLERANCE);
            bounds.origin.x >= d.origin.x - t
                && bounds.origin.y >= d.origin.y - t
                && bounds.right() <= d.right() + t
                && bounds.bottom() <= d.bottom() + t
        })
    };
    match load_state() {
        Some(state) if fits(state.bounds()) && state.maximized => {
            WindowBounds::Maximized(state.bounds())
        }
        Some(state) if fits(state.bounds()) => WindowBounds::Windowed(state.bounds()),
        _ => WindowBounds::centered(None, DEFAULT_SIZE, cx),
    }
}

/// Options for the main window.
pub fn main_window_options(cx: &App) -> WindowOptions {
    let mut options = TitleBar::window_options();
    if let Some(titlebar) = options.titlebar.as_mut() {
        // Traffic lights at x 16, centred on the 52px topbar (y = 52 / 2 - 7).
        titlebar.traffic_light_position = Some(point(px(16.), px(19.)));
    }
    WindowOptions {
        window_bounds: Some(initial_bounds(cx)),
        window_min_size: Some(MIN_SIZE),
        ..options
    }
}

/// Opens the main window and keeps its bounds saved. `fixtures` loads the
/// showcase data; without it the app starts empty (first-run state until an
/// environment is saved). Needs the connection runtime.
pub fn open_main_window(options: WindowOptions, fixtures: bool, cx: &mut App) -> anyhow::Result<(AnyWindowHandle, WindowRoot)> {
    let platform = TitlebarPlatform::current();
    let app = if fixtures { AppRootOptions::fixtures(platform) } else { AppRootOptions::without_fixtures(platform) };
    let app = AppRootOptions { state_dir: StateDir(data_dir()), ..app };
    follow_os_reduced_motion(cx);
    let (handle, root) = WindowRoot::open_window(options, app, cx)?;
    handle.update(cx, |_, window, cx| {
        ThemeSettings::follow_window(window, cx);
        root.app.update(cx, |app, cx| {
            app.start_clock(cx);
            persist_bounds(window, cx);
        });
    })?;
    Ok((handle, root))
}

/// Saves bounds 500ms after the last move, resize or maximize.
fn persist_bounds<T: 'static>(window: &mut Window, cx: &mut Context<T>) {
    let generation = Rc::new(Cell::new(0u64));
    cx.observe_window_bounds(window, move |_, window, cx| {
        let (bounds, maximized) = match window.window_bounds() {
            WindowBounds::Windowed(b) => (b, false),
            WindowBounds::Maximized(b) => (b, true),
            // Leaving fullscreen restores the last saved state.
            WindowBounds::Fullscreen(_) => return,
        };
        let state = SavedWindowState {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized,
        };
        let ticket = generation.get() + 1;
        generation.set(ticket);
        let generation = generation.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(PERSIST_DEBOUNCE).await;
            if generation.get() == ticket {
                save_state(&state);
            }
        })
        .detach();
    })
    .detach();
}

/// Reads the OS "reduce motion" setting off the main thread and applies it
/// as `prefers-reduced-motion` (Chromium reads the same settings).
fn follow_os_reduced_motion(cx: &mut App) {
    let read = cx.background_spawn(async { os_prefers_reduced_motion() });
    cx.spawn(async move |cx| {
        let reduce = read.await;
        cx.update(|cx| {
            cx.set_global(PrefersReducedMotion(reduce));
            cx.refresh_windows();
        });
    })
    .detach();
}

/// macOS: Accessibility > Display > Reduce motion.
#[cfg(target_os = "macos")]
fn os_prefers_reduced_motion() -> bool {
    std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "1")
}

/// Windows: "Show animations in Windows" (`SPI_GETCLIENTAREAANIMATION`).
#[cfg(target_os = "windows")]
fn os_prefers_reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
    let mut animate: i32 = 1;
    // SAFETY: the out pointer is a live BOOL for the duration of the call.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animate).cast(), 0) };
    ok != 0 && animate == 0
}

/// Linux: GNOME's `enable-animations`, which GTK and Chromium follow.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn os_prefers_reduced_motion() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "false")
}
