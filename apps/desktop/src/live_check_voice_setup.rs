//! `live-check voice-setup`: the model setup dialog against a server whose speech stand-in
//! starts with no model installed (`T3_VOICE_STANDIN_MISSING=1`; it reports download progress
//! for about three seconds). The composer's mic opens "Set up voice input" with the model
//! `chooseSetupSpeechModel` picks for English, Download shows progress and moves to "Voice input
//! is ready", whose microphone select lists the system's microphones, and Start dictating
//! records into the draft. Run `seed` first, on a fresh stand-in.

use std::time::Duration;

use t3code_client_runtime::voice_input::VoiceInputPhase;
use t3code_web::hooks::use_settings::ClientSettingsStore;
use t3code_client_runtime::voice_input::speech::SpeechRecommendationRole;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::components::settings::speech_model_browser::{ShownSpeechModelBrowser, SpeechModelBrowser};
use t3code_web::router::Route;
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

    settings_models(check, &key)?;
    t3code_web::speech::pcm_capture::test_source::set(None);
    Ok(())
}

fn browser(check: &mut Check) -> Option<gpui_kit::Entity<SpeechModelBrowser>> {
    check.cx.update(|cx| cx.try_global::<ShownSpeechModelBrowser>().and_then(|b| b.0.upgrade()))
}

fn model_state(check: &mut Check, id: &str) -> Option<(String, bool)> {
    let b = browser(check)?;
    check.cx.update(|cx| b.read(cx).models().iter().find(|m| m.id.as_str() == id).map(|m| (m.state.as_str().to_owned(), m.active)))
}

