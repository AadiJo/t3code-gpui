//! `live-check composer-actions`: the composer's own shortcuts (`composer.*`) driven through real
//! key events on a seeded server: effort and mode open the model picker where the fork does,
//! effort steps, the workspace and branch shortcuts open the strip's menus on a draft, dictation
//! with no transcription destination leaves the key alone, a repeated action is ignored until the
//! key comes up, and send-and-new-thread sends then opens a new draft. Run `seed` first.
//!
//! `live-check stash`: the prompt stash through Mod+S and the drawer's keys, with a file's upload
//! kept across stash and restore, the stored `t3code:prompt-stash:v2` document, and shots of the
//! badge and drawer at the owner's look for the fork comparison.
//!
//! `live-check env-banner`: the machine banner while the server is down. The phase writes
//! `env-banner.stop` when the thread is shown (the runner then stops the server) and
//! `env-banner.start` after Disconnect (the runner restarts it), and checks the reconnect grace,
//! Retry now, the 20 s Disconnect server, Reconnect switching the machine back on, and the
//! banner leaving once connected.

use std::time::Duration;

use t3code_web::router::Route;
use t3code_web::shortcuts::commands as k;

use super::{Check, composer_of, load_state, thread_with_turn};

fn picker_state(check: &mut Check) -> Option<(Option<usize>, bool)> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).picker().map(|p| (p.read(cx).effort_focus(), p.read(cx).permissions_open())))
}

fn effort(check: &mut Check) -> Option<String> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).state().selection.effort.as_ref().map(|e| e.to_string()))
}

fn strip_menu(check: &mut Check) -> Option<&'static str> {
    check.read(|root, cx| root.thread_view().read(cx).branch_strip().and_then(|s| s.read(cx).open_menu_name()))
}

fn prompt(check: &mut Check) -> String {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned())
}

fn close_strip(check: &mut Check) {
    check.press("escape");
    let strip = check.read(|root, cx| root.thread_view().read(cx).branch_strip().cloned());
    if let Some(strip) = strip {
        check.cx.update(|cx| strip.update(cx, |s, cx| s.open_menu("", cx)));
    }
    check.frame();
}

