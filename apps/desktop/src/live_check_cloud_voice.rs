//! `live-check cloud-voice`: the cloud transcription destinations end to end, against a server
//! whose outbound calls go to a local stand-in for OpenAI, xAI and OpenRouter (it accepts only the
//! fake key `sk-standin-good`; `T3_CLOUD_STANDIN_LOG` names its request log). Through real clicks
//! and keys: "Transcribe with" lists "Cloud via <environment>" with the three providers; OpenAI
//! asks for a key, "Save and enable" saves it on the environment and makes it this device's
//! destination; a dictation lands the stand-in's transcript in the composer; OpenRouter lists the
//! stand-in's models and dictates with the chosen one; the key button reopens the dialog as
//! "Manage OpenRouter dictation" and "Remove key…" removes the key; xAI with a wrong key shows
//! the rejected-key error. A stand-in microphone feeds a 440 Hz tone. Run `seed` first.

use std::sync::Arc;
use std::time::Duration;

use t3code_client_runtime::voice_input::VoiceInputPhase;
use t3code_client_runtime::voice_input::speech::VoiceTranscriptionDestination;
use t3code_contracts::CloudSpeechProvider;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::router::Route;
use t3code_web::speech::pcm_capture::test_source::{self, TestSource};
use t3code_web::speech::voice_input_session::VoiceInput;

use super::*;

fn source() {
    let rate = 48_000u32;
    let samples: Arc<[f32]> = (0..rate as usize * 60).map(|i| 0.3 * (2. * std::f32::consts::PI * 440. * i as f32 / rate as f32).sin()).collect();
    test_source::set(Some(TestSource { rate, samples, devices: vec!["Desk microphone".into()], deny: false }));
}

fn visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}

fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("no visible element {id}"))?;
    click_at(check, at)
}

fn phase(check: &mut Check) -> VoiceInputPhase {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).voice_state(cx).phase)
}

fn prompt(check: &mut Check) -> String {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned())
}

fn set_prompt(check: &mut Check, text: &str) {
    let composer = composer_of(check);
    let text = text.to_owned();
    check.cx.update(|cx| composer.read(cx).editor().clone().update(cx, |e, cx| e.set_text(text, cx)));
    check.frame();
}

fn destination(check: &mut Check) -> Option<VoiceTranscriptionDestination> {
    check.cx.update(|cx| t3code_web::speech::voice_consent::audio_grant(cx).map(|g| g.destination))
}

fn requests(check: &mut Check) -> String {
    let _ = check;
    std::env::var("T3_CLOUD_STANDIN_LOG").ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

fn open_settings(check: &mut Check) {
    let root = check.root.clone();
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |router, cx| router.navigate(Route::settings(SettingsPath::Voice), cx));
    });
    check.wait(Duration::from_millis(600));
}

/// Opens "Transcribe with" and picks a provider through the environment.
fn pick_cloud(check: &mut Check, provider: CloudSpeechProvider, environment: &str) -> anyhow::Result<()> {
    super::live_check_settings::click_id(check, "voice-destination")?;
    let id = format!("voice-destination-cloud-{}-{environment}", provider.as_str());
    let wanted = id.clone();
    check.pump(move |c| visible(c, &wanted), Duration::from_secs(5), "cloud choice listed")?;
    click(check, &id)?;
    check.pump(|c| visible(c, "cloud-voice-configuration"), Duration::from_secs(5), "cloud dialog open")?;
    Ok(())
}

fn type_key(check: &mut Check, key: &str) -> anyhow::Result<()> {
    check.pump(|c| visible(c, "voice-cloud-key"), Duration::from_secs(10), "key field shown")?;
    click(check, "voice-cloud-key")?;
    check.type_text(key);
    Ok(())
}

/// Records 1.5 s with the composer's mic button and waits for the transcript.
fn dictate(check: &mut Check, what: &str) -> anyhow::Result<String> {
    set_prompt(check, "");
    check.focus_composer();
    check.pump(|c| visible(c, "composer-voice"), Duration::from_secs(10), "mic button shown")?;
    click(check, "composer-voice")?;
    check.pump(|c| phase(c) == VoiceInputPhase::Recording, Duration::from_secs(20), "recording")?;
    check.wait(Duration::from_millis(1500));
    click(check, "composer-voice-finish")?;
    check.pump(|c| matches!(phase(c), VoiceInputPhase::Idle | VoiceInputPhase::Error), Duration::from_secs(30), what)?;
    Ok(prompt(check))
}

