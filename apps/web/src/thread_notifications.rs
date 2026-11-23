//! Notification settings, sounds, the per-thread notification tag and the
//! unread badge image. Ports `apps/web/src/threadNotifications.ts`.

use crate::i18n::t;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use t3code_contracts::NotificationMode;

use crate::components::sidebar::model::{EnvironmentId, ThreadId};
use crate::components::thread_notification_coordinator_logic::ThreadNotificationKind;

/// The client settings notifications read. Defaults match `settings.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationSettings {
    /// Thread notifications: off, system notifications, sound, or both.
    pub mode: NotificationMode,
    /// In-app toasts for other threads while the window has focus.
    pub in_app: bool,
    /// Fork: the bell and inbox in the sidebar footer.
    pub inbox_enabled: bool,
    /// Fork: the Dock and taskbar unread count.
    pub app_icon_badge: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self { mode: NotificationMode::Off, in_app: false, inbox_enabled: false, app_icon_badge: true }
    }
}

/// The settings label for a mode (`NOTIFICATION_MODE_LABELS`).
pub fn notification_mode_label(mode: NotificationMode) -> gpui_kit::SharedString {
    match mode {
        NotificationMode::Off => t!("Off", context = "notification setting, no notifications or sound"),
        NotificationMode::Notifications => t!("Notifications only"),
        NotificationMode::Sound => t!("Sound only"),
        NotificationMode::NotificationsAndSound => t!("Notifications with sound"),
    }
}

/// Whether the mode plays a sound.
pub fn has_notification_sound(mode: NotificationMode) -> bool {
    matches!(mode, NotificationMode::Sound | NotificationMode::NotificationsAndSound)
}

/// Whether the mode shows system notifications.
pub fn has_desktop_notifications(mode: NotificationMode) -> bool {
    matches!(mode, NotificationMode::Notifications | NotificationMode::NotificationsAndSound)
}

/// A short, stable tag per thread, so a newer notification replaces the
/// thread's last one (`threadNotificationTag`: 64-bit FNV-1a over UTF-16 units).
pub fn thread_notification_tag(environment_id: &EnvironmentId, thread_id: &ThreadId) -> String {
    let key = format!("{}:{}", environment_id.0, thread_id.0);
    let hash = key.encode_utf16().fold(0xcbf2_9ce4_8422_2325_u64, |hash, unit| {
        (hash ^ u64::from(unit)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// The unread count drawn as a 64x64 red disc (`renderBadgeImage`), for the
/// Windows taskbar overlay. Returns straight RGBA8 rows, or `None` for zero.
pub fn render_badge_image(count: usize) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    if count == 0 {
        return None;
    }
    let (label, size) = if count > 9 { ("9+".to_string(), 30) } else { (count.to_string(), 40) };
    // The overlay is an OS surface outside the theme; the colour is the web app's literal.
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
<circle cx="32" cy="32" r="28" fill="#e5484d"/>
<text x="32" y="34" font-family="Inter Variable" font-weight="600" font-size="{size}" fill="white"
 text-anchor="middle" dominant-baseline="central">{label}</text></svg>"##
    );
    let mut options = usvg::Options::default();
    if let Some(font) = crate::Assets::get("fonts/inter/inter-latin-wght-normal.ttf") {
        options.fontdb_mut().load_font_data(font.into_owned());
    }
    let tree = usvg::Tree::from_str(&svg, &options).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(64, 64)?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect(),
    )
}

const COMPLETION_SOUND: &[u8] = include_bytes!("../assets/sounds/notification-completion.mp3");
const INPUT_SOUND: &[u8] = include_bytes!("../assets/sounds/notification-input.mp3");

/// Plays the alert for `kind` on the audio thread (`playNotificationSound`).
/// A sound already playing is not restarted, so bursts do not clip; input and
/// completion alerts keep separate slots.
pub fn play_notification_sound(kind: ThreadNotificationKind) {
    static AUDIO: OnceLock<Option<Sender<ThreadNotificationKind>>> = OnceLock::new();
    let sender = AUDIO.get_or_init(|| {
        let (sender, receiver) = channel::<ThreadNotificationKind>();
        std::thread::Builder::new()
            .name("notification-sound".into())
            .spawn(move || {
                // Opened on the first alert, like the web app's unlocked AudioContext.
                let Ok(mut stream) = rodio::OutputStreamBuilder::open_default_stream() else {
                    eprintln!("notification sound: no audio output");
                    return;
                };
                stream.log_on_drop(false);
                let mut playing_until: HashMap<bool, Instant> = HashMap::new();
                for kind in receiver {
                    let completion = kind == ThreadNotificationKind::Completion;
                    if playing_until.get(&completion).is_some_and(|until| Instant::now() < *until) {
                        continue;
                    }
                    let bytes = if completion { COMPLETION_SOUND } else { INPUT_SOUND };
                    match rodio::Decoder::try_from(std::io::Cursor::new(bytes)) {
                        Ok(source) => {
                            let duration = rodio::Source::total_duration(&source)
                                .unwrap_or(Duration::from_millis(500));
                            stream.mixer().add(source);
                            playing_until.insert(completion, Instant::now() + duration);
                        }
                        Err(error) => eprintln!("notification sound: {error}"),
                    }
                }
            })
            .ok()
            .map(|_| sender)
    });
    if let Some(sender) = sender {
        let _ = sender.send(kind);
    }
}
