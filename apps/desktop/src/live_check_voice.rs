//! `live-check voice`: dictation end to end against a server whose speech service is a
//! stand-in (it speaks the real voice routes and socket; its transcript names the duration and
//! pitch of the audio it received). A stand-in microphone feeds a 48 kHz 440 Hz tone at the
//! capture boundary, so the meter, the resampler and the upload all run for real.
//!
//! Settings → Voice: the system's microphones are remembered into the order, "Transcribe with"
//! picks the environment, its model's language row appears, and the microphone test records,
//! meters and plays back. The composer: the mic appears only once the destination is ready; the
//! button and `mod+shift+d` start and finish; the live text streams; the transcript lands after
//! what was typed; batch upload when streaming is off; a 600 ms hold finishes on release in
//! Auto; Escape cancels; Send during recording sends with the transcript; the global indicator
//! finishes a recording whose draft is off screen; a refused microphone shows "Mic blocked"
//! and Record again recovers; Off hides the mic. Run `seed` first.

use std::sync::Arc;
use std::time::Duration;

use t3code_client_runtime::voice_input::VoiceInputPhase;
use t3code_web::components::settings::microphone_test::{ShownMicrophoneTest, TestPhase};
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::hooks::use_settings::ClientSettingsStore;
use t3code_web::router::Route;
use t3code_web::speech::pcm_capture::test_source::{self, TestSource};
use t3code_web::speech::voice_input_session::VoiceInput;

use super::*;

const DEVICES: [&str; 2] = ["Desk microphone", "USB microphone"];

pub(super) fn source(deny: bool) {
    let rate = 48_000u32;
    let samples: Arc<[f32]> = (0..rate as usize * 60).map(|i| 0.3 * (2. * std::f32::consts::PI * 440. * i as f32 / rate as f32).sin()).collect();
    test_source::set(Some(TestSource { rate, samples, devices: DEVICES.iter().map(|d| (*d).to_owned()).collect(), deny }));
}

pub(super) fn element_visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}

pub(super) fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("no visible element {id}"))?;
    super::click_at(check, at)
}

/// Notes the bounds of `ids` for the fork comparison.
pub(super) fn note_bounds(check: &mut Check, ids: &[&str]) {
    for id in ids {
        let element = ElementId::from(SharedString::from((*id).to_owned()));
        let bounds = check.cx.update_window(check.window, |_, w, _| w.find_all(element).iter().map(|e| e.bounds()).collect::<Vec<_>>()).unwrap_or_default();
        check.note(format!("bounds {id}: {bounds:?}"));
    }
}

pub(super) fn phase(check: &mut Check) -> VoiceInputPhase {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).voice_state(cx).phase)
}

pub(super) fn prompt(check: &mut Check) -> String {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned())
}

pub(super) fn set_prompt(check: &mut Check, text: &str) {
    let composer = composer_of(check);
    let text = text.to_owned();
    check.cx.update(|cx| composer.read(cx).editor().clone().update(cx, |e, cx| e.set_text(text, cx)));
    check.frame();
}

fn preview(check: &mut Check) -> Option<String> {
    let composer = composer_of(check);
    check.cx.update(|cx| {
        let owner = composer.read(cx).voice_owner();
        let voice = VoiceInput::try_global(cx)?;
        voice.read(cx).activity_for(&owner)?.preview.as_ref().map(|p| format!("{}{}", p.committed, p.tentative))
    })
}

