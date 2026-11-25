//! `live-check earlier-turns`: "Load earlier turns" on the seeded long thread (`live-state.json`
//! from `seed`, extended by `memory-seed`), on a profile that has never opened it. The first
//! window must be the server's bounded snapshot with the control above it; each click shows
//! "Loading earlier turns…", merges one page and leaves the row on screen where it was, also
//! while a turn streams; the last page removes the control. Heap is noted after the whole
//! history is loaded and after the thread is left, and the run writes `history-record.json`.

use std::time::Duration;

use gpui_kit::*;
use t3code_client_runtime::operations::SendMessage;
use t3code_contracts::ModelSelection;
use t3code_web::components::chat::messages_timeline::{TimelineAction, TimelineHistory};
use t3code_web::components::chat::messages_timeline_logic::RowKind;
use t3code_web::components::chat::timeline_entries::{EntryKind, MessageRole, TimelineInput};
use t3code_web::components::chat::virtual_list::ListViewport;

use super::live_check_families::pair_if_needed;
use super::live_check_heap::heap_mark;
use super::{Check, click_at, commands_of, key_of, load_state, wheel_event};

fn viewport(check: &mut Check) -> ListViewport {
    check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().read(cx).viewport())
}

/// The control's state when the first row is the control.
fn control(check: &mut Check) -> Option<TimelineHistory> {
    check.read(|root, cx| {
        let timeline = root.thread_view().read(cx).chat().read(cx).timeline().read(cx);
        match &timeline.rows().first()?.kind {
            RowKind::HistoryControl(history) => Some(history.clone()),
            _ => None,
        }
    })
}

fn user_turns(input: &TimelineInput) -> usize {
    input.entries.iter().filter(|e| matches!(&e.kind, EntryKind::Message(m) if m.role == MessageRole::User)).count()
}

/// Wheels up until the list is at its top.
fn scroll_to_top(check: &mut Check) {
    for _ in 0..4000 {
        let v = viewport(check);
        if v.scroll_top <= px(0.5) {
            break;
        }
        wheel_event(check, v.bounds.center(), ScrollDelta::Pixels(point(px(0.), px(400.))), TouchPhase::Moved);
        check.cx.advance_clock(Duration::from_millis(16));
        check.frame();
    }
}

/// The first row under the control that is fully on screen: its key and window top.
fn anchor_row(check: &mut Check) -> Option<(u64, Pixels)> {
    let v = viewport(check);
    let keys: Vec<u64> = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().read(cx).rows().iter().map(|r| r.key).collect());
    v.row_bounds.iter().find(|(ix, b)| *ix > 0 && b.top() >= v.bounds.top()).map(|(ix, b)| (keys[*ix], b.top()))
}

/// Where row `key` is drawn now, if on screen.
fn row_top(check: &mut Check, key: u64) -> Option<Pixels> {
    let v = viewport(check);
    let ix = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().read(cx).rows().iter().position(|r| r.key == key))?;
    v.row_bounds.iter().find(|(i, _)| *i == ix).map(|(_, b)| b.top())
}

/// Declines whatever the thread asks and waits until no turn runs, so the thread can unmount.
fn settle(check: &mut Check) -> anyhow::Result<()> {
    let timeline = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().clone());
    for _ in 0..20 {
        let input = check.shown_input();
        if !input.is_working && input.pending_approval_ids.is_empty() && input.queued.is_empty() {
            return Ok(());
        }
        if let Some(request) = input.pending_approval_ids.first().cloned() {
            // The Decline label sits inside the virtual list, which the test registry does not reach.
            check.cx.update(|cx| timeline.update(cx, |_, cx| cx.emit(TimelineAction::RespondToApproval { request, approve: false })));
        }
        check.wait(Duration::from_secs(1));
    }
    anyhow::bail!("the long thread's turns did not settle")
}