pub(super) fn run_composer_actions(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    page_keys(check, &state)?;
    let key = thread_with_turn(check, &state, "Composer actions")?;
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    // The Claude instance's models have effort levels; Replay's have none.
    let picked = super::pick_instance(check, "claudeAgent");
    check.check("the composer can pick the Claude instance", picked, picked);
    let levels = {
        let composer = composer_of(check);
        check.cx.update(|cx| {
            let state = composer.read(cx).state();
            state.catalog.model(&state.selection.instance_id, &state.selection.model).and_then(|m| m.effort.as_ref()).map_or(0, |d| d.options.len())
        })
    };
    check.note(format!("selected model has {levels} effort levels"));
    check.focus_composer();

    // composer.effort: the picker opens with the keyboard on the selected level.
    check.press("mod-shift-e");
    let opened = picker_state(check);
    check.check("mod+shift+e opens the model picker on the effort column", matches!(opened, Some((Some(_), false))), format!("{opened:?}"));
    check.shot("a0-effort-open.png")?;
    let before = effort(check);
    let start = opened.and_then(|o| o.0);
    check.press("down");
    let moved = picker_state(check).and_then(|p| p.0);
    check.check("Down moves to the next level", moved.is_some() && moved != start, format!("{start:?} -> {moved:?}"));
    check.press("enter");
    let picked = effort(check);
    check.check("Enter picks that level", picked.is_some() && picked != before, format!("{before:?} -> {picked:?}"));
    check.press("escape");
    let closed = picker_state(check).is_none();
    check.check("Escape closes the picker", closed, "");

    // composer.increaseEffort / decreaseEffort have no default chord: dispatch on the focused composer.
    check.focus_composer();
    let before = effort(check);
    check.action(k::IncreaseEffort);
    let up = effort(check);
    check.check("increaseEffort steps the level", up.is_some() && up != before, format!("{before:?} -> {up:?}"));
    check.cx.update_window(check.window, |_, w, cx| w.dispatch_action(Box::new(k::IncreaseEffort), cx)).ok();
    check.frame();
    let repeat = effort(check);
    check.check("a repeat of the same press is ignored until a key comes up", repeat == up, format!("{repeat:?}"));
    check.press("escape");
    check.action(k::DecreaseEffort);
    let down = effort(check);
    check.check("decreaseEffort steps back", down == before, format!("{up:?} -> {down:?}"));

    // composer.mode with the default mode: no chip, so the picker answers with its permissions menu.
    check.press("mod-shift-a");
    let mode = picker_state(check);
    check.check("mod+shift+a opens the picker's permissions menu (no chip)", matches!(mode, Some((_, true))), format!("{mode:?}"));
    check.shot("a1-mode-open.png")?;
    check.press("escape");
    check.press("escape");

    // composer.dictation with no destination: the key falls through and nothing starts.
    check.focus_composer();
    check.type_text("dictation probe");
    check.press("mod-shift-d");
    let text = prompt(check);
    check.check("mod+shift+d with no transcription destination changes nothing", text == "dictation probe", text);

    // composer.host on a server thread with one machine: no picker to open.
    check.press("mod-shift-h");
    let host = strip_menu(check);
    check.check("mod+shift+h with one machine opens nothing", host.is_none(), format!("{host:?}"));

    // composer.sendAndNewThread: the message goes, then a new draft opens in the project.
    check.press("mod-alt-enter");
    check.pump(|c| matches!(c.route(), Route::Draft(_)), Duration::from_secs(10), "a new draft opened")?;
    let route = check.route();
    check.check("mod+alt+enter on a thread opens a new draft", matches!(route, Route::Draft(_)), format!("{route:?}"));
    let k2 = key.clone();
    check.pump(
        move |c| super::projection_of(c, &k2).is_some_and(|p| p.runs.len() >= 2),
        Duration::from_secs(30),
        "the message went to the thread",
    )?;
    check.check("and the message was sent to the thread", true, "");

    // On the draft the strip's workspace and branch menus open from the keyboard.
    check.pump(|c| strip_ready(c), Duration::from_secs(20), "draft strip refs loaded")?;
    check.focus_composer();
    check.press("mod-shift-x");
    let workspace = strip_menu(check);
    check.check("mod+shift+x opens the workspace menu", matches!(workspace, Some("workspace" | "run-context")), format!("{workspace:?}"));
    check.shot("a2-workspace-open.png")?;
    close_strip(check);
    check.focus_composer();
    check.press("mod-shift-g");
    let branch = strip_menu(check);
    check.check("mod+shift+g opens the branch picker", branch == Some("branch"), format!("{branch:?}"));
    check.shot("a3-branch-open.png")?;
    close_strip(check);
    Ok(())
}

fn strip_ready(check: &mut Check) -> bool {
    check.read(|root, cx| root.thread_view().read(cx).branch_strip().is_some_and(|s| s.read(cx).props().show_git_controls))
}

/// The timeline's scroll top and the page the composer scrolls (viewport less 36px and its inset).
fn timeline_metrics(check: &mut Check) -> (f32, f32) {
    check.read(|root, cx| {
        let timeline = root.thread_view().read(cx).chat().read(cx).timeline().clone();
        let timeline = timeline.read(cx);
        let (bounds, _, top, _) = timeline.list_state().viewport_metrics();
        (f32::from(top), f32::from(bounds.size.height - gpui_kit::px(36.) - timeline.composer_bottom()))
    })
}

