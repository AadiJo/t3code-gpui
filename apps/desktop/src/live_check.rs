// End-to-end check of the state bridge against a real T3 Code server: the live scene in a
// headless window, driven like a user, with screenshots and timings. Phases run in order as
// separate processes on one data directory (`T3_LIVE_DATA_DIR`):
//
//   seed     pairs (`T3_PAIRING_URL`), creates projects and threads with the Replay provider,
//            one long thread, and pins, settles and snoozes a few
//   drive    sidebar sections, cold / cached thread switches, send and stream, approve,
//            queue, rename and pin from the context menu
//   restart  a new process paints the sidebar and the last thread from cache before the socket
//            connects, then catches up
//   bench    offline (server stopped): mapping a 3,000-row thread, resident memory with a
//            5,000-thread shell and 20 mapped threads
//
// Usage: live-check <seed|drive|restart|bench> <out-dir>
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use t3code_client_runtime::connection::ConnectionPhase;
use t3code_client_runtime::connection::onboarding::parse_pairing_url;
use t3code_client_runtime::operations::{CreateThread, EnvironmentCommands, SendMessage};
use t3code_client_runtime::state::cache_store::{CacheStore, CachedThread};
use t3code_client_runtime::state::models::{format_ms, now_ms};
use t3code_client_runtime::state::sidebar::{PartitionOptions, partition, row_status};
use t3code_client_runtime::state::thread_shell::navigation_rows;
use t3code_client_runtime::state::ThreadStatus;
use t3code_contracts::{
    CommandId, ModelSelection, OrchestrationV2TurnItem, ProjectId, ProjectMutation, ProjectMutationProjectCreate, ProviderInteractionMode,
    RuntimeMode, ThreadId, TurnItemId,
};
use t3code_shared::thread_projection::VisibleTurnItem;
use t3code_web::components::chat::timeline_entries::TimelineInput;
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::composer_logic::ComposerSubmissionIntent;
use t3code_web::connection::{ConnectionRuntime, ConnectionsState};
use t3code_web::scenes::live::{LiveApp, data_dir};
use t3code_web::session_logic::{TimelineMapper, timeline_input};
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};

const STATE_FILE: &str = "live-state.json";

/// What `seed` made, for the later phases.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Seeded {
    environment_id: String,
    long_thread: String,
    approval_thread: String,
    queue_thread: String,
    other_threads: Vec<String>,
    last_thread: Option<String>,
}

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
    handle: AnyWindowHandle,
    app: Entity<LiveApp>,
}

fn connected(state: &ConnectionsState) -> bool {
    !state.environments.is_empty() && state.environments.iter().all(|env| env.status.as_ref().is_some_and(|s| s.phase == ConnectionPhase::Connected))
}

impl Check {
    fn note(&mut self, text: impl AsRef<str>) {
        let line = format!("{:>8.3}s  {}", self.started.elapsed().as_secs_f64(), text.as_ref());
        println!("{line}");
        writeln!(self.log, "{line}").ok();
    }

    /// One frame; returns its render time.
    fn frame(&mut self) -> anyhow::Result<Duration> {
        self.cx.run_until_parked();
        let started = Instant::now();
        self.cx.update_window(self.handle, |_, w, cx| w.render_frame(cx))?;
        Ok(started.elapsed())
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame()?;
        self.frame()?;
        let image = self.cx.capture_screenshot(self.handle)?;
        image.save(self.out.join(name))?;
        self.note(format!("screenshot {name}"));
        Ok(())
    }

    fn pump(&mut self, until: impl Fn(&mut Self) -> bool, timeout: Duration, what: &str) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.frame()?;
            if until(self) {
                self.note(format!("ok: {what}"));
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn read<R>(&mut self, f: impl FnOnce(&LiveApp, &App) -> R) -> R {
        let app = self.app.clone();
        self.cx.update(|cx| f(app.read(cx), cx))
    }

    fn connected(&mut self) -> bool {
        self.cx.update(|cx| connected(ConnectionRuntime::state(cx).read(cx)))
    }

    fn sidebar_threads(&mut self) -> usize {
        self.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.len())
    }

    fn active_input(&mut self) -> Option<TimelineInput> {
        self.read(|app, cx| app.thread().map(|t| t.read(cx).input().clone()))
    }

    /// The sidebar list index of a thread row.
    fn row_index(&mut self, key: &ThreadKey) -> Option<usize> {
        self.read(|app, cx| {
            app.sidebar().read(cx).list().entries.iter().position(|entry| {
                matches!(entry, t3code_web::components::sidebar_logic::SidebarListEntry::Thread { thread, .. } if &thread.key == key)
            })
        })
    }

