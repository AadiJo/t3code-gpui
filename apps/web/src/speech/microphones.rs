//! The microphones the system reports and the saved preference order (`browserMicrophones.ts`,
//! `microphoneOrder.ts`). The order holds exactly one System default row; voice input records
//! from the first entry that is connected.
//!
//! The browser names devices by opaque per-origin ids, which a native app cannot reproduce, so
//! an entry the fork saved matches a system device by its label: the same name, or the name
//! followed by the browser's parenthesised suffix ("MacBook Pro Microphone (Built-in)").
//! Devices this app remembers use the system name as both id and label.

use cpal::traits::{DeviceTrait as _, HostTrait as _};
use gpui_kit::App;
use t3code_contracts::{VoiceMicrophoneEntry, VoiceMicrophoneEntryDefault, VoiceMicrophoneEntryDevice};

use crate::hooks::use_settings::ClientSettingsStore;

/// One input device as the system reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemMicrophone {
    pub name: String,
}

/// The audio inputs the system reports now. Blocks briefly; call off the UI thread.
pub fn list_audio_inputs() -> Vec<SystemMicrophone> {
    #[cfg(feature = "test-support")]
    if let Some(source) = super::pcm_capture::test_source::current() {
        return source.devices.into_iter().map(|name| SystemMicrophone { name }).collect();
    }
    let Ok(devices) = cpal::default_host().input_devices() else { return Vec::new() };
    let mut seen = std::collections::HashSet::new();
    devices
        .filter_map(|device| device.name().ok())
        .filter(|name| seen.insert(name.clone()))
        .map(|name| SystemMicrophone { name })
        .collect()
}

/// `microphoneEntryKey`.
pub fn entry_key(entry: &VoiceMicrophoneEntry) -> String {
    match entry {
        VoiceMicrophoneEntry::Device(device) => format!("device:{}", device.device_id),
        _ => "default".to_owned(),
    }
}

fn system_default() -> VoiceMicrophoneEntry {
    VoiceMicrophoneEntry::Default(VoiceMicrophoneEntryDefault {})
}

/// Whether a saved entry names this system device.
pub fn entry_matches(entry: &VoiceMicrophoneEntryDevice, device: &SystemMicrophone) -> bool {
    let name = device.name.as_str();
    entry.device_id == name || entry.label == name || entry.label.strip_prefix(name).is_some_and(|rest| rest.starts_with(" ("))
}

/// `microphoneOrder`: the saved order with exactly one System default row; an empty order
/// starts from the legacy single choice.
pub fn microphone_order(saved: &[VoiceMicrophoneEntry], legacy: &str) -> Vec<VoiceMicrophoneEntry> {
    if saved.is_empty() {
        return if legacy.is_empty() {
            vec![system_default()]
        } else {
            vec![VoiceMicrophoneEntry::Device(VoiceMicrophoneEntryDevice { device_id: legacy.to_owned(), label: String::new() }), system_default()]
        };
    }
    let mut keys = std::collections::HashSet::new();
    let mut order: Vec<VoiceMicrophoneEntry> = saved
        .iter()
        .filter(|entry| !matches!(entry, VoiceMicrophoneEntry::Unknown(_)) && keys.insert(entry_key(entry)))
        .cloned()
        .collect();
    if !keys.contains("default") {
        order.push(system_default());
    }
    order
}

/// `rememberMicrophones`: devices seen for the first time go directly below System default.
/// `None` when nothing changed.
pub fn remember_microphones(order: &[VoiceMicrophoneEntry], devices: &[SystemMicrophone]) -> Option<Vec<VoiceMicrophoneEntry>> {
    let mut next: Option<Vec<VoiceMicrophoneEntry>> = None;
    let mut insert_at = order.iter().position(|entry| matches!(entry, VoiceMicrophoneEntry::Default(_))).map_or(0, |ix| ix + 1);
    for device in devices {
        let current = next.as_deref().unwrap_or(order);
        let known = current.iter().any(|entry| matches!(entry, VoiceMicrophoneEntry::Device(saved) if entry_matches(saved, device)));
        if known {
            continue;
        }
        let list = next.get_or_insert_with(|| order.to_vec());
        list.insert(
            insert_at,
            VoiceMicrophoneEntry::Device(VoiceMicrophoneEntryDevice { device_id: device.name.clone(), label: device.name.clone() }),
        );
        insert_at += 1;
    }
    next
}

/// `preferredMicrophone`: the first connected entry; System default always is.
pub fn preferred_microphone<'a>(order: &'a [VoiceMicrophoneEntry], devices: &[SystemMicrophone]) -> Option<&'a VoiceMicrophoneEntry> {
    order.iter().find(|entry| match entry {
        VoiceMicrophoneEntry::Device(saved) => devices.iter().any(|device| entry_matches(saved, device)),
        _ => true,
    })
}

/// The system device to record from; `None` is System default.
pub fn preferred_device(order: &[VoiceMicrophoneEntry], devices: &[SystemMicrophone]) -> Option<String> {
    match preferred_microphone(order, devices)? {
        VoiceMicrophoneEntry::Device(saved) => devices.iter().find(|device| entry_matches(saved, device)).map(|device| device.name.clone()),
        _ => None,
    }
}

/// `moveMicrophone`.
pub fn move_microphone(order: &[VoiceMicrophoneEntry], key: &str, steps: isize) -> Option<Vec<VoiceMicrophoneEntry>> {
    let from = order.iter().position(|entry| entry_key(entry) == key)?;
    let to = (from as isize + steps).clamp(0, order.len() as isize - 1) as usize;
    if from == to {
        return None;
    }
    let mut next = order.to_vec();
    let entry = next.remove(from);
    next.insert(to, entry);
    Some(next)
}

/// `forgetMicrophone`: removes a named microphone; System default stays.
pub fn forget_microphone(order: &[VoiceMicrophoneEntry], key: &str) -> Option<Vec<VoiceMicrophoneEntry>> {
    (key != "default").then(|| order.iter().filter(|entry| entry_key(entry) != key).cloned().collect())
}

/// The saved order as the settings hold it now.
pub fn current_order(cx: &App) -> Vec<VoiceMicrophoneEntry> {
    let settings = ClientSettingsStore::get(cx);
    microphone_order(&settings.voice_microphones, &settings.voice_microphone)
}

/// `updateMicrophoneOrder`: saves a changed order.
pub fn save_order(order: Vec<VoiceMicrophoneEntry>, cx: &mut App) {
    ClientSettingsStore::update(cx, |settings| settings.voice_microphones = order);
}