/// Page Up / Page Down in an empty composer page the long thread's timeline by one page.
fn page_keys(check: &mut Check, state: &super::Seeded) -> anyhow::Result<()> {
    let long = super::key_of(state, &state.long_thread);
    check.go(&long);
    check.pump(|c| c.shown_input().entries.len() >= 6, Duration::from_secs(20), "long thread shown")?;
    check.wait(Duration::from_millis(400));
    check.focus_composer();
    let (top, page) = timeline_metrics(check);
    check.press("pageup");
    check.wait(Duration::from_millis(300));
    let (after_up, _) = timeline_metrics(check);
    check.check("Page Up in the composer scrolls the timeline up", after_up < top - page * 0.9, format!("top {top:.1} -> {after_up:.1}, page {page:.1}"));
    check.press("pagedown");
    check.wait(Duration::from_millis(300));
    let (after_down, _) = timeline_metrics(check);
    check.check("Page Down scrolls down exactly one page", (after_down - after_up - page).abs() < 1., format!("{after_up:.1} -> {after_down:.1}, page {page:.1}"));
    check.press("pageup");
    check.wait(Duration::from_millis(300));
    let (again, _) = timeline_metrics(check);
    check.check("and Page Up exactly one page from a settled position", (after_down - again - page).abs() < 1., format!("{after_down:.1} -> {again:.1}"));
    Ok(())
}

fn stash_entries(check: &mut Check) -> Vec<(String, usize)> {
    check.cx.update(|cx| {
        t3code_web::prompt_stash_store::PromptStash::entries(cx).iter().map(|e| (e.prompt.clone(), e.file_count())).collect()
    })
}

fn stash_menu(check: &mut Check) -> Option<usize> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).stash_menu())
}

fn set_prompt(check: &mut Check, text: &str) {
    let composer = composer_of(check);
    let text = text.to_owned();
    check.cx.update(|cx| {
        let editor = composer.read(cx).editor().clone();
        editor.update(cx, |e, cx| {
            let end = text.len();
            e.set_text(text, cx);
            e.set_cursor(end, cx);
        });
    });
    check.frame();
}