pub(super) fn server_log(check: &Check) -> String {
    let path = std::env::var("T3_VOICE_SERVER_LOG").unwrap_or_default();
    let _ = check;
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Whether the transcript looks like the stand-in's report of the tone.
/// Short clips round the zero-crossing estimate by up to 20 Hz.
pub(super) fn heard_tone(text: &str) -> bool {
    let hertz = text.split(" at ").nth(1).and_then(|rest| rest.split(' ').next()).and_then(|n| n.parse::<u32>().ok());
    text.contains("stand-in heard") && hertz.is_some_and(|hz| (420..=460).contains(&hz))
}

fn record_for(check: &mut Check, start: &str, seconds: f64) -> anyhow::Result<()> {
    match start {
        "button" => click(check, "composer-voice")?,
        _ => check.press(start),
    }
    check.pump(|c| phase(c) == VoiceInputPhase::Recording, Duration::from_secs(20), "recording")?;
    check.wait(Duration::from_secs_f64(seconds));
    Ok(())
}

pub(super) fn run_voice(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    source(false);
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    // Start from Off with the streaming default.
    check.cx.update(|cx| {
        t3code_web::speech::voice_consent::choose_audio_destination(None, cx);
        ClientSettingsStore::update(cx, |s| {
            s.speech_streaming = true;
            s.voice_shortcut_mode = t3code_contracts::VoiceShortcutMode::Auto;
            s.voice_microphones = Vec::new();
        });
    });
    let key = thread_with_turn(check, &state, "Voice")?;
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    check.frame();
    let hidden = !element_visible(check, "composer-voice");
    check.check("with transcription Off the composer shows no mic", hidden, "hidden");
    check.focus_composer();
    let before = prompt(check);
    check.press("mod-shift-d");
    let (after, text) = (phase(check), prompt(check));
    check.check("and mod+shift+d leaves the key alone", after == VoiceInputPhase::Idle && text == before, format!("{after:?}"));

    // ---- Settings → Voice ----
    let root = check.root.clone();
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |router, cx| router.navigate(Route::settings(SettingsPath::Voice), cx));
    });
    check.pump(
        |c| c.cx.update(|cx| ClientSettingsStore::get(cx).voice_microphones.len() >= 3),
        Duration::from_secs(10),
        "system microphones listed",
    )?;
    let order: Vec<String> = check.cx.update(|cx| {
        ClientSettingsStore::get(cx)
            .voice_microphones
            .iter()
            .map(|e| match e {
                t3code_contracts::VoiceMicrophoneEntry::Device(d) => d.label.clone(),
                _ => "System default".into(),
            })
            .collect()
    });
    check.check(
        "new microphones are remembered below System default",
        order == ["System default", DEVICES[0], DEVICES[1]],
        format!("{order:?}"),
    );
    check.wait(Duration::from_millis(600));
    check.shot("v0-settings-off.png")?;
    super::live_check_settings::click_id(check, "voice-destination")?;
    let id = format!("voice-destination-{environment_id}");
    check.pump(move |c| element_visible(c, &id), Duration::from_secs(5), "destination menu open")?;
    check.shot("v1-destination-menu.png")?;
    click(check, &format!("voice-destination-{environment_id}"))?;
    check.pump(
        |c| c.cx.update(|cx| VoiceInput::destination(cx).is_some_and(|d| d.ready)),
        Duration::from_secs(10),
        "destination chosen and ready",
    )?;
    check.pump(
        |c| c.cx.update(|cx| VoiceInput::try_global(cx).and_then(|v| v.read(cx).status(cx)).is_some()),
        Duration::from_secs(10),
        "model status probed",
    )?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v2-settings-environment.png")?;

    // Microphone test: record, meter, stop early, play back.
    let test = check.cx.update(|cx| cx.try_global::<ShownMicrophoneTest>().and_then(|t| t.0.upgrade())).ok_or_else(|| anyhow::anyhow!("microphone test"))?;
    super::live_check_settings::click_id(check, "voice-test-record")?;
    let t = test.clone();
    check.pump(move |c| c.cx.update(|cx| t.read(cx).phase() == TestPhase::Recording && t.read(cx).level() > 0.05), Duration::from_secs(10), "meter moving")?;
    check.wait(Duration::from_millis(600));
    let level = check.cx.update(|cx| test.read(cx).level());
    check.check("the microphone test meters the tone", (0.5..=1.0).contains(&level), format!("level {level:.3}"));
    check.wait(Duration::from_millis(1000));
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v3-mic-test-recording.png")?;
    click(check, "voice-test-stop")?;
    let t = test.clone();
    check.pump(move |c| c.cx.update(|cx| t.read(cx).phase() == TestPhase::Ready), Duration::from_secs(10), "test recording ready")?;
    let seconds = check.cx.update(|cx| test.read(cx).recorded_seconds()).unwrap_or(0.);
    check.check("Stop keeps what was recorded", (1.2..=4.0).contains(&seconds), format!("{seconds:.2} s"));
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v4-mic-test-ready.png")?;
    click(check, "microphone-test-play")?;
    check.wait(Duration::from_millis(300));
    click(check, "microphone-test-close")?;
    let phase_after = check.cx.update(|cx| test.read(cx).phase());
    check.check("the X discards the recording", phase_after == TestPhase::Idle, format!("{phase_after:?}"));

    // ---- Composer: streaming ----
    check.go(&key);
    check.pump(|c| element_visible(c, "composer-voice"), Duration::from_secs(10), "mic button shown")?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v5-mic-idle.png")?;
    note_bounds(check, &["composer-voice", "composer-send"]);
    check.focus_composer();
    set_prompt(check, "");
    record_for(check, "button", 2.6)?;
    check.pump(|c| preview(c).is_some_and(|p| p.strip_prefix("heard ").and_then(|r| r.split(' ').next()).and_then(|n| n.parse::<f64>().ok()).is_some_and(|n| n >= 2.)), Duration::from_secs(10), "live text")?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v6-recording.png")?;
    note_bounds(check, &["composer-voice-cancel", "composer-voice-finish", "composer-send"]);
    let typed_during = prompt(check);
    check.check("the editor is frozen while recording", typed_during.is_empty(), format!("{typed_during:?}"));
    check.press("mod-shift-d");
    check.pump(|c| phase(c) == VoiceInputPhase::Idle, Duration::from_secs(20), "streamed transcript")?;
    let text = prompt(check);
    check.check("mod+shift+d finishes and the transcript lands in the draft", text.starts_with("stand-in heard") && heard_tone(&text), format!("{text:?}"));
    let streamed = server_log(check).contains("\"kind\":\"stream-finish\"");
    check.check("the audio streamed over the socket", streamed, "stream-finish logged");
    check.shot("v7-transcribed.png")?;

    // ---- Batch upload ----
    check.cx.update(|cx| ClientSettingsStore::update(cx, |s| s.speech_streaming = false));
    set_prompt(check, "Batch:");
    let batches = server_log(check).matches("\"kind\":\"batch\"").count();
    record_for(check, "mod-shift-d", 1.6)?;
    let no_preview = preview(check).is_none();
    check.check("no live text without streaming", no_preview, "none");
    click(check, "composer-voice-finish")?;
    check.pump(|c| phase(c) == VoiceInputPhase::Idle, Duration::from_secs(20), "batch transcript")?;
    let text = prompt(check);
    let uploaded = server_log(check).matches("\"kind\":\"batch\"").count() == batches + 1;
    check.check(
        "Finish uploads the recording to /api/voice/transcribe and the transcript follows the typed text after a space",
        text.starts_with("Batch: stand-in heard") && heard_tone(&text) && uploaded,
        format!("{text:?}"),
    );

    // ---- Hold in Auto mode ----
    set_prompt(check, "");
    check.focus_composer();
    let keystroke = Keystroke::parse(if cfg!(target_os = "macos") { "cmd-shift-d" } else { "ctrl-shift-d" })?;
    let down = keystroke.clone();
    check.cx.update_window(check.window, |_, w, cx| w.dispatch_keystroke(down, cx)).ok();
    check.pump(|c| phase(c) == VoiceInputPhase::Recording, Duration::from_secs(20), "recording while held")?;
    check.wait(Duration::from_millis(700));
    check.cx.update_window(check.window, |_, w, cx| w.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx)).ok();
    check.pump(|c| phase(c) == VoiceInputPhase::Idle, Duration::from_secs(20), "released hold transcribed")?;
    let text = prompt(check);
    check.check("a hold of 300 ms or more finishes on release", heard_tone(&text), format!("{text:?}"));

    // ---- Escape cancels ----
    set_prompt(check, "keep me");
    record_for(check, "mod-shift-d", 0.8)?;
    check.press("escape");
    check.pump(|c| phase(c) == VoiceInputPhase::Idle, Duration::from_secs(5), "cancelled")?;
    check.wait(Duration::from_millis(500));
    let text = prompt(check);
    check.check("Escape cancels and leaves the draft alone", text == "keep me", format!("{text:?}"));

    // ---- Send during recording ----
    set_prompt(check, "Send this:");
    let sent_before = check.shown_input().entries.len();
    record_for(check, "mod-shift-d", 1.2)?;
    check.press("enter");
    check.pump(|c| c.shown_input().entries.len() > sent_before, Duration::from_secs(30), "message sent after the transcript")?;
    let last_user = check.shown_input().entries.iter().rev().find_map(|entry| match &entry.kind {
        t3code_web::components::chat::timeline_entries::EntryKind::Message(m)
            if m.role == t3code_web::components::chat::timeline_entries::MessageRole::User =>
        {
            Some(m.text.to_string())
        }
        _ => None,
    });
    let last_user = last_user.unwrap_or_default();
    check.check("Send during recording sends with the transcript", last_user.starts_with("Send this: stand-in heard"), format!("{last_user:?}"));

    // ---- Global indicator ----
    let other = thread_with_turn(check, &state, "Voice elsewhere")?;
    check.go(&key);
    check.focus_composer();
    set_prompt(check, "");
    record_for(check, "mod-shift-d", 0.5)?;
    check.go(&other);
    check.pump(|c| element_visible(c, "voice-indicator"), Duration::from_secs(5), "indicator shown")?;
    check.wait(Duration::from_millis(800));
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v8-indicator.png")?;
    click(check, "composer-voice-finish")?;
    check.pump(|c| !element_visible(c, "voice-indicator"), Duration::from_secs(20), "indicator gone")?;
    check.go(&key);
    check.frame();
    let text = prompt(check);
    check.check("the indicator finishes into the draft that is off screen", heard_tone(&text), format!("{text:?}"));

    // ---- A refused microphone ----
    source(true);
    set_prompt(check, "");
    check.focus_composer();
    click(check, "composer-voice")?;
    check.pump(|c| phase(c) == VoiceInputPhase::Error, Duration::from_secs(10), "error")?;
    let shown = element_visible(check, "composer-voice-error");
    check.check("a refused microphone shows the blocked error", shown, "error capsule");
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("v9-error.png")?;
    source(false);
    click(check, "composer-voice-retry")?;
    check.pump(|c| phase(c) == VoiceInputPhase::Recording, Duration::from_secs(10), "recording again")?;
    check.press("escape");
    check.pump(|c| phase(c) == VoiceInputPhase::Idle, Duration::from_secs(5), "idle")?;

    // ---- Off hides the mic ----
    check.cx.update(|cx| t3code_web::speech::voice_consent::choose_audio_destination(None, cx));
    check.frame();
    let hidden = !element_visible(check, "composer-voice");
    check.check("choosing Off hides the mic", hidden, "hidden");
    test_source::set(None);
    Ok(())
}