pub(super) fn run_cloud_voice(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    source();
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let environment = state.environment_id.clone();
    let key = thread_with_turn(check, &state, "Cloud dictation")?;

    // ---- OpenAI: key, enable, dictate ----
    open_settings(check);
    super::live_check_settings::click_id(check, "voice-destination")?;
    let ids: Vec<String> = [CloudSpeechProvider::Openai, CloudSpeechProvider::Xai, CloudSpeechProvider::Openrouter]
        .iter()
        .map(|p| format!("voice-destination-cloud-{}-{environment}", p.as_str()))
        .collect();
    let first = ids[0].clone();
    check.pump(move |c| visible(c, &first), Duration::from_secs(5), "destination menu open")?;
    let listed = ids.iter().all(|id| visible(check, id));
    check.check("Transcribe with lists OpenAI, Grok Voice Transcribe and OpenRouter under Cloud via the environment", listed, format!("{ids:?}"));
    check.shot("cv0-menu.png")?;
    click(check, &ids[0])?;
    check.pump(|c| visible(c, "cloud-voice-configuration"), Duration::from_secs(5), "cloud dialog open")?;
    type_key(check, "sk-standin-good")?;
    check.wait(Duration::from_millis(400));
    check.shot("cv1-openai-dialog.png")?;
    click(check, "voice-cloud-submit")?;
    check.pump(|c| !visible(c, "cloud-voice-configuration"), Duration::from_secs(15), "the key saves and the dialog closes")?;
    let chosen = destination(check);
    check.check(
        "Save and enable makes OpenAI via the environment this device's destination",
        matches!(&chosen, Some(VoiceTranscriptionDestination::Cloud { provider: CloudSpeechProvider::Openai, .. })),
        format!("{chosen:?}"),
    );
    check.pump(|c| c.cx.update(|cx| VoiceInput::destination(cx).is_some_and(|d| d.ready)), Duration::from_secs(10), "cloud destination ready")?;
    check.shot("cv2-openai-chosen.png")?;
    check.go(&key);
    check.wait(Duration::from_millis(500));
    let text = dictate(check, "OpenAI transcript")?;
    check.check("an OpenAI dictation lands the stand-in's transcript", text.contains("openai stand-in heard") && text.contains("gpt-transcribe"), &text);
    let log = requests(check);
    check.check("the server called the OpenAI stand-in with the saved key", log.contains("\"url\":\"/api.openai.com/v1/audio/transcriptions\",\"keyOk\":true"), log.lines().last().unwrap_or_default());

    // ---- OpenRouter: model, key, dictate ----
    open_settings(check);
    pick_cloud(check, CloudSpeechProvider::Openrouter, &environment)?;
    check.pump(|c| visible(c, "voice-cloud-model-0"), Duration::from_secs(10), "OpenRouter models listed")?;
    check.wait(Duration::from_millis(400));
    check.shot("cv3-openrouter-dialog.png")?;
    click(check, "voice-cloud-model-0")?;
    type_key(check, "sk-standin-good")?;
    click(check, "voice-cloud-submit")?;
    check.pump(|c| !visible(c, "cloud-voice-configuration"), Duration::from_secs(15), "OpenRouter enabled")?;
    let chosen = destination(check);
    check.check(
        "OpenRouter becomes the destination with the chosen model",
        matches!(&chosen, Some(VoiceTranscriptionDestination::Cloud { provider: CloudSpeechProvider::Openrouter, model: Some(m), .. }) if m == "standin/whisper-small"),
        format!("{chosen:?}"),
    );
    check.go(&key);
    check.wait(Duration::from_millis(500));
    let text = dictate(check, "OpenRouter transcript")?;
    check.check("an OpenRouter dictation uses the chosen model", text.contains("openrouter stand-in heard") && text.contains("standin/whisper-small"), &text);

    // ---- Manage and remove the OpenRouter key ----
    open_settings(check);
    click(check, "voice-cloud-manage")?;
    check.pump(|c| visible(c, "voice-cloud-remove"), Duration::from_secs(10), "manage dialog with Remove key")?;
    check.shot("cv4-manage.png")?;
    click(check, "voice-cloud-remove")?;
    click(check, "voice-cloud-remove-confirm")?;
    check.pump(|c| !visible(c, "cloud-voice-configuration"), Duration::from_secs(15), "key removed")?;
    let removed = {
        let link = check.cx.update(|cx| t3code_web::connection::ConnectionRuntime::global(cx).environment_link(&t3code_contracts::EnvironmentId::new(environment.as_str())));
        let link = link.context("environment link")?;
        check.block(async move { t3code_client_runtime::voice_input::environment::cloud_speech_keys(&link).await })?.ok()
    };
    check.check("Remove key… removes the OpenRouter key from the environment", removed.as_ref().is_some_and(|k| k.openrouter == Some(false) && k.openai), format!("{removed:?}"));

    // ---- xAI with a wrong key: the provider refuses it ----
    pick_cloud(check, CloudSpeechProvider::Xai, &environment)?;
    type_key(check, "sk-standin-wrong")?;
    click(check, "voice-cloud-submit")?;
    check.pump(|c| !visible(c, "cloud-voice-configuration"), Duration::from_secs(15), "xAI enabled")?;
    check.go(&key);
    check.wait(Duration::from_millis(500));
    dictate(check, "xAI refusal")?;
    let refused = phase(check) == VoiceInputPhase::Error;
    let message = {
        let composer = composer_of(check);
        check.cx.update(|cx| composer.read(cx).voice_state(cx).error.map(|e| format!("{e:?}")))
    };
    check.check("xAI refusing the key shows the voice error", refused, format!("{message:?}"));
    check.shot("cv5-xai-refused.png")?;
    check.cx.update(|cx| t3code_web::speech::voice_consent::choose_audio_destination(None, cx));
    let log = requests(check);
    check.check("every cloud request went to the stand-in", log.lines().count() >= 4, log.lines().count());
    Ok(())
}
