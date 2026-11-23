//! Windows: toast notifications through the WinRT notifier, and the unread
//! count as a taskbar overlay icon (`ITaskbarList3::SetOverlayIcon`). An
//! unpackaged app needs an AppUserModelID registered under HKCU for toasts to
//! show; it is registered at startup with the app's name and icon. Windows asks
//! for no permission; notifications can be turned off in Settings > System >
//! Notifications. Clicks on toasts left from an earlier run are not handled
//! (that needs a COM activator).

use std::cell::RefCell;

use futures::channel::mpsc::UnboundedSender;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{ITaskbarList3, SetCurrentProcessExplicitAppUserModelID, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, EnumThreadWindows, HICON, ICONINFO, IsWindowVisible,
};
use windows::core::{BOOL, HSTRING, PCWSTR, Result};

use super::{Activation, NativeBackend, NativeNotification};

const APP_ID: &str = "T3Code.Desktop";
const GROUP: &str = "t3-work";
const APP_ICON: &[u8] = include_bytes!("../../../../assets/prod/black-macos-1024.png");

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A toast whose launch argument carries the entry id (`windowsToastXml`).
fn toast_xml(notification: &NativeNotification) -> String {
    let text: String = std::iter::once(&notification.title)
        .chain(&notification.lines)
        .map(|line| format!("<text>{}</text>", escape_xml(line)))
        .collect();
    format!(
        r#"<toast launch="{}" activationType="foreground"><visual><binding template="ToastGeneric">{text}</binding></visual><audio silent="true"/></toast>"#,
        escape_xml(&notification.id)
    )
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

/// Registers the AppUserModelID with a display name and icon, so toasts show for
/// an unpackaged app, and makes this process use it.
fn register_app_id() -> Result<()> {
    let icon = dirs::data_dir().map(|dir| dir.join("t3code-gpui").join("notification-icon.png"));
    if let Some(icon) = &icon
        && !icon.exists()
        && let Ok(image) = image::load_from_memory(APP_ICON)
    {
        let _ = std::fs::create_dir_all(icon.parent().unwrap_or(icon));
        let _ = image.resize(128, 128, image::imageops::FilterType::Triangle).save(icon);
    }
    let path = wide(&format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    let mut key = HKEY::default();
    // SAFETY: every pointer is a live, NUL-terminated buffer or an out parameter on the stack.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let mut values = vec![("DisplayName", "T3 Code".to_string())];
        if let Some(icon) = icon {
            values.push(("IconUri", icon.display().to_string()));
        }
        for (name, value) in values {
            let name = wide(name);
            let value = wide(&value);
            let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
            let _ = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
        }
        let _ = RegCloseKey(key);
        SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID))
    }
}

/// The app's visible top-level windows (all on this, the UI, thread).
fn app_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, windows: LPARAM) -> BOOL {
        // SAFETY: `windows` is the address of the Vec below, alive for the enumeration.
        let windows = unsafe { &mut *(windows.0 as *mut Vec<HWND>) };
        // SAFETY: the handle comes from the enumeration.
        if unsafe { IsWindowVisible(window) }.as_bool() {
            windows.push(window);
        }
        TRUE
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback only touches the Vec passed as its parameter.
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(collect), LPARAM(&mut windows as *mut _ as isize));
    }
    windows
}

/// A 64px icon from straight RGBA (`renderBadgeImage`'s pixels).
fn icon_from_rgba(rgba: &[u8]) -> Result<HICON> {
    let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let mask = vec![0_u8; 64 * 64 / 8];
    // SAFETY: both buffers hold a full 64x64 bitmap at the stated depth.
    unsafe {
        let color = CreateBitmap(64, 64, 1, 32, Some(bgra.as_ptr().cast()));
        let mask = CreateBitmap(64, 64, 1, 1, Some(mask.as_ptr().cast()));
        let info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// The toast notifier and the taskbar.
pub struct WindowsBackend {
    notifier: ToastNotifier,
    activations: UnboundedSender<Activation>,
    taskbar: Option<ITaskbarList3>,
    overlay: RefCell<Option<HICON>>,
}

impl WindowsBackend {
    /// Registers the app id and opens the notifier. `None` if toasts are unavailable.
    pub fn new(activations: UnboundedSender<Activation>) -> Option<Self> {
        if let Err(error) = register_app_id() {
            eprintln!("notifications: could not register the app id: {error}");
        }
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
            .map_err(|error| eprintln!("notifications: no toast notifier: {error}"))
            .ok()?;
        // SAFETY: COM is initialized for this (UI) thread before creating the taskbar object.
        let taskbar = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
                .and_then(|list| list.HrInit().map(|()| list))
                .ok()
        };
        Some(Self { notifier, activations, taskbar, overlay: RefCell::new(None) })
    }

    fn try_show(&self, notification: NativeNotification) -> Result<()> {
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(toast_xml(&notification)))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        // Same tag, same group: a newer toast replaces the older one.
        toast.SetTag(&HSTRING::from(notification.id.as_str()))?;
        toast.SetGroup(&HSTRING::from(GROUP))?;
        let activations = self.activations.clone();
        let activation = notification.activation;
        toast.Activated(&TypedEventHandler::new(move |_, _| {
            let _ = activations.unbounded_send(activation.clone());
            Ok(())
        }))?;
        self.notifier.Show(&toast)
    }
}

impl NativeBackend for WindowsBackend {
    fn show(&self, notification: NativeNotification) {
        if let Err(error) = self.try_show(notification) {
            eprintln!("notifications: could not show: {error}");
        }
    }

    fn remove(&self, id: &str) {
        let removed = ToastNotificationManager::History().and_then(|history| {
            history.RemoveGroupedTagWithId(&HSTRING::from(id), &HSTRING::from(GROUP), &HSTRING::from(APP_ID))
        });
        // Removing a toast that is already gone fails harmlessly.
        let _ = removed;
    }

    fn set_badge(&self, count: usize) {
        let Some(taskbar) = &self.taskbar else { return };
        let icon = t3code_web::thread_notifications::render_badge_image(count)
            .and_then(|rgba| icon_from_rgba(&rgba).ok());
        let description = HSTRING::from(if count > 0 { format!("{count} unread notifications") } else { String::new() });
        for window in app_windows() {
            // SAFETY: the window belongs to this thread and the icon stays alive until replaced.
            let _ = unsafe { taskbar.SetOverlayIcon(window, icon.unwrap_or_default(), &description) };
        }
        if let Some(previous) = self.overlay.replace(icon) {
            // SAFETY: the taskbar copied the previous icon when it was set.
            let _ = unsafe { DestroyIcon(previous) };
        }
    }
}