/// Settings → Voice: the model browser, the model test, the advanced rows and the cleanup model.
fn settings_models(check: &mut Check, thread: &ThreadKey) -> anyhow::Result<()> {
    let root = check.root.clone();
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |router, cx| router.navigate(Route::settings(SettingsPath::Voice), cx));
    });
    check.pump(|c| browser(c).is_some_and(|b| c.cx.update(|cx| b.read(cx).models().len() == 2)), Duration::from_secs(15), "model catalog")?;
    let b = browser(check).expect("browser");
    check.wait(Duration::from_millis(500));
    super::live_check_settings::click_id(check, "speech-models-list")?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s4-models.png")?;
    let names = check.cx.update(|cx| b.read(cx).shown_model_names(cx));
    check.check("Models lists the active model first", names == ["Stand-in small", "Stand-in large"], format!("{names:?}"));
    // The headless host reports no preferred language; pick English as the language menu would.
    let b2 = b.clone();
    check.cx.update(|cx| b2.update(cx, |b, cx| b.set_languages(vec!["en".to_owned()], cx)));
    check.frame();
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s4b-models-english.png")?;
    let roles = check.cx.update(|cx| b.read(cx).recommendations());
    check.check(
        "the reference benchmarks badge the larger model Recommended and the smaller Faster",
        roles.iter().any(|(id, r)| id == "standin-large" && *r == SpeechRecommendationRole::Recommended)
            && roles.iter().any(|(id, r)| id == "standin-small" && *r == SpeechRecommendationRole::Faster),
        format!("{roles:?}"),
    );
    let window = check.window;
    let b2 = b.clone();
    check.cx.update_window(window, |_, w, cx| b2.update(cx, |b, cx| b.set_search("large", w, cx))).ok();
    check.frame();
    let names = check.cx.update(|cx| b.read(cx).shown_model_names(cx));
    check.check("search filters by name and description", names == ["Stand-in large"], format!("{names:?}"));
    let b2 = b.clone();
    check.cx.update_window(window, |_, w, cx| b2.update(cx, |b, cx| b.set_search("", w, cx))).ok();

    // The model test records without cleanup into its own transcript.
    super::live_check_settings::click_id(check, "speech-test-model")?;
    check.pump(browser_recording, Duration::from_secs(15), "test recording")?;
    check.wait(Duration::from_millis(1500));
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s5-test-recording.png")?;
    super::live_check_settings::click_id(check, "composer-voice-finish")?;
    check.pump(|c| browser(c).is_some_and(|b| c.cx.update(|cx| b.read(cx).test_transcript().is_some())), Duration::from_secs(20), "test transcript")?;
    let transcript = check.cx.update(|cx| b.read(cx).test_transcript().unwrap_or_default().to_owned());
    check.check("Test model shows exactly what the model heard", heard_tone(&transcript), format!("{transcript:?}"));
    check.wait(Duration::from_millis(300));
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s6-test-transcript.png")?;

    // Advanced: hardware and unload go to the environment.
    super::live_check_settings::click_id(check, "voice-advanced-fold")?;
    check.pump(|c| element_visible(c, "speech-hardware"), Duration::from_secs(5), "advanced open")?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s7-advanced.png")?;
    let b2 = b.clone();
    check.cx.update(|cx| {
        b2.update(cx, |b, cx| b.set_acceleration(t3code_contracts::SpeechAcceleration::Known(t3code_contracts::SpeechAccelerationKnown::Cpu), cx))
    });
    check.pump(|c| server_log(c).contains("\"kind\":\"acceleration\",\"value\":\"cpu\""), Duration::from_secs(10), "hardware saved")?;
    check.pump(|c| browser(c).is_some_and(|b| c.cx.update(|cx| !b.read(cx).busy())), Duration::from_secs(10), "idle after hardware")?;
    let b2 = b.clone();
    check.cx.update(|cx| b2.update(cx, |b, cx| b.set_unload(t3code_contracts::SpeechModelUnloadTimeout::Min15, cx)));
    check.pump(|c| server_log(c).contains("\"kind\":\"unload\",\"value\":\"min_15\""), Duration::from_secs(10), "unload saved")?;
    check.check("Hardware and Unload model write to the environment", true, "acceleration cpu, unload min_15 logged");

    // Download, use and delete from the list.
    check.pump(|c| browser(c).is_some_and(|b| c.cx.update(|cx| !b.read(cx).busy())), Duration::from_secs(10), "idle")?;
    super::live_check_settings::click_id(check, "speech-download-standin-large")?;
    check.pump(|c| model_state(c, "standin-large").is_some_and(|(s, _)| s == "downloading"), Duration::from_secs(10), "list download progress")?;
    check.wait(Duration::from_millis(1100));
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s8-list-downloading.png")?;
    check.pump(|c| model_state(c, "standin-large").is_some_and(|(s, _)| s == "installed"), Duration::from_secs(20), "list download done")?;
    check.check("Download in the list shows progress and installs the model", true, "downloading then installed");
    super::live_check_settings::click_id(check, "speech-use-standin-large")?;
    check.pump(|c| model_state(c, "standin-large").is_some_and(|(_, a)| a), Duration::from_secs(10), "large selected")?;
    check.check("Use model selects it", true, "standin-large active");
    super::live_check_settings::click_id(check, "speech-delete-standin-small")?;
    check.pump(|c| model_state(c, "standin-small").is_some_and(|(s, _)| s == "downloadable"), Duration::from_secs(10), "small deleted")?;
    check.check("Delete removes an installed model", true, "standin-small downloadable");

    // Deleting the active model leaves the environment without one; the setup can be cancelled.
    super::live_check_settings::click_id(check, "speech-delete-standin-large")?;
    check.pump(|c| model_state(c, "standin-large").is_some_and(|(s, _)| s == "downloadable"), Duration::from_secs(10), "large deleted")?;
    check.go(thread);
    check.pump(|c| element_visible(c, "composer-voice"), Duration::from_secs(10), "mic")?;
    check.focus_composer();
    click(check, "composer-voice")?;
    check.pump(|c| setup(c).open && setup(c).model.is_some(), Duration::from_secs(15), "setup again")?;
    let offered = setup(check).model.map(|m| m.name).unwrap_or_default();
    check.check("the setup offers the environment's selected model", offered == "Stand-in large", offered);
    click(check, "voice-setup-download")?;
    check.pump(|c| setup(c).downloading, Duration::from_secs(5), "downloading")?;
    click(check, "voice-setup-cancel-download")?;
    check.pump(|c| server_log(c).contains("\"kind\":\"cancel\",\"id\":\"standin-large\""), Duration::from_secs(10), "cancel logged")?;
    let closed = !setup(check).open;
    check.check("Cancel download closes the dialog and stops the download on the environment", closed, "cancel logged");

    // Cleanup model: a separate model for transcript cleanup is an environment setting.
    check.cx.update(|cx| t3code_web::speech::voice_consent::choose_transcript_cleanup(true, cx));
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |router, cx| router.navigate(Route::settings(SettingsPath::Voice), cx));
    });
    check.wait(Duration::from_millis(500));
    super::live_check_settings::click_id(check, "switch-speech-cleanup-model")?;
    let environment_id = t3code_contracts::EnvironmentId::new(load_state(&check.out)?.environment_id.as_str());
    let env2 = environment_id.clone();
    check.pump(
        move |c| super::live_check_settings::server_settings(c, &env2).is_some_and(|s| s.speech_post_processing_model_selection.is_some()),
        Duration::from_secs(10),
        "cleanup model saved",
    )?;
    let saved = super::live_check_settings::server_settings(check, &environment_id).and_then(|s| s.speech_post_processing_model_selection);
    check.wait(Duration::from_millis(400));
    super::live_check_settings::click_id(check, "switch-speech-cleanup-model")?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s9-cleanup-model.png")?;
    check.check("Cleanup model turns on a separate model, starting from the text generation model", saved.is_some(), format!("{saved:?}"));
    let env2 = environment_id.clone();
    check.pump(
        move |c| super::live_check_settings::server_settings(c, &env2).is_some_and(|s| s.speech_post_processing_model_selection.is_none()),
        Duration::from_secs(10),
        "cleanup model cleared",
    )?;
    check.check("and turning it off goes back to the text generation model", true, "cleared");
    check.cx.update(|cx| t3code_web::speech::voice_consent::choose_transcript_cleanup(false, cx));

    // Language names follow the UI language, from the CLDR table.
    check.cx.update(|cx| t3code_web::i18n::runtime::apply_language_setting("de", cx));
    check.frame();
    let names = check.cx.update(|_| {
        ["en", "de", "ja"].map(|code| t3code_web::speech::language_names::language_label(code).to_string())
    });
    super::live_check_settings::click_id(check, "speech-models-list")?;
    super::hover_at(check, point(px(5.), px(700.)))?;
    check.shot("s10-models-german.png")?;
    check.cx.update(|cx| t3code_web::i18n::runtime::apply_language_setting("en", cx));
    check.check("language names follow the UI language (German)", names == ["Englisch", "Deutsch", "Japanisch"], format!("{names:?}"));
    Ok(())
}

fn browser_recording(check: &mut Check) -> bool {
    let Some(b) = browser(check) else { return false };
    check.cx.update(|cx| {
        let owner = format!("voice-test:{}", b.read(cx).environment_id().as_str());
        VoiceInput::try_global(cx).is_some_and(|v| v.read(cx).state_for(&owner).phase == VoiceInputPhase::Recording)
    })
}
