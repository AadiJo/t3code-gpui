//! `live-check voice-setup`: the model setup dialog against a server whose speech stand-in
//! starts with no model installed (`T3_VOICE_STANDIN_MISSING=1`; it reports download progress
//! for about three seconds). The composer's mic opens "Set up voice input" with the model
//! `chooseSetupSpeechModel` picks for English, Download shows progress and moves to "Voice input
//! is ready", whose microphone select lists the system's microphones, and Start dictating
//! records into the draft. Run `seed` first, on a fresh stand-in.

use std::time::Duration;

use t3code_client_runtime::voice_input::VoiceInputPhase;
use t3code_web::hooks::use_settings::ClientSettingsStore;
use t3code_web::speech::voice_input_session::{VoiceInput, VoiceSetup};

use super::live_check_voice::{click, element_visible, heard_tone, phase, prompt, server_log, set_prompt, source};
use super::*;

fn setup(check: &mut Check) -> VoiceSetup {
    check.cx.update(|cx| VoiceInput::try_global(cx).map(|v| v.read(cx).setup().clone()).unwrap_or_default())
}

pub(super) fn run_voice_setup(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    source(false);
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    check.cx.update(|cx| {
        ClientSettingsStore::update(cx, |s| {
            s.speech_streaming = true;
            s.speech_language = t3code_contracts::SpeechLanguage::new("en".to_owned());
            s.voice_microphones = Vec::new();
        });
        let destination = t3code_client_runtime::voice_input::speech::VoiceTranscriptionDestination::Environment { environment_id: environment_id.clone() };
        t3code_web::speech::voice_consent::choose_audio_destination(Some(destination), cx);
    });
    let key = thread_with_turn(check, &state, "Voice setup")?;
    check.go(&key);
    check.pump(|c| element_visible(c, "composer-voice"), Duration::from_secs(15), "mic shown while the environment has no model")?;
    check.focus_composer();
    set_prompt(check, "Setup:");
    click(check, "composer-voice")?;
    check.pump(|c| setup(c).open && setup(c).model.is_some(), Duration::from_secs(15), "setup dialog offers a model")?;
    let offered = setup(check);
    let model = offered.model.clone().map(|m| m.name).unwrap_or_default();
    let shown = element_visible(check, "voice-input-setup");
    check.check(
        "a start with no model opens \"Set up voice input\" instead of the unavailable toast",
        offered.step == 0 && shown,
        format!("step {}", offered.step),
    );
    check.check(
        "it offers the environment's selected model for English",
        model == "Stand-in small" && offered.language.as_deref() == Some("en") && offered.error.is_none(),
        format!("{model:?} for {:?}", offered.language),
    );
    let now = phase(check);
    check.check("nothing records before the model is ready", now == VoiceInputPhase::Idle, format!("{now:?}"));
    check.wait(Duration::from_millis(400));
    check.shot("s0-setup.png")?;

    click(check, "voice-setup-download")?;
    check.pump(|c| setup(c).model.as_ref().and_then(|m| m.downloaded).is_some_and(|d| d > 0.), Duration::from_secs(10), "download progress")?;
    let progress = setup(check).model.and_then(|m| m.downloaded.map(|d| (d / m.size * 100.).round())).unwrap_or(0.);
    let (downloading, bar) = (setup(check).downloading, element_visible(check, "voice-setup-progress"));
    check.check(
        "Download polls the environment and shows its progress",
        downloading && bar && progress > 0. && progress < 100.,
        format!("{progress}%"),
    );
    check.shot("s1-downloading.png")?;
    check.pump(|c| setup(c).step == 1, Duration::from_secs(20), "model ready")?;
    let ready = setup(check);
    check.check(
        "once downloaded the dialog says \"Voice input is ready\"",
        ready.open && !ready.downloading && ready.error.is_none(),
        format!("step {} error {:?}", ready.step, ready.error),
    );
    check.pump(|c| element_visible(c, "voice-setup-microphone"), Duration::from_secs(5), "microphone select")?;
    check.wait(Duration::from_millis(400));
    check.shot("s2-ready.png")?;
    let log = server_log(check);
    check.check("the stand-in downloaded the offered model", log.contains("\"kind\":\"download-done\",\"id\":\"standin-small\""), "download-done logged");

    click(check, "voice-setup-microphone")?;
    check.wait(Duration::from_millis(300));
    check.shot("s3-microphones.png")?;
    let listed = check.cx.update(|cx| {
        t3code_web::components::chat::voice_input_setup::VoiceInputSetupDialog::try_global(cx).map(|d| d.read(cx).microphone_names()).unwrap_or_default()
    });
    check.check("the microphone select lists the system's microphones", listed == ["Desk microphone", "USB microphone"], format!("{listed:?}"));
    click(check, "voice-setup-microphone")?;
    check.frame();
    click(check, "voice-setup-start")?;
    check.pump(|c| phase(c) == VoiceInputPhase::Recording, Duration::from_secs(20), "recording after setup")?;
    let closed = !setup(check).open;
    check.check("Start dictating closes the dialog and records into the draft", closed, "closed");
    check.wait(Duration::from_millis(1500));
    click(check, "composer-voice-finish")?;
    check.pump(|c| phase(c) == VoiceInputPhase::Idle, Duration::from_secs(20), "transcript")?;
    let text = prompt(check);
    check.check("the transcript lands after the typed text", text.starts_with("Setup: stand-in heard") && heard_tone(&text), format!("{text:?}"));
    t3code_web::speech::pcm_capture::test_source::set(None);
    Ok(())
}