/// Clicks the control and checks the loading state, the merged page and the held row.
fn load_page(check: &mut Check, page: usize, records: &mut Vec<serde_json::Value>) -> anyhow::Result<()> {
    scroll_to_top(check);
    let before = user_turns(&check.shown_input());
    let (key, top) = anchor_row(check).ok_or_else(|| anyhow::anyhow!("no row under the control"))?;
    let at = viewport(check).row_bounds.iter().find(|(ix, _)| *ix == 0).map(|(_, b)| b.center()).ok_or_else(|| anyhow::anyhow!("the control is not drawn"))?;
    let working = check.shown_input().is_working;
    click_at(check, at)?;
    // The runtime marks the request loading on its own thread; the next frames show it.
    check.pump(|c| control(c).is_some_and(|h| h.loading) || user_turns(&c.shown_input()) > before, Duration::from_secs(30), "the request")?;
    let loading = control(check).is_some_and(|h| h.loading);
    if page == 1 {
        let shown = control(check);
        check.check("a click shows \"Loading earlier turns…\" with the button disabled", loading, format!("{shown:?}"));
        check.shot("h2-loading.png")?;
    }
    check.pump(|c| !control(c).is_some_and(|h| h.loading) && user_turns(&c.shown_input()) > before, Duration::from_secs(30), "an earlier page")?;
    check.frame();
    let after = user_turns(&check.shown_input());
    let moved = row_top(check, key).map(|now| f32::from(now - top));
    check.check(
        &format!("page {page}: the row on screen stays where it was{}", if working { " while a turn streams" } else { "" }),
        moved.is_some_and(|d| d.abs() <= 0.5),
        format!("moved {moved:?} px, user turns {before} -> {after}"),
    );
    records.push(serde_json::json!({ "page": page, "turns_before": before, "turns_after": after, "row_moved_px": moved, "loading_shown": loading, "while_streaming": working }));
    Ok(())
}

