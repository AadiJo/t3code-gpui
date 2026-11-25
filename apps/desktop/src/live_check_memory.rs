//! `live-check memory`: the client's memory over a working session on two servers. Launches and
//! pairs both (`T3_PAIRING_URL`, `T3_PAIRING_URL_B`), opens the ten most recently updated
//! threads in turn, then loads all of the seeded long thread's history (`live-state.json` from
//! `seed`) and scrolls it from its end to its top and back by wheel, timing each event and draw. Each step notes heap in use and
//! resident memory (`live_check_heap::heap_mark`, with a heap profile under `T3_HEAP_PROFILE`)
//! and the run writes `memory-record.json`. `memory-seed` first extends the long thread to
//! `T3_LONG_TURNS` turns (default 150) on the seeding profile.

use std::time::{Duration, Instant};

use gpui_kit::*;
use t3code_web::state::StateBridge;

use super::live_check_families::{all_connected, frame_summary, pair_if_needed};
use super::live_check_heap::heap_mark;
use super::{Check, commands_of, key_of, load_state, ms, wheel_event};

/// One step's memory.
#[derive(serde::Serialize)]
struct Step {
    step: String,
    heap_mib: f64,
    rss_mib: f64,
}

fn mark(check: &mut Check, steps: &mut Vec<Step>, step: &str) {
    let (heap_mib, rss_mib) = heap_mark(step);
    check.note(format!("{step}: heap {heap_mib:.1} MiB, resident {rss_mib:.1} MiB"));
    steps.push(Step { step: step.to_owned(), heap_mib, rss_mib });
}

fn timeline_viewport(check: &mut Check) -> t3code_web::components::chat::virtual_list::ListViewport {
    check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().read(cx).viewport())
}

/// Wheels the timeline by `dy` per event until `done`; returns each event-and-draw time.
fn scroll_until(check: &mut Check, dy: f32, done: impl Fn(&t3code_web::components::chat::virtual_list::ListViewport) -> bool) -> Vec<f64> {
    let mut frames = Vec::new();
    for _ in 0..4000 {
        let viewport = timeline_viewport(check);
        if done(&viewport) {
            break;
        }
        let at = viewport.bounds.center();
        let started = Instant::now();
        wheel_event(check, at, ScrollDelta::Pixels(point(px(0.), px(dy))), TouchPhase::Moved);
        check.cx.advance_clock(Duration::from_millis(16));
        check.frame();
        frames.push(ms(started.elapsed()));
    }
    frames
}

/// Sends turns to the long thread until it has `T3_LONG_TURNS`, each waited out.
pub(super) fn run_memory_seed(check: &mut Check) -> anyhow::Result<()> {
    use t3code_client_runtime::operations::SendMessage;
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(60), "connected")?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let thread = t3code_contracts::ThreadId::new(state.long_thread.as_str());
    let commands = commands_of(check, &environment_id)?;
    let wanted: usize = std::env::var("T3_LONG_TURNS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let latest = |c: &mut Check| {
        let (env, thread) = (environment_id.clone(), thread.clone());
        c.cx.update(|cx| {
            StateBridge::shell(cx).read(cx).presented().iter().find(|row| row.environment_id == env && row.id() == &thread).and_then(|row| {
                let run = row.latest_run.as_ref()?;
                Some((run.run_id.to_string(), run.completed_at.is_some()))
            })
        })
    };
    for turn in 21..=wanted {
        let before = latest(check).map(|(id, _)| id);
        let (commands, message) = (commands.clone(), SendMessage::text(thread.clone(), format!("Turn {turn}: keep going with the review.")));
        check.block(async move { commands.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        check.pump(move |c| latest(c).is_some_and(|(id, done)| Some(&id) != before.as_ref() && done), Duration::from_secs(60), "turn settled")?;
        if turn % 10 == 0 {
            check.note(format!("long thread at {turn} turns"));
        }
    }
    Ok(())
}

pub(super) fn run_memory(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    let mut steps = Vec::new();
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    check.pump(|c| all_connected(c, 2), Duration::from_secs(60), "both servers connected")?;
    check.pump(|c| c.sidebar_threads() > 0, Duration::from_secs(30), "sidebar rows")?;
    check.wait(Duration::from_secs(2));
    mark(check, &mut steps, "launch");

    let long = key_of(&state, &state.long_thread);
    let mut recent: Vec<(i64, t3code_web::components::sidebar::model::ThreadKey)> = check.cx.update(|cx| {
        StateBridge::shell(cx).read(cx).snapshot().threads.iter().filter(|t| !t.archived).map(|t| (t.updated_at.0, t.key.clone())).collect()
    });
    recent.sort_by(|a, b| b.0.cmp(&a.0));
    let mut threads: Vec<_> = recent.into_iter().map(|(_, key)| key).filter(|key| key != &long).take(9).collect();
    threads.insert(0, long.clone());
    let mut opens = Vec::new();
    for (index, key) in threads.iter().enumerate() {
        opens.push(ms(check.navigate_timed(key)));
        check.wait(Duration::from_millis(1500));
        mark(check, &mut steps, &format!("thread-{}", index + 1));
    }
    check.note(format!("opened {} threads: {:?} ms (event and draw)", opens.len(), opens.iter().map(|v| (v * 100.).round() / 100.).collect::<Vec<_>>()));

    check.go(&long);
    check.pump(|c| !c.shown_input().entries.is_empty(), Duration::from_secs(30), "the long thread's timeline")?;
    check.wait(Duration::from_millis(1500));
    // Pages through the thread command the "Load earlier turns" control sends (`earlier-turns` clicks it).
    let mut pages = 0;
    loop {
        let task = check.cx.update(|cx| StateBridge::threads(cx).read(cx).active().cloned().map(|thread| thread.update(cx, |thread, cx| thread.load_earlier(cx))));
        let Some(task) = task else { break };
        match check.wait_task(task)? {
            t3code_client_runtime::state::threads::LoadEarlierResult::Loaded => pages += 1,
            t3code_client_runtime::state::threads::LoadEarlierResult::Busy => check.wait(Duration::from_millis(200)),
            _ => break,
        }
        check.wait(Duration::from_millis(300));
        anyhow::ensure!(pages < 200, "history never ended");
    }
    check.note(format!("loaded {pages} earlier history pages"));
    mark(check, &mut steps, "long-loaded");
    let rows = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().read(cx).rows().len());
    let up = scroll_until(check, 120., |v| v.scroll_top <= px(0.5));
    let top = timeline_viewport(check);
    check.check("the long thread scrolls to its top", top.scroll_top <= px(0.5), format!("scroll top {:?} after {} events, {rows} rows", top.scroll_top, up.len()));
    mark(check, &mut steps, "long-top");
    let down = scroll_until(check, -120., |v| v.distance_from_end() <= px(0.5));
    let end = timeline_viewport(check);
    check.check("the long thread scrolls back to its end", end.distance_from_end() <= px(0.5), format!("{:?} from the end after {} events", end.distance_from_end(), down.len()));
    mark(check, &mut steps, "long-end");
    check.note(format!("timeline wheel up: {}", frame_summary(&up)));
    check.note(format!("timeline wheel down: {}", frame_summary(&down)));

    let quiet = threads.last().cloned().unwrap_or(long);
    check.go(&quiet);
    check.wait(Duration::from_secs(2));
    mark(check, &mut steps, "after-long");

    let record = serde_json::json!({ "steps": steps, "open_ms": opens, "wheel_up_ms": up, "wheel_down_ms": down, "timeline_rows": rows });
    std::fs::write(check.out.join("memory-record.json"), serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}
