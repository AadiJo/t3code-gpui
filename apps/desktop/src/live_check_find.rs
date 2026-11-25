//! `live-check find-history-seed` then `live-check find-history`: find in thread reaches a match
//! above the loaded window. The seed makes a 45-turn thread on a real server whose only
//! "quokkaflux" is in its first message and writes its id to `<out>/find-thread.txt`. The check,
//! on a fresh profile (`T3_FIND_THREAD`), opens the thread cold (the server's bounded window,
//! without the first turn), finds "quokkaflux" with no loaded match, and must page history
//! through the server search's entry in one request until the match is loaded and active.

use std::time::Duration;

use t3code_client_runtime::operations::{CreateThread, SendMessage};
use t3code_contracts::{ModelSelection, ProviderInteractionMode, RuntimeMode, ThreadId};
use t3code_web::components::chat::timeline_entries::{EntryKind, MessageRole};
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;

use super::live_check_families::{all_connected, pair_if_needed, project_at};
use super::*;

const WORD: &str = "quokkaflux";

fn user_turns(check: &mut Check) -> usize {
    check.shown_input().entries.iter().filter(|e| matches!(&e.kind, EntryKind::Message(m) if m.role == MessageRole::User)).count()
}

pub(super) fn run_find_history_seed(check: &mut Check) -> anyhow::Result<()> {
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.clone());
    let environment_id = t3code_contracts::EnvironmentId::new(environment_id.as_str());
    let project = project_at(check, &environment_id, &std::env::var("T3_PROJECT_ROOT")?)?;
    let commands = commands_of(check, &environment_id)?;
    let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let input = CreateThread {
        thread_id: thread_id.clone(),
        project_id: project,
        title: "Find through history".into(),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
    };
    let c = commands.clone();
    check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let key = thread_key(&environment_id, &thread_id);
    for turn in 1..=45 {
        let text = if turn == 1 { format!("Turn 1: remember the {WORD} note.") } else { format!("Turn {turn}: keep going.") };
        let before = check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&key).and_then(|r| r.latest_run.as_ref().map(|r| r.run_id.to_string())));
        let c = commands.clone();
        let message = SendMessage::text(thread_id.clone(), &text);
        check.block(async move { c.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        let k = key.clone();
        check.pump(
            move |c| {
                c.cx.update(|cx| {
                    StateBridge::shell(cx).read(cx).row(&k).and_then(|r| r.latest_run.as_ref()).is_some_and(|r| Some(r.run_id.to_string()) != before && r.completed_at.is_some())
                })
            },
            Duration::from_secs(60),
            "turn settled",
        )?;
    }
    std::fs::write(check.out.join("find-thread.txt"), thread_id.as_str())?;
    check.note(format!("seeded 45 turns on {}", thread_id.as_str()));
    Ok(())
}

pub(super) fn run_find_history(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.clone());
    let environment_id = t3code_contracts::EnvironmentId::new(environment_id.as_str());
    let thread_id = ThreadId::new(std::env::var("T3_FIND_THREAD").context("T3_FIND_THREAD")?);
    let key = thread_key(&environment_id, &thread_id);
    let k = key.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&k).is_some()), Duration::from_secs(30), "the thread in the sidebar")?;
    check.go(&key);
    check.pump(|c| user_turns(c) > 0 && c.shown_input().history.has_more, Duration::from_secs(30), "the bounded window")?;
    let before = user_turns(check);
    check.check("a cold open shows the bounded window with older turns left on the server", before < 45, format!("{before} user turns"));
    let timeline = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().clone());
    check.cx.update(|cx| timeline.update(cx, |t, cx| t.prepare_find(cx)));
    check.pump(|c| { let t = timeline.clone(); !c.cx.update(|cx| t.read(cx).find_counting()) }, Duration::from_secs(10), "the find index")?;
    let count = check.cx.update(|cx| timeline.update(cx, |t, cx| t.set_find_query(WORD, cx)));
    check.check("no loaded row matches, so find asks for older history", count.1 == 0, format!("{count:?}"));
    let t = timeline.clone();
    let found = check.pump(move |c| c.cx.update(|cx| t.read(cx).find_count().1) > 0, Duration::from_secs(30), "the match loaded");
    let after = user_turns(check);
    let (active, total) = check.cx.update(|cx| timeline.read(cx).find_count());
    check.check("find pages history through the server's entry until the match loads", found.is_ok() && total == 1, format!("{active}/{total}, user turns {before} -> {after}"));
    check.check("the page reached the first turn in one request", after == 45, format!("{after} user turns"));
    let first_shown = check.cx.update(|cx| timeline.read(cx).active_match()).is_some();
    check.check("the match is active and revealed", first_shown, "");
    check.wait(Duration::from_millis(500));
    check.shot("find-01-match-loaded.png")?;
    Ok(())
}