pub(super) fn run_stash(check: &mut Check) -> anyhow::Result<()> {
    use t3code_web::composer_draft_store::UploadState;
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = super::key_of(&state, &state.edit_thread);
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    // Start from an empty stash.
    for (id, _) in check.cx.update(|cx| t3code_web::prompt_stash_store::PromptStash::entries(cx).iter().map(|e| (e.id.clone(), ())).collect::<Vec<_>>()) {
        check.cx.update(|cx| t3code_web::prompt_stash_store::PromptStash::take(&id, cx));
    }
    set_prompt(check, "");
    check.focus_composer();
    check.type_text("Check why the CI cache misses on arm64");
    check.press("mod-s");
    let after = (prompt(check), stash_entries(check));
    check.check("Mod+S stashes the prompt and clears the composer", after.0.is_empty() && after.1.len() == 1, format!("{after:?}"));
    check.type_text("Write the release notes for the relay handshake change");
    check.press("mod-s");
    // A file whose upload finished goes into the stash with its upload.
    let file = check.out.join("stash-notes.txt");
    std::fs::write(&file, "notes for the stash check\n")?;
    let composer = composer_of(check);
    check.cx.update(|cx| composer.update(cx, |c, cx| c.attach_paths(vec![file.clone()], cx)));
    check.pump(
        |c| {
            let composer = composer_of(c);
            c.cx.update(|cx| composer.read(cx).attachments().iter().all(|a| a.upload == UploadState::Uploaded) && !composer.read(cx).attachments().is_empty())
        },
        Duration::from_secs(20),
        "file uploaded",
    )?;
    check.focus_composer();
    check.type_text("Refactor the clamp helper so saturate reuses it and add a test for negative input");
    check.press("mod-s");
    let entries = stash_entries(check);
    let attachments_left = check.cx.update(|cx| composer.read(cx).attachments().len());
    check.check(
        "a prompt with an uploaded file stashes with the file, newest first",
        entries.len() == 3 && entries[0].1 == 1 && attachments_left == 0,
        format!("{entries:?} attachments left {attachments_left}"),
    );
    check.wait(Duration::from_millis(100));
    check.shot("s0-stash-badge.png")?;
    let stored = check.cx.update(|cx| t3code_web::client_persistence_storage::LocalStorage::get::<serde_json::Value>(cx, "t3code:prompt-stash:v2"));
    let stored_ok = stored.as_ref().is_some_and(|v| v["version"] == 2 && v["state"]["entries"].as_array().is_some_and(|e| e.len() == 3));
    check.check("the stash is stored under t3code:prompt-stash:v2, version 2", stored_ok, format!("{}", stored.map(|v| v.to_string().chars().take(160).collect::<String>()).unwrap_or_default()));

    // An empty composer: Mod+S opens the drawer on the newest entry.
    check.press("mod-s");
    let open = stash_menu(check);
    check.check("Mod+S on an empty composer opens the stash", open == Some(0), format!("{open:?}"));
    check.shot("s1-stash-menu.png")?;
    check.press("down");
    check.press("down");
    let at = stash_menu(check);
    check.check("Down moves the highlight", at == Some(2), format!("{at:?}"));
    check.press("enter");
    let restored = (prompt(check), stash_entries(check).len(), stash_menu(check));
    check.check(
        "Enter restores the highlighted (oldest) prompt and closes the stash",
        restored.0 == "Check why the CI cache misses on arm64" && restored.1 == 2 && restored.2.is_none(),
        format!("{restored:?}"),
    );
    // Restoring into text appends after a blank line.
    check.press("mod-s");
    set_prompt(check, "Current draft");
    let composer = composer_of(check);
    let newest = stash_entries(check);
    check.cx.update(|cx| composer.update(cx, |c, _| c.stash_menu().is_none()));
    let id = check.cx.update(|cx| t3code_web::prompt_stash_store::PromptStash::entries(cx).iter().find(|e| e.file_count() > 0).map(|e| e.id.clone()).unwrap_or_default());
    check.cx.update_window(check.window, |_, w, cx| composer.update(cx, |c, cx| c.restore_stash_entry(&id, w, cx))).ok();
    check.frame();
    let merged = prompt(check);
    let restored_file = check.cx.update(|cx| composer.read(cx).attachments().iter().map(|a| (a.name.to_string(), a.upload)).collect::<Vec<_>>());
    check.check(
        "restoring into text appends after a blank line, with the file back and uploaded",
        merged.starts_with("Current draft\n\n[stash-notes.txt](t3-context://v1/file/") && merged.contains("Refactor the clamp helper") && restored_file == vec![("stash-notes.txt".to_owned(), UploadState::Uploaded)],
        format!("{merged:?} {restored_file:?} (stash was {newest:?})"),
    );
    set_prompt(check, "");
    check.cx.update(|cx| composer.update(cx, |c, cx| {
        let ids: Vec<_> = c.attachments().iter().map(|a| a.context_id.clone()).collect();
        for id in ids {
            c.remove_attachment(id, cx);
        }
    }));
    check.focus_composer();
    // Mod+Backspace deletes the highlighted entry; Escape closes.
    check.press("mod-s");
    let before = stash_entries(check).len();
    check.press("mod-backspace");
    let after = stash_entries(check).len();
    let still = stash_menu(check).is_some();
    check.check("Mod+Backspace deletes the highlighted entry", after + 1 == before && still, format!("{before} -> {after}"));
    check.press("escape");
    let closed = stash_menu(check).is_none();
    check.check("Escape closes the stash", closed, "");
    // With exactly one entry, Mod+S on an empty composer restores it straight away.
    let left = stash_entries(check);
    check.press("mod-s");
    let text = prompt(check);
    let now_empty = stash_entries(check).is_empty();
    check.check(
        "with one entry, Mod+S on an empty composer restores it",
        left.len() == 1 && text == left[0].0 && now_empty,
        format!("{left:?} -> {text:?}"),
    );
    // Typing closes an open drawer (`stashMenuClosesOn`).
    check.press("mod-s");
    set_prompt(check, "");
    check.press("mod-s");
    check.type_text("x");
    let closed = stash_menu(check).is_none();
    check.check("typing closes the stash", closed, "");
    set_prompt(check, "");
    Ok(())
}