    /// Clicks a sidebar row and returns (handler time, click-to-frame).
    fn click_row(&mut self, key: &ThreadKey, label: &str) -> anyhow::Result<(Duration, Duration)> {
        self.frame()?;
        let ix = self.row_index(key).with_context(|| format!("no row for {label}"))?;
        let bounds = self.read(|app, cx| app.sidebar().read(cx).entry_bounds(ix)).context("row not laid out")?;
        let at = bounds.center();
        use gpui_kit::test::TestEventExt as _;
        let started = Instant::now();
        self.cx.update_window(self.handle, |_, w, cx| {
            w.pointer_move(at, None, cx);
            w.pointer_down(at, MouseButton::Left, cx);
            w.pointer_up(at, MouseButton::Left, cx);
        })?;
        let handler = started.elapsed();
        self.cx.update_window(self.handle, |_, w, cx| w.render_frame(cx))?;
        let to_frame = started.elapsed();
        let t = self.read(|app, _| app.timing());
        self.note(format!(
            "click {label}: handler {:.2} ms (open {:.2}, timeline {:.2}, composer {:.2}), click-to-frame {:.2} ms",
            ms(handler),
            t.open_micros as f64 / 1000.0,
            t.timeline_micros as f64 / 1000.0,
            t.composer_micros as f64 / 1000.0,
            ms(to_frame)
        ));
        Ok((handler, to_frame))
    }

    fn click_point(&mut self, what: &str, at: Point<Pixels>) -> anyhow::Result<()> {
        use gpui_kit::test::TestEventExt as _;
        self.frame()?;
        self.cx.update_window(self.handle, |_, w, cx| {
            w.pointer_move(at, None, cx);
            w.pointer_down(at, MouseButton::Left, cx);
            w.pointer_up(at, MouseButton::Left, cx);
        })?;
        self.frame()?;
        self.note(format!("click {what} at {at:?}"));
        Ok(())
    }

    fn type_and_send(&mut self, text: &str, intent: ComposerSubmissionIntent) -> anyhow::Result<()> {
        let composer = self.read(|app, _| app.composer().cloned()).context("no composer")?;
        let text = text.to_owned();
        self.cx.update(|cx| {
            composer.update(cx, |composer, cx| {
                composer.editor().update(cx, |editor, cx| editor.set_text(text.clone(), cx));
                composer.submit(intent, cx);
            })
        });
        self.frame()?;
        self.note(format!("sent {text:?} ({intent:?})"));
        Ok(())
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn key_of(state: &Seeded, thread: &str) -> ThreadKey {
    thread_key(&t3code_contracts::EnvironmentId::new(state.environment_id.as_str()), &ThreadId::new(thread))
}

fn open_app(out: &Path) -> anyhow::Result<Check> {
    std::fs::create_dir_all(out)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
    });
    let (handle, app) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(1280.), px(860.)), cx)),
            show: false,
            focus: true,
            ..component::TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| LiveApp::new(window, cx)))
    })?;
    Ok(Check { cx, out: out.to_owned(), started: Instant::now(), log: String::new(), handle, app })
}

fn load_state(out: &Path) -> anyhow::Result<Seeded> {
    Ok(serde_json::from_slice(&std::fs::read(out.join(STATE_FILE)).context("run seed first")?)?)
}

fn save_state(out: &Path, state: &Seeded) -> anyhow::Result<()> {
    Ok(std::fs::write(out.join(STATE_FILE), serde_json::to_vec_pretty(state)?)?)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: live-check <seed|drive|restart|bench> <out-dir>";
    let phase = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    if phase == "bench" {
        prepare_bench_caches(&out)?;
    }
    let mut check = open_app(&out)?;
    check.note(format!("phase {phase}, data dir {}", data_dir().display()));
    let result = match phase.as_str() {
        "seed" => run_seed(&mut check),
        "drive" => run_drive(&mut check),
        "restart" => run_restart(&mut check),
        "bench" => run_bench(&mut check),
        other => bail!("unknown phase {other}"),
    };
    if let Err(error) = &result {
        check.note(format!("FAILED: {error:#}"));
        check.shot(&format!("{phase}-failure.png")).ok();
    }
    check.note(format!("resident memory {} MiB", rss_mib()));
    std::fs::write(out.join(format!("{phase}-transcript.txt")), &check.log)?;
    check.cx.update(ConnectionRuntime::shutdown);
    result
}

