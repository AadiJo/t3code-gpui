//! Linux: freedesktop notifications and the Unity launcher badge over the
//! session D-Bus. A worker thread owns the calls so the UI never waits on the
//! bus; a second thread listens for clicks (the `default` action).

use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

use super::{Activation, NativeBackend, NativeNotification};

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The desktop entry the launcher badge belongs to (`app.setBadgeCount`).
const DESKTOP_ENTRY: &str = "application://t3code.desktop";
const APP_ICON: &[u8] = include_bytes!("../../../../assets/prod/black-macos-1024.png");

enum Command {
    Show(NativeNotification),
    Remove(String),
    Badge(usize),
}

/// Server ids by our id, and what each server id opens.
#[derive(Default)]
struct Shown {
    ids: HashMap<String, u32>,
    activations: HashMap<u32, Activation>,
}

/// The session bus notification service.
pub struct LinuxBackend {
    commands: Sender<Command>,
}

/// The app icon as `image-data` (`iiibiiay`): 64px RGBA.
fn icon_data() -> Option<Value<'static>> {
    let image = image::load_from_memory(APP_ICON).ok()?.resize(64, 64, image::imageops::FilterType::Triangle).into_rgba8();
    let (width, height) = image.dimensions();
    Some(Value::from((width as i32, height as i32, (width * 4) as i32, true, 8_i32, 4_i32, image.into_raw())))
}

impl LinuxBackend {
    /// Connects to the session bus. `None` when there is no bus.
    pub fn new(activations: UnboundedSender<Activation>) -> Option<Self> {
        let connection = Connection::session().map_err(|error| eprintln!("notifications: no session bus: {error}")).ok()?;
        let shown = Arc::new(Mutex::new(Shown::default()));
        let listener = connection.clone();
        let listening = shown.clone();
        std::thread::Builder::new()
            .name("notification-clicks".into())
            .spawn(move || {
                let Ok(proxy) = Proxy::new(&listener, DESTINATION, PATH, DESTINATION) else { return };
                let Ok(signals) = proxy.receive_signal("ActionInvoked") else { return };
                for signal in signals {
                    let Ok((id, action)) = signal.body().deserialize::<(u32, String)>() else { continue };
                    if action != "default" {
                        continue;
                    }
                    let activation = listening.lock().ok().and_then(|shown| shown.activations.get(&id).cloned());
                    if let Some(activation) = activation {
                        let _ = activations.unbounded_send(activation);
                    }
                }
            })
            .ok()?;
        let (commands, receiver) = channel();
        std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || {
                let icon = icon_data();
                let Ok(proxy) = Proxy::new(&connection, DESTINATION, PATH, DESTINATION) else { return };
                for command in receiver {
                    match command {
                        Command::Show(notification) => {
                            let replaces = shown.lock().ok().and_then(|s| s.ids.get(&notification.id).copied()).unwrap_or(0);
                            let mut hints: HashMap<&str, Value> = HashMap::new();
                            if let Some(icon) = &icon {
                                hints.insert("image-data", icon.try_clone().unwrap_or(Value::from(0)));
                            }
                            hints.insert("desktop-entry", Value::from("t3code"));
                            let body = notification.lines.join("\n");
                            let actions = ["default", "Open"];
                            let result: zbus::Result<u32> = proxy.call(
                                "Notify",
                                &("T3 Code", replaces, "", notification.title.as_str(), body.as_str(), &actions[..], hints, -1_i32),
                            );
                            match result {
                                Ok(id) => {
                                    if let Ok(mut shown) = shown.lock() {
                                        shown.ids.insert(notification.id, id);
                                        shown.activations.insert(id, notification.activation);
                                    }
                                }
                                Err(error) => eprintln!("notifications: could not show: {error}"),
                            }
                        }
                        Command::Remove(id) => {
                            let server_id = shown.lock().ok().and_then(|mut s| {
                                let server_id = s.ids.remove(&id)?;
                                s.activations.remove(&server_id);
                                Some(server_id)
                            });
                            if let Some(server_id) = server_id {
                                let _: zbus::Result<()> = proxy.call("CloseNotification", &(server_id,));
                            }
                        }
                        Command::Badge(count) => {
                            let mut properties: HashMap<&str, Value> = HashMap::new();
                            properties.insert("count", Value::from(count as i64));
                            properties.insert("count-visible", Value::from(count > 0));
                            let sent = connection.emit_signal(
                                None::<&str>,
                                "/com/canonical/unity/launcherentry/1",
                                "com.canonical.Unity.LauncherEntry",
                                "Update",
                                &(DESKTOP_ENTRY, properties),
                            );
                            if let Err(error) = sent {
                                eprintln!("notifications: could not set the badge: {error}");
                            }
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { commands })
    }
}

impl NativeBackend for LinuxBackend {
    fn show(&self, notification: NativeNotification) {
        let _ = self.commands.send(Command::Show(notification));
    }

    fn remove(&self, id: &str) {
        let _ = self.commands.send(Command::Remove(id.to_string()));
    }

    fn set_badge(&self, count: usize) {
        let _ = self.commands.send(Command::Badge(count));
    }
}