fn env_banner(check: &mut Check) -> Option<t3code_web::components::chat::composer::ComposerBanner> {
    let composer = composer_of(check);
    check.cx.update(|cx| composer.read(cx).state().banners.iter().find(|b| b.id.starts_with("environment-unavailable:")).cloned())
}

fn emit_banner_action(check: &mut Check, id: String) {
    use t3code_web::components::chat::composer::ComposerEvent;
    let composer = composer_of(check);
    check.cx.update(|cx| composer.update(cx, |_, cx| cx.emit(ComposerEvent::BannerAction(id.into()))));
    check.frame();
}

pub(super) fn run_env_banner(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = super::key_of(&state, &state.edit_thread);
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "thread shown")?;
    let none = env_banner(check).is_none();
    check.check("no machine banner while connected", none, "");
    std::fs::write(check.out.join("env-banner.stop"), "")?;
    check.pump(|c| !c.connected(), Duration::from_secs(60), "the server went away")?;
    let lost = std::time::Instant::now();
    check.wait(Duration::from_millis(1000));
    let early = env_banner(check);
    check.check("a reconnect shows no banner during its 2 s grace", early.is_none(), format!("{early:?}"));
    check.pump(|c| env_banner(c).is_some(), Duration::from_secs(10), "machine banner")?;
    let banner = env_banner(check).expect("banner");
    let shown_after = lost.elapsed();
    check.check(
        "after the grace the banner says the machine is reconnecting, with Retry now",
        banner.title.ends_with("is reconnecting") && banner.action.as_deref() == Some("Retry now") && banner.secondary_action.is_none(),
        format!("{:?} {:?} {:?} after {shown_after:?}", banner.title, banner.action, banner.description),
    );
    check.shot("e0-env-reconnecting.png")?;
    emit_banner_action(check, banner.id.to_string());
    let kept = env_banner(check).is_some();
    check.check("Retry now keeps the banner while the server is down", kept, "");
    check.pump(|c| env_banner(c).is_some_and(|b| b.secondary_action.is_some()), Duration::from_secs(30), "Disconnect server offered")?;
    check.check("Disconnect server appears once the machine stayed unavailable 20 s", lost.elapsed() >= Duration::from_secs(19), format!("{:?}", lost.elapsed()));
    check.shot("e1-env-disconnect.png")?;
    let id = banner.id.to_string();
    emit_banner_action(check, format!("{id}#secondary"));
    let environment = key.environment_id.0.to_string();
    let enabled = |c: &mut Check| {
        let environment = environment.clone();
        c.cx.update(|cx| {
            t3code_web::connection::ConnectionRuntime::state(cx).read(cx).environments.iter().find(|e| e.id().as_str() == environment).map(|e| e.entry.enabled)
        })
    };
    check.pump(|c| enabled(c) == Some(false), Duration::from_secs(10), "machine switched off")?;
    check.check("Disconnect server switches the machine off", true, "");
    std::fs::write(check.out.join("env-banner.start"), "")?;
    check.wait(Duration::from_secs(8));
    // The fork leaves the thread after Disconnect; switch the machine back on as Settings does.
    let environment_id = t3code_client_runtime::environment::EnvironmentId::new(environment.as_str());
    let task = check.cx.update(|cx| t3code_web::connection::ConnectionRuntime::global(cx).set_enabled(environment_id, true, cx));
    check.cx.background_executor.spawn(async move { _ = task.await; }).detach();
    let _ = id;
    check.pump(|c| enabled(c) == Some(true), Duration::from_secs(10), "machine switched on")?;
    check.pump(|c| c.connected(), Duration::from_secs(60), "reconnected")?;
    check.pump(|c| env_banner(c).is_none(), Duration::from_secs(10), "banner gone")?;
    check.check("switched back on, the banner leaves once connected", true, "");
    Ok(())
}