pub(super) fn run_earlier_turns(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| c.connected(), Duration::from_secs(60), "connected")?;
    check.pump(|c| c.sidebar_threads() > 0, Duration::from_secs(30), "sidebar rows")?;
    // Shots compare against the fork capture at the owner's settings (`T3CODE_*`, dark), applied
    // once the profile's own settings have loaded.
    check.wait(Duration::from_secs(1));
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    // The user messages the server stores for the thread before this run (`T3_LONG_TURNS`, read from its database).
    let expected: usize = std::env::var("T3_LONG_TURNS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let long = key_of(&state, &state.long_thread);
    let quiet = check.cx.update(|cx| {
        t3code_web::state::StateBridge::shell(cx).read(cx).snapshot().threads.iter().find(|t| t.key != long && !t.archived).map(|t| t.key.clone())
    });
    let (base_heap, _) = heap_mark("before-long");
    check.go(&long);
    check.pump(|c| !c.shown_input().entries.is_empty() && control(c).is_some(), Duration::from_secs(30), "the long thread with its history control")?;
    settle(check)?;
    check.wait(Duration::from_millis(800));

    let first = user_turns(&check.shown_input());
    let (bounded_heap, _) = heap_mark("bounded");
    let history = control(check);
    check.check(
        "the first window is the server's bounded snapshot",
        first < expected && history.as_ref().is_some_and(|h| h.has_more && !h.loading && h.error.is_none()),
        format!("{first} user turns of {expected}, control {history:?}"),
    );
    scroll_to_top(check);
    let corner = point(px(5.), viewport(check).bounds.bottom() - px(5.));
    super::hover_at(check, corner)?;
    check.shot("h1-control.png")?;
    let v = viewport(check);
    let interface = check.cx.update(|cx| { use t3code_web::ActiveAppTheme as _; cx.app_theme().fonts.interface });
    check.note(format!("interface {interface}px, timeline bounds {:?}, control row {:?}", v.bounds, v.row_bounds.first()));

    let mut records = Vec::new();
    load_page(check, 1, &mut records)?;

    // A turn streams at the end while the user pages history at the top.
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let thread = t3code_contracts::ThreadId::new(state.long_thread.as_str());
    let commands = commands_of(check, &environment_id)?;
    let mut message = SendMessage::text(thread, "Turn streaming: show the whole review again.");
    message.model_selection = Some(ModelSelection { instance_id: "devReplay".into(), model: "showcase".into(), options: None });
    check.block(async move { commands.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    check.pump(|c| c.shown_input().is_working, Duration::from_secs(20), "the streaming turn")?;
    load_page(check, 2, &mut records)?;

    let mut page = 2;
    while control(check).is_some_and(|h| h.has_more) {
        page += 1;
        anyhow::ensure!(page < 100, "history never ended");
        load_page(check, page, &mut records)?;
    }
    // The showcase turn stops at an approval; declining ends it.
    settle(check)?;
    let total = user_turns(&check.shown_input());
    let last = control(check);
    check.check(
        "the last page removes the control and every turn is loaded",
        last.is_none() && total == expected + 1,
        format!("control {last:?}, {total} user turns, the server holds {} with the streamed one", expected + 1),
    );
    let first_text = check.read(|root, cx| {
        let input = root.thread_view().read(cx).chat().read(cx).timeline().read(cx).input().clone();
        input.entries.iter().find_map(|e| match &e.kind {
            EntryKind::Message(m) if m.role == MessageRole::User => Some(m.text.to_string()),
            _ => None,
        })
    });
    check.note(format!("oldest loaded user message: {first_text:?}"));
    scroll_to_top(check);
    check.shot("h3-complete.png")?;
    let (loaded_heap, _) = heap_mark("long-loaded");
    check.note(format!("heap with the whole thread loaded: {loaded_heap:.1} MiB (before {base_heap:.1})"));

    if let Some(quiet) = quiet {
        check.go(&quiet);
    }
    // A thread that ran is kept mounted until the keep-alive sweep sees it idle.
    let mut left_heap = loaded_heap;
    for second in 1..=12 {
        check.wait(Duration::from_secs(1));
        if second % 3 == 0 {
            left_heap = heap_mark(&format!("after-leaving-{second}s")).0;
        }
    }
    // glibc keeps freed chunks counted while its arenas stay fragmented, so the holders are read
    // directly; a heap profile (`T3_HEAP_PROFILE`, `T3_HEAP_FROM=bounded`) shows what is still live.
    let (long_env, long_thread) = (environment_id.clone(), t3code_contracts::ThreadId::new(state.long_thread.as_str()));
    let held = check.cx.update(|cx| {
        let runtime = t3code_web::state::StateBridge::environment(&long_env, cx).map(|env| env.read(cx).store.held_threads());
        let (live, resume) = runtime.unwrap_or_default();
        let views = t3code_web::state::StateBridge::threads(cx).read(cx).cached_keys().any(|key| key == &long);
        (live.contains(&long_thread), resume.contains(&long_thread), views)
    });
    check.check(
        "leaving the thread gives its history back: no runtime mount, resume entry or mapped view holds it",
        held == (false, false, false),
        format!("(mounted, resume cache, mapped view) = {held:?}; heap {left_heap:.1} MiB after leaving, {loaded_heap:.1} loaded, {bounded_heap:.1} at the first window, {base_heap:.1} before"),
    );

    // Coming back starts from the bounded window again, with the control.
    check.go(&long);
    check.pump(|c| control(c).is_some(), Duration::from_secs(30), "the control after coming back")?;
    let again = user_turns(&check.shown_input());
    check.check("coming back shows the bounded window and the control again", again < total, format!("{again} user turns"));

    let record = serde_json::json!({ "first_window_turns": first, "pages": records, "total_turns": total, "heap_mib": { "before": base_heap, "bounded": bounded_heap, "loaded": loaded_heap, "after_leaving": left_heap }, "revisit_turns": again });
    std::fs::write(check.out.join("history-record.json"), serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}