fn rss_mib() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(|l| l.split_whitespace().nth(1).unwrap_or("0").to_owned()))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or_else(|| "?".into(), |kb| format!("{:.1}", kb / 1024.0))
}

/// Runs a future on the bridge's tokio runtime and pumps the app until it resolves.
fn block<T: Send + 'static>(check: &mut Check, future: impl std::future::Future<Output = T> + Send + 'static) -> anyhow::Result<T> {
    let task = check.cx.update(|cx| StateBridge::spawn(future, cx));
    wait_task(check, task)?.context("runtime stopped")
}

/// Pumps the app until a GPUI task resolves.
fn wait_task<T: 'static>(check: &mut Check, task: Task<T>) -> anyhow::Result<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    check.cx.update(|cx| {
        cx.spawn(async move |_| {
            _ = sender.send(task.await);
        })
        .detach()
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        check.cx.run_until_parked();
        if let Ok(value) = receiver.try_recv() {
            return Ok(value);
        }
        if Instant::now() > deadline {
            bail!("timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_seed(check: &mut Check) -> anyhow::Result<()> {
    let paired = check.cx.update(|cx| !ConnectionRuntime::state(cx).read(cx).environments.is_empty());
    if !paired {
        let url = std::env::var("T3_PAIRING_URL").context("T3_PAIRING_URL")?;
        let request = parse_pairing_url(&url)?;
        let pairing = check.cx.update(|cx| ConnectionRuntime::global(cx).pair(request, None, cx));
        let label = wait_task(check, pairing)?.map_err(|e| anyhow::anyhow!("{}", e.0))?;
        check.note(format!("paired {label}"));
    }
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    let environment_id = t3code_contracts::EnvironmentId::new(id.as_str());
    check.pump(|c| c.cx.update(|cx| StateBridge::environment(&environment_id, cx).is_some()), Duration::from_secs(10), "store open")?;
    let commands: EnvironmentCommands =
        check.cx.update(|cx| StateBridge::environment(&environment_id, cx).map(|env| env.read(cx).commands.clone())).context("store")?;
    let workspaces = out_workspaces(&check.out);
    let mut projects = Vec::new();
    for name in ["atlas", "harbor", "meridian"] {
        let project_id = ProjectId::new(uuid::Uuid::new_v4().to_string());
        let root = workspaces.join(format!("{name}-{}", now_ms()));
        let commands = commands.clone();
        let mutation = ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
            command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
            project_id: project_id.clone(),
            title: name.into(),
            workspace_root: root.display().to_string(),
            create_workspace_root_if_missing: Some(true),
            default_model_selection: None,
            scripts: None,
        });
        block(check, async move { commands.mutate_project(mutation).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        check.note(format!("created project {name}"));
        projects.push(project_id);
    }
    let create = |check: &mut Check, project: &ProjectId, title: &str, model: &str| -> anyhow::Result<ThreadId> {
        let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
        let input = CreateThread {
            thread_id: thread_id.clone(),
            project_id: project.clone(),
            title: title.into(),
            model_selection: ModelSelection { instance_id: "devReplay".into(), model: model.into(), options: None },
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Default,
            branch: None,
            worktree_path: None,
        };
        let commands = commands.clone();
        block(check, async move { commands.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        check.note(format!("created thread {title}"));
        Ok(thread_id)
    };
    let send_and_settle = |check: &mut Check, thread: &ThreadId, text: &str| -> anyhow::Result<()> {
        let latest = |c: &mut Check, env: &t3code_contracts::EnvironmentId, thread: &ThreadId| {
            c.cx.update(|cx| {
                StateBridge::shell(cx)
                    .read(cx)
                    .presented()
                    .iter()
                    .find(|row| &row.environment_id == env && row.id() == thread)
                    .map(|row| (row.latest_run.as_ref().map(|r| (r.run_id.clone(), r.completed_at.is_some())), row_status(row)))
            })
        };
        let before = latest(check, &environment_id, thread).and_then(|(run, _)| run).map(|(id, _)| id);
        let commands = commands.clone();
        let message = SendMessage::text(thread.clone(), text);
        block(check, async move { commands.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        let env = environment_id.clone();
        let thread = thread.clone();
        check.pump(
            move |c| {
                let seen = latest(c, &env, &thread);
                seen.is_some_and(|(run, status)| {
                    run.is_some_and(|(id, done)| Some(&id) != before.as_ref() && done)
                        && !matches!(status, t3code_client_runtime::state::sidebar::ThreadStatus::Working | t3code_client_runtime::state::sidebar::ThreadStatus::Preparing)
                })
            },
            Duration::from_secs(60),
            "turn settled",
        )
    };
    let long = create(check, &projects[0], "Long design review", "multi-turn")?;
    for turn in 1..=30 {
        send_and_settle(check, &long, &format!("Turn {turn}: keep going with the review."))?;
    }
    let mut others = Vec::new();
    for (ix, title) in ["Sidebar polish", "Cache audit", "Release notes", "Flaky test hunt", "Docs pass", "Perf budget"].iter().enumerate() {
        let thread = create(check, &projects[ix % projects.len()], title, "multi-turn")?;
        send_and_settle(check, &thread, &format!("Start on {title}."))?;
        others.push(thread);
    }
    let approval = create(check, &projects[1], "Approval flow", "approval")?;
    let queue = create(check, &projects[2], "Queue flow", "multi-turn")?;
    // Fill the pinned, settled and snoozed sections.
    let (pin, settle, snooze) = (others[0].clone(), others[1].clone(), others[2].clone());
    let c = commands.clone();
    block(check, async move {
        c.pin(&pin, None).await?;
        c.settle(&settle).await?;
        c.snooze(&snooze, format_ms(now_ms() + 3 * 3_600_000)).await
    })?
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    check.note("pinned, settled and snoozed one thread each");
    let state = Seeded {
        environment_id: id.to_string(),
        long_thread: long.to_string(),
        approval_thread: approval.to_string(),
        queue_thread: queue.to_string(),
        other_threads: others.iter().map(ToString::to_string).collect(),
        last_thread: None,
    };
    save_state(&check.out, &state)?;
    check.pump(|c| c.sidebar_threads() >= 9, Duration::from_secs(10), "sidebar lists the seeded threads")?;
    check.shot("00-seeded.png")
}

fn out_workspaces(out: &Path) -> PathBuf {
    out.parent().map_or_else(|| out.join("workspaces"), |parent| parent.join("server").join("workspaces"))
}

/// The runtime's sections next to the sidebar view's, for the transcript.
fn compare_sections(check: &mut Check) -> String {
    check.cx.update(|cx| {
        let shell = StateBridge::shell(cx);
        let shell = shell.read(cx);
        let listed = navigation_rows(shell.presented(), &Default::default());
        let refs: Vec<_> = listed.iter().map(|row| row.as_ref()).collect();
        let sections = partition(&refs, now_ms(), |_| true, None, PartitionOptions::default());
        let titles = |rows: &[&t3code_client_runtime::state::models::ThreadRow]| rows.iter().map(|r| r.title().to_owned()).collect::<Vec<_>>();
        format!(
            "runtime sections: pinned {:?}; active {:?}; snoozed {:?}; settled {:?}",
            titles(&sections.pinned),
            titles(&sections.active),
            titles(&sections.snoozed),
            titles(&sections.settled)
        )
    })
}

fn view_sections(check: &mut Check) -> String {
    check.read(|app, cx| {
        let mut by_section: HashMap<String, Vec<String>> = HashMap::new();
        for entry in &app.sidebar().read(cx).list().entries {
            if let t3code_web::components::sidebar_logic::SidebarListEntry::Thread { thread, section, .. } = entry {
                by_section.entry(format!("{section:?}")).or_default().push(thread.title.to_string());
            }
        }
        let mut keys: Vec<_> = by_section.into_iter().collect();
        keys.sort();
        format!("view sections: {keys:?}")
    })
}

fn row_status_of(check: &mut Check, key: &ThreadKey) -> String {
    check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(key).map(|row| format!("{:?}", row_status(row))).unwrap_or_default())
}

fn run_drive(check: &mut Check) -> anyhow::Result<()> {
    let mut state = load_state(&check.out)?;
    check.pump(|c| c.sidebar_threads() >= 9, Duration::from_secs(20), "sidebar rows")?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    check.shot("01-sidebar.png")?;
    let runtime = compare_sections(check);
    check.note(runtime);
    let view = view_sections(check);
    check.note(view);

    // A cold thread: nothing in memory or on disk.
    let long = key_of(&state, &state.long_thread);
    let (_, cold) = check.click_row(&long, "long thread (cold)")?;
    let source = check.cx.update(|cx| StateBridge::threads(cx).read(cx).last_switch());
    check.note(format!("cold switch source {source:?}, click-to-frame {:.2} ms", ms(cold)));
    let clicked = Instant::now();
    check.pump(
        |c| c.read(|app, cx| app.thread().is_some_and(|t| t.read(cx).status() == ThreadStatus::Live && !t.read(cx).input().entries.is_empty())),
        Duration::from_secs(20),
        "long thread live",
    )?;
    check.note(format!("cold thread: full timeline {:.1} ms after the click frame", ms(clicked.elapsed())));
    let rows = check.active_input().map_or(0, |i| i.entries.len());
    check.note(format!("long thread entries {rows}"));
    check.shot("02-long-thread.png")?;

    // Another thread, then back: memory-cached.
    let other = key_of(&state, &state.other_threads[3]);
    check.click_row(&other, "other thread (cold)")?;
    check.pump(|c| c.read(|app, cx| app.thread().is_some_and(|t| t.read(cx).status() == ThreadStatus::Live)), Duration::from_secs(20), "other live")?;
    let (_, cached) = check.click_row(&long, "long thread (memory-cached)")?;
    // Baseline: the same click on the row already open does no switch; the difference is the
    // switch's own cost, the rest is pointer dispatch and the sidebar's click handling.
    check.frame()?;
    let (_, noop) = check.click_row(&long, "long thread again (already open, no switch)")?;
    check.note(format!("no-op click-to-frame {:.2} ms (switch adds {:.2} ms)", ms(noop), ms(cached.saturating_sub(noop))));
    let source = check.cx.update(|cx| StateBridge::threads(cx).read(cx).last_switch());
    let entries = check.active_input().map_or(0, |i| i.entries.len());
    check.note(format!("memory-cached switch: source {source:?}, click-to-frame {:.2} ms, entries at first frame {entries}", ms(cached)));
    check.shot("03-long-thread-cached.png")?;

    // Send, watch the reply stream, the row status change.
    let queue_key = key_of(&state, &state.queue_thread);
    check.click_row(&queue_key, "queue thread")?;
    check.pump(|c| c.read(|app, cx| app.thread().is_some_and(|t| t.read(cx).status() == ThreadStatus::Live)), Duration::from_secs(20), "queue thread live")?;
    let before = check.read(|app, _| (app.text_updates, app.structure_updates));
    check.type_and_send("Walk me through the cache layers.", ComposerSubmissionIntent::Foreground)?;
    check.shot("04-sent.png")?;
    let stream_started = Instant::now();
    let mut statuses = vec![row_status_of(check, &queue_key)];
    let mut frame_times = Vec::new();
    let mut shot_mid = false;
    let mut queued = false;
    let mut shot_queued = false;
    while stream_started.elapsed() < Duration::from_secs(30) {
        frame_times.push(check.frame()?);
        let status = row_status_of(check, &queue_key);
        if statuses.last() != Some(&status) {
            check.note(format!("sidebar row status → {status}"));
            statuses.push(status);
        }
        let working = check.active_input().is_some_and(|i| i.is_working);
        if working && !queued {
            check.type_and_send("Then summarise it in three bullets.", ComposerSubmissionIntent::Alternate)?;
            queued = true;
        }
        if working && !shot_mid && stream_started.elapsed() > Duration::from_millis(150) {
            check.shot("05-streaming.png")?;
            shot_mid = true;
        }
        if queued && !shot_queued && check.active_input().is_some_and(|i| !i.queued.is_empty()) {
            check.shot("06-queued.png")?;
            shot_queued = true;
        }
        if !working && stream_started.elapsed() > Duration::from_secs(2) && check.active_input().is_some_and(|i| i.queued.is_empty()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let elapsed = stream_started.elapsed().as_secs_f64();
    let after = check.read(|app, _| (app.text_updates, app.structure_updates));
    frame_times.sort();
    let p50 = frame_times.get(frame_times.len() / 2).copied().unwrap_or_default();
    let p95 = frame_times.get(frame_times.len() * 95 / 100).copied().unwrap_or_default();
    let max = frame_times.last().copied().unwrap_or_default();
    check.note(format!(
        "streaming: {} text updates and {} structure updates in {elapsed:.2} s ({:.1} view updates/s); frame p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms over {} frames",
        after.0 - before.0,
        after.1 - before.1,
        (after.0 - before.0 + after.1 - before.1) as f64 / elapsed,
        ms(p50),
        ms(p95),
        ms(max),
        frame_times.len()
    ));
    check.note(format!("row statuses seen: {statuses:?}"));
    check.shot("07-after-stream.png")?;

    // Approve a request.
    let approval_key = key_of(&state, &state.approval_thread);
    check.click_row(&approval_key, "approval thread")?;
    check.pump(|c| c.read(|app, cx| app.thread().is_some_and(|t| t.read(cx).status() == ThreadStatus::Live)), Duration::from_secs(20), "approval thread live")?;
    check.type_and_send("Write the probe file.", ComposerSubmissionIntent::Foreground)?;
    check.pump(|c| c.active_input().is_some_and(|i| !i.pending_approval_ids.is_empty()), Duration::from_secs(30), "approval requested")?;
    let status = row_status_of(check, &approval_key);
    check.note(format!("sidebar row status with approval pending: {status}"));
    check.shot("08-approval.png")?;
    let entry_id = check
        .active_input()
        .and_then(|input| {
            input.entries.iter().rev().find_map(|entry| match &entry.kind {
                t3code_web::components::chat::timeline_entries::EntryKind::Work(work)
                    if matches!(work.item, t3code_web::components::chat::timeline_entries::WorkItem::Approval { .. }) =>
                {
                    Some(work.id.0.to_string())
                }
                _ => None,
            })
        })
        .context("approval row")?;
    let _ = entry_id;
    // The test registry does not reach ids inside the virtual list, so click the Approve
    // label where the row draws it (located on 08-approval.png: "Waiting for approval ·
    // Decline Approve More" under the command, window points).
    let at = point(px(643.), px(278.));
    {
        use gpui_kit::test::TestEventExt as _;
        check.frame()?;
        check.cx.update_window(check.handle, |_, w, cx| {
            w.pointer_move(at, None, cx);
            w.pointer_down(at, MouseButton::Left, cx);
            w.pointer_up(at, MouseButton::Left, cx);
        })?;
    }
    check.note(format!("clicked Approve at {at:?}"));
    check.pump(|c| c.active_input().is_some_and(|i| i.pending_approval_ids.is_empty()), Duration::from_secs(30), "approval answered")?;
    check.shot("09-approved.png")?;

    // Pin, then rename, from the context menu.
    let target = key_of(&state, &state.other_threads[4]);
    let ix = check.row_index(&target).context("row")?;
    let sidebar = check.read(|app, _| app.sidebar().clone());
    let at = check.read(|app, cx| app.sidebar().read(cx).entry_bounds(ix)).context("bounds")?.center();
    check.cx.update(|cx| sidebar.update(cx, |s, cx| s.open_context_menu_at(ix, at, cx)));
    check.shot("10-context-menu.png")?;
    // Thread menu without a branch: Pin, Settle, Snooze, Rename, … The test registry does not
    // reach the deferred menu's ids, so click where 10-context-menu.png shows "Pin thread".
    check.click_point("Pin thread", point(px(211.), px(399.)))?;
    check.pump(
        |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.iter().any(|t| t.key == target && t.pinned_at.is_some())),
        Duration::from_secs(10),
        "pinned in the sidebar",
    )?;
    check.shot("11-pinned.png")?;
    // Rename: the menu row emits this event; send it the same way.
    check.cx.update(|cx| {
        sidebar.update(cx, |_, cx| {
            cx.emit(t3code_web::components::sidebar::SidebarEvent::ThreadCommand(
                vec![target.clone()],
                t3code_web::components::sidebar::context_menu::MenuCommand::Rename,
            ))
        })
    });
    check.note("sidebar event: Rename thread");
    check.frame()?;
    let field = check.read(|app, _| app.rename_field().cloned()).context("rename field")?;
    check.cx.update_window(check.handle, |_, w, cx| {
        field.update(cx, |field, cx| field.set_value("Docs pass (renamed)", w, cx));
    })?;
    check.shot("12-rename-typed.png")?;
    let app = check.app.clone();
    check.cx.update(|cx| app.update(cx, |app, cx| app.confirm_rename(cx)));
    check.pump(
        |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.iter().any(|t| t.title.as_ref() == "Docs pass (renamed)")),
        Duration::from_secs(10),
        "renamed in the sidebar",
    )?;
    check.shot("13-renamed.png")?;
    let view = view_sections(check);
    check.note(view);

    // Switch away and back again, then end on the long thread for the restart phase.
    let (_, back) = check.click_row(&queue_key, "queue thread (memory-cached)")?;
    let source = check.cx.update(|cx| StateBridge::threads(cx).read(cx).last_switch());
    check.note(format!("memory-cached switch: source {source:?}, click-to-frame {:.2} ms", ms(back)));
    let (_, back) = check.click_row(&long, "long thread (memory-cached)")?;
    let source = check.cx.update(|cx| StateBridge::threads(cx).read(cx).last_switch());
    check.note(format!("memory-cached switch: source {source:?}, click-to-frame {:.2} ms", ms(back)));
    check.shot("14-back-to-long.png")?;
    state.last_thread = Some(state.long_thread.clone());
    save_state(&check.out, &state)?;
    // Give the runtime's disk writer time to persist the idle thread and shell.
    let wait = Instant::now();
    while wait.elapsed() < Duration::from_secs(12) {
        check.frame()?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn run_restart(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    let first = check.frame()?;
    let rows = check.sidebar_threads();
    let connected = check.connected();
    check.note(format!("first frame {:.2} ms: {rows} sidebar rows from cache, connected {connected}", ms(first)));
    if rows == 0 {
        bail!("the sidebar did not paint from cache");
    }
    check.shot("20-restart-sidebar-from-cache.png")?;
    let last = key_of(&state, state.last_thread.as_deref().context("drive saved no last thread")?);
    let (_, disk) = check.click_row(&last, "last thread (disk-cached)")?;
    let status = check.read(|app, cx| app.thread().map(|t| t.read(cx).status()));
    let entries = check.active_input().map_or(0, |i| i.entries.len());
    let source = check.cx.update(|cx| StateBridge::threads(cx).read(cx).last_switch());
    let connected = check.connected();
    check.note(format!(
        "disk-cached switch: source {source:?}, status {status:?}, {entries} entries, click-to-frame {:.2} ms, connected {connected}",
        ms(disk)
    ));
    check.shot("21-restart-thread-from-cache.png")?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    check.pump(|c| c.read(|app, cx| app.thread().is_some_and(|t| t.read(cx).status() == ThreadStatus::Live)), Duration::from_secs(20), "thread live")?;
    check.shot("22-restart-live.png")
}

/// A projection with its visible rows repeated to `rows`, each with a fresh item id.
fn widened(projection: &t3code_shared::thread_projection::ThreadProjection, rows: usize) -> t3code_shared::thread_projection::ThreadProjection {
    let mut wide = projection.clone();
    let source = projection.visible_turn_items.clone();
    let mut out: Vec<VisibleTurnItem> = Vec::with_capacity(rows);
    let mut n = 0usize;
    while out.len() < rows {
        for row in source.iter() {
            if out.len() >= rows {
                break;
            }
            let mut value = serde_json::to_value(&*row.item).expect("item");
            let id = format!("bench-{n}");
            value["id"] = serde_json::Value::String(id.clone());
            if value.get("messageId").is_some() {
                value["messageId"] = serde_json::Value::String(format!("bench-msg-{n}"));
            }
            let item: OrchestrationV2TurnItem = serde_json::from_value(value).expect("item");
            out.push(VisibleTurnItem { position: out.len() as u64, source_item_id: TurnItemId::new(id), item: Arc::new(item), ..row.clone() });
            n += 1;
        }
    }
    wide.visible_turn_items = Arc::new(out);
    wide
}

fn run_bench(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let cache = CacheStore::new(&data_dir());
    let cached = cache.load_thread(&environment_id, &ThreadId::new(state.long_thread.as_str())).context("long thread disk cache")?;
    let wide = widened(&cached.projection, 3_000);
    let mut times = Vec::new();
    let mut input = TimelineInput::default();
    for _ in 0..7 {
        let started = Instant::now();
        input = timeline_input(&mut TimelineMapper::default(), &wide, &[]);
        times.push(started.elapsed());
    }
    times.sort();
    check.note(format!(
        "map 3,000 rows ({} entries): median {:.2} ms, min {:.2}, max {:.2} over 7 runs",
        input.entries.len(),
        ms(times[3]),
        ms(times[0]),
        ms(times[6])
    ));
    let mut mapper = TimelineMapper::default();
    timeline_input(&mut mapper, &wide, &[]);
    let started = Instant::now();
    timeline_input(&mut mapper, &wide, &[]);
    check.note(format!("remap of the same 3,000 rows with the memo: {:.2} ms ({} mapped)", ms(started.elapsed()), mapper.last_mapped));

    // The mapping of three real threads, for comparison with the TypeScript.
    let mut dump = Vec::new();
    for thread in [&state.long_thread, &state.approval_thread, &state.queue_thread] {
        let Some(cached) = cache.load_thread(&environment_id, &ThreadId::new(thread.as_str())) else { continue };
        let input = timeline_input(&mut TimelineMapper::default(), &cached.projection, &[]);
        let entries: Vec<serde_json::Value> = input
            .entries
            .iter()
            .map(|entry| {
                use t3code_web::components::chat::timeline_entries::{EntryKind, ToolStatus};
                let (kind, label, status) = match &entry.kind {
                    EntryKind::Message(m) => ("message", m.text.to_string(), None),
                    EntryKind::Work(w) => (
                        "work",
                        w.label.to_string(),
                        Some(match w.status {
                            ToolStatus::InProgress => "inProgress",
                            ToolStatus::Completed => "completed",
                            ToolStatus::Failed => "failed",
                            ToolStatus::Declined => "declined",
                            ToolStatus::Stopped => "stopped",
                        }),
                    ),
                    EntryKind::ProposedPlan(_) => ("proposed-plan", String::new(), None),
                    EntryKind::Event(_) => ("event", String::new(), None),
                    EntryKind::SentMessage { text, .. } => ("sent-message", text.to_string(), None),
                    EntryKind::HtmlRender { .. } => ("html-render", String::new(), None),
                    EntryKind::McpApp { .. } => ("mcp-app", String::new(), None),
                };
                serde_json::json!({ "id": entry.id.0.as_ref(), "kind": kind, "label": label, "status": status })
            })
            .collect();
        let projection_path = check.out.join(format!("projection-{thread}.json"));
        std::fs::write(&projection_path, serde_json::to_vec(&*cached.projection)?)?;
        dump.push(serde_json::json!({ "thread": thread, "projection": projection_path, "entries": entries }));
    }
    std::fs::write(check.out.join("rust-timeline.json"), serde_json::to_vec_pretty(&dump)?)?;
    check.note(format!("wrote rust-timeline.json for {} threads", dump.len()));

    // The data dir holds a 5,000-row shell and 20 thread caches (written before start), and
    // the server is stopped, so both come from the disk cache.
    check.pump(|c| c.sidebar_threads() >= 5_000, Duration::from_secs(10), "5,000 rows in the sidebar")?;
    check.shot("30-bench-5000.png")?;
    let after_shell = rss_mib();
    let rebuilds = check.cx.update(|cx| StateBridge::shell(cx).read(cx).rebuild_count());
    check.note(format!("resident with the 5,000-row shell: {after_shell} MiB ({rebuilds} shell rebuilds)"));
    let mut switch_times = Vec::new();
    for n in 0..20 {
        let key = thread_key(&environment_id, &ThreadId::new(format!("bench-thread-{n}")));
        let started = Instant::now();
        check.cx.update(|cx| StateBridge::threads(cx).update(cx, |threads, cx| threads.open(key, cx)));
        check.cx.update_window(check.handle, |_, w, cx| w.render_frame(cx))?;
        switch_times.push(started.elapsed());
    }
    switch_times.sort();
    let held = check.cx.update(|cx| StateBridge::threads(cx).read(cx).cached_keys().count());
    check.note(format!(
        "opened 20 disk-cached threads: open+frame median {:.2} ms, max {:.2} ms; {held} mapped views held",
        ms(switch_times[10]),
        ms(switch_times[19])
    ));
    check.note(format!("resident with 5,000 rows and 20 mapped threads: {} MiB", rss_mib()));
    Ok(())
}

/// Writes a 5,000-row shell and 20 thread caches built from the long thread into the data dir,
/// replacing its shell cache. Run it on a copy of the data dir.
fn prepare_bench_caches(out: &Path) -> anyhow::Result<()> {
    let state = load_state(out)?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let _guard = runtime.enter();
    let cache = CacheStore::new(&data_dir());
    let cached = cache.load_thread(&environment_id, &ThreadId::new(state.long_thread.as_str())).context("long thread disk cache")?;
    let shell = cache.load_shell(&environment_id).context("shell cache")?;
    let mut snapshot = shell.snapshot.clone();
    let template = snapshot.threads.first().cloned().context("a shell row")?;
    let threads: Vec<_> = (0..5_000)
        .map(|n| {
            let mut row = (*template).clone();
            row.id = ThreadId::new(format!("bench-thread-{n}"));
            row.title = format!("Bench thread {n}");
            row.pinned_at = None;
            row.settled_override = None;
            row.snoozed_until = None;
            Arc::new(row)
        })
        .collect();
    snapshot.threads = Arc::new(threads);
    cache.save_shell(&environment_id, &snapshot, shell.origin.as_ref(), cache.ticket());
    for n in 0..20 {
        let mut projection = (*cached.projection).clone();
        let mut thread = (*projection.thread).clone();
        thread.id = ThreadId::new(format!("bench-thread-{n}"));
        projection.thread = Arc::new(thread);
        cache.save_thread(&environment_id, &CachedThread { projection: Arc::new(projection), ..cached.clone() }, cache.ticket());
    }
    // Writes run on blocking threads; give them a moment before the app reads.
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}
