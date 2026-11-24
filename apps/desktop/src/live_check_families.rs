// live-check phases for older settled threads and thread families:
//
//   families-seed  pairs two servers (`T3_PAIRING_URL`, `T3_PAIRING_URL_B`), and on each makes a
//            project and `T3_FAMILY_THREADS` (default 1100) threads settled at backdated times,
//            interleaved between the servers, every fifth sharing its neighbour's time; on server A
//            a thread with one turn and a fork of it, both settled long ago
//   families a new process: launch to the first sidebar rows from cache (the Settled count
//            includes what the cache knows is not loaded), then live. Opening the old thread
//            merges its family. The Settled shelf is opened and grown by "Show more" until
//            every older page is loaded: after each step the drawn settled rows equal the
//            servers' own order read page by page, the header counts every settled thread and
//            each environment's pages match the server's. Server A's socket is dropped and the
//            pages read again. Resident memory and wheel frame times before and after.
//   families-search  sidebar search by title finds a thread settled before the cut: the search
//            sends each server's cut (`olderSettledBefore`), the older rows it returns join the
//            shell and the sidebar lists the match; clearing the search drops them again.

use super::*;
use t3code_client_runtime::state::older_settled_threads::{OLDER_SETTLED_PAGE_SIZE, PagesStatus};
use t3code_contracts::rpc::methods::OrchestrationGetOlderSettledThreads;
use t3code_contracts::{
    OrchestrationV2Command, OrchestrationV2CommandThreadSettle, OrchestrationV2GetOlderSettledThreadsInput,
    OrchestrationV2OlderSettledCursor, OrchestrationV2ThreadForkSourcePoint, OrchestrationV2ThreadForkSourcePointLatestStable,
};
use t3code_web::components::sidebar_logic::{Shelf, SidebarSection};

const FAMILIES_FILE: &str = "families-state.json";
/// Seeded settle times step back from here, a minute apart.
const SETTLE_BASE_MS: i64 = 1_767_225_600_000;

#[derive(Serialize, Deserialize)]
struct FamiliesState {
    env_a: String,
    env_b: String,
    family_root: String,
    family_fork: String,
}

/// One settled thread as the servers order them.
#[derive(Clone, Debug, PartialEq)]
struct Truth {
    key: ThreadKey,
    settled_at: i64,
}

fn pair_if_needed(check: &mut Check, wanted: usize, var: &str) -> anyhow::Result<()> {
    let have = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.len());
    if have >= wanted {
        return Ok(());
    }
    let request = parse_pairing_url(&std::env::var(var).with_context(|| var.to_owned())?)?;
    let pairing = check.cx.update(|cx| ConnectionRuntime::global(cx).pair(request, None, cx));
    let label = check.wait_task(pairing)?.map_err(|e| anyhow::anyhow!("{}", e.0))?;
    check.note(format!("paired {label}"));
    Ok(())
}

fn environment_ids(check: &mut Check) -> Vec<t3code_contracts::EnvironmentId> {
    check.cx.update(|cx| {
        ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| t3code_contracts::EnvironmentId::new(e.entry.environment_id.as_str())).collect()
    })
}

fn all_connected(check: &mut Check, count: usize) -> bool {
    check.cx.update(|cx| {
        let state = ConnectionRuntime::state(cx).read(cx);
        state.environments.len() >= count && state.environments.iter().all(|e| e.status.as_ref().is_some_and(|s| s.phase == ConnectionPhase::Connected))
    })
}

fn project_at(check: &mut Check, environment_id: &t3code_contracts::EnvironmentId, root: &str) -> anyhow::Result<ProjectId> {
    let id = environment_id.clone();
    let root_text = root.to_owned();
    check.pump(move |c| c.cx.update(|cx| StateBridge::environment(&id, cx).is_some_and(|env| env.read(cx).shell.snapshot.is_some())), Duration::from_secs(30), "shell loaded")?;
    let existing = check.cx.update(|cx| {
        let shell = StateBridge::environment(environment_id, cx)?.read(cx).shell.clone();
        shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == root_text).map(|p| p.id.clone())
    });
    if let Some(id) = existing {
        return Ok(id);
    }
    let commands = commands_of(check, environment_id)?;
    let id = ProjectId::new(uuid::Uuid::new_v4().to_string());
    let mutation = ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        project_id: id.clone(),
        title: "orbit".into(),
        workspace_root: root.to_owned(),
        create_workspace_root_if_missing: Some(false),
        default_model_selection: None,
        scripts: None,
    });
    check.block(async move { commands.mutate_project(mutation).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(id)
}

fn settle_command(thread_id: &ThreadId, settled_at: i64) -> OrchestrationV2Command {
    OrchestrationV2Command::ThreadSettle(OrchestrationV2CommandThreadSettle {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        thread_id: thread_id.clone(),
        expected_latest_run_id: None,
        settled_at: Some(format_ms(settled_at)),
    })
}

fn create_input(project: &ProjectId, title: String) -> CreateThread {
    CreateThread {
        thread_id: ThreadId::new(uuid::Uuid::new_v4().to_string()),
        project_id: project.clone(),
        title,
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::ApprovalRequired,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
    }
}

pub(super) fn run_families_seed(check: &mut Check) -> anyhow::Result<()> {
    use futures::StreamExt as _;
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    check.pump(|c| all_connected(c, 2), Duration::from_secs(60), "both servers connected")?;
    let ids = environment_ids(check);
    let roots = [std::env::var("T3_PROJECT_ROOT").context("T3_PROJECT_ROOT")?, std::env::var("T3_PROJECT_ROOT_B").context("T3_PROJECT_ROOT_B")?];
    let per_server: usize = std::env::var("T3_FAMILY_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(1100);
    for (server, (environment_id, root)) in ids.iter().zip(&roots).enumerate() {
        let project = project_at(check, environment_id, root)?;
        let commands = commands_of(check, environment_id)?;
        let started = Instant::now();
        for chunk in (0..per_server).collect::<Vec<_>>().chunks(100) {
            let last = *chunk.last().expect("chunk");
            let (commands, project, chunk) = (commands.clone(), project.clone(), chunk.to_vec());
            let failures = check.block(async move {
                futures::stream::iter(chunk)
                    .map(|n| {
                        let (commands, project) = (commands.clone(), project.clone());
                        async move {
                            // Interleaved between the servers; every fifth shares the previous time.
                            let step = if n % 5 == 4 { n - 1 } else { n } as i64;
                            let settled_at = SETTLE_BASE_MS - step * 120_000 - server as i64 * 60_000;
                            let input = create_input(&project, format!("Old {} {n:04}", ["alpha", "beta"][server]));
                            let thread_id = input.thread_id.clone();
                            commands.create_thread(input).await.map_err(|e| e.to_string())?;
                            commands.dispatch(settle_command(&thread_id, settled_at)).await.map_err(|e| e.to_string())?;
                            Ok::<_, String>(())
                        }
                    })
                    .buffer_unordered(12)
                    .filter_map(|result| async move { result.err() })
                    .collect::<Vec<_>>()
                    .await
            })?;
            anyhow::ensure!(failures.is_empty(), "seeding failed: {failures:?}");
            check.note(format!("server {server}: seeded {} threads", last + 1));
        }
        check.note(format!("server {server}: {per_server} settled threads in {:.1} s", started.elapsed().as_secs_f64()));
    }

    // A thread with a turn and a fork of it, both settled long before every other thread.
    let env_a = ids[0].clone();
    let project_a = project_at(check, &env_a, &roots[0])?;
    let commands = commands_of(check, &env_a)?;
    let root_input = create_input(&project_a, "Family root".into());
    let root = root_input.thread_id.clone();
    let c = commands.clone();
    check.block(async move { c.create_thread(root_input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let c = commands.clone();
    let message = SendMessage::text(root.clone(), "One turn so the thread can fork.");
    check.block(async move { c.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let (e, r) = (env_a.clone(), root.clone());
    check.pump(
        move |c| {
            c.cx.update(|cx| {
                StateBridge::shell(cx).read(cx).presented().iter().any(|row| row.environment_id == e && row.id() == &r && row.latest_run.as_ref().is_some_and(|run| run.completed_at.is_some()))
            })
        },
        Duration::from_secs(60),
        "family root's turn completed",
    )?;
    let fork = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let (c, r, f) = (commands.clone(), root.clone(), fork.clone());
    check
        .block(async move {
            c.fork(&r, f, OrchestrationV2ThreadForkSourcePoint::LatestStable(OrchestrationV2ThreadForkSourcePointLatestStable {}), Some("Family fork".into()), false).await
        })?
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let oldest = SETTLE_BASE_MS - per_server as i64 * 120_000 - 86_400_000;
    for (thread, at) in [(&root, oldest), (&fork, oldest - 60_000)] {
        let (c, command) = (commands.clone(), settle_command(thread, at));
        check.block(async move { c.dispatch(command).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let state = FamiliesState { env_a: env_a.to_string(), env_b: ids[1].to_string(), family_root: root.to_string(), family_fork: fork.to_string() };
    std::fs::write(check.out.join(FAMILIES_FILE), serde_json::to_vec_pretty(&state)?)?;
    check.note("seeded the family root and its fork");
    Ok(())
}

/// Every settled thread a server holds, in its order: the shell's settled rows and then every
/// older page read from the cut, page by page. Returns the rows and each page's row count.
fn server_truth(check: &mut Check, environment_id: &t3code_contracts::EnvironmentId) -> anyhow::Result<(Vec<Truth>, Vec<usize>, u64)> {
    let (session, shell) = check
        .cx
        .update(|cx| {
            let env = StateBridge::environment(environment_id, cx)?;
            let env = env.read(cx);
            Some((env.store.context().supervisor.session.borrow().clone()?, env.store.shell().source()))
        })
        .context("session")?;
    let snapshot = shell.snapshot.clone().context("snapshot")?;
    let cut = snapshot.older_settled.clone().context("the shell names no older settled cut")?;
    let mut rows: Vec<Truth> = snapshot
        .threads
        .iter()
        .filter(|t| t.lineage.is_listed() && t.archived_at.is_none() && t.settled_override == Some(t3code_contracts::OrchestrationV2ThreadShellSettledOverride::Settled))
        .map(|t| Truth { key: thread_key(environment_id, &t.id), settled_at: t.settled_at.as_deref().and_then(t3code_client_runtime::state::models::parse_ms).unwrap_or(0) })
        .collect();
    let env = environment_id.clone();
    let (pages, counted) = check.block(async move {
        let mut pages: Vec<Vec<Truth>> = Vec::new();
        let mut before = OrchestrationV2OlderSettledCursor { settled_at: cut.before.clone(), thread_id: None };
        let counts = session
            .client
            .call::<OrchestrationGetOlderSettledThreads>(OrchestrationV2GetOlderSettledThreadsInput { before: before.clone(), project_ids: None, limit: 0 })
            .await
            .map_err(|e| e.to_string())?;
        loop {
            let page = session
                .client
                .call::<OrchestrationGetOlderSettledThreads>(OrchestrationV2GetOlderSettledThreadsInput { before: before.clone(), project_ids: None, limit: OLDER_SETTLED_PAGE_SIZE })
                .await
                .map_err(|e| e.to_string())?;
            let full = page.threads.len() as u64 == OLDER_SETTLED_PAGE_SIZE;
            let last = page.threads.last().map(|t| OrchestrationV2OlderSettledCursor { settled_at: t.settled_at.clone().unwrap_or_default(), thread_id: Some(t.id.clone()) });
            pages.push(
                page.threads
                    .iter()
                    .map(|t| Truth { key: thread_key(&env, &t.id), settled_at: t.settled_at.as_deref().and_then(t3code_client_runtime::state::models::parse_ms).unwrap_or(0) })
                    .collect(),
            );
            match last {
                Some(last) if full => before = last,
                _ => break,
            }
        }
        Ok::<_, String>((pages, counts.counts.iter().map(|c| c.count).sum::<u64>()))
    })?
    .map_err(|e| anyhow::anyhow!(e))?;
    let sizes = pages.iter().map(Vec::len).collect();
    // A shell row below the cut (a visited old thread a delta brought in) is also on a page.
    let held: std::collections::HashSet<ThreadKey> = rows.iter().map(|t| t.key.clone()).collect();
    rows.extend(pages.into_iter().flatten().filter(|t| !held.contains(&t.key)));
    Ok((rows, sizes, counted))
}

fn order(rows: &mut [Truth]) {
    rows.sort_by(|l, r| r.settled_at.cmp(&l.settled_at).then_with(|| l.key.thread_id.cmp(&r.key.thread_id)));
}

/// The settled rows the sidebar draws, in order, and the Settled header's count.
fn drawn_settled(check: &mut Check) -> (Vec<ThreadKey>, Option<usize>, Option<usize>) {
    check.read(|root, cx| {
        let list = root.sidebar().read(cx).list();
        let rows = list
            .entries
            .iter()
            .filter_map(|e| match e {
                SidebarListEntry::Thread { thread, section: SidebarSection::Settled, .. } => Some(thread.key.clone()),
                _ => None,
            })
            .collect();
        let header = list.entries.iter().find_map(|e| match e {
            SidebarListEntry::ShelfHeader { shelf: Shelf::Settled, count, .. } => Some(*count),
            _ => None,
        });
        let more = list.entries.iter().find_map(|e| match e {
            SidebarListEntry::ShowMore { count } => Some(*count),
            _ => None,
        });
        (rows, header, more)
    })
}

/// Clicks the first list entry matching `which`, scrolling the list until it is in view.
fn click_entry(check: &mut Check, which: impl Fn(&SidebarListEntry) -> bool + Copy, label: &str) -> anyhow::Result<Duration> {
    let mut seen = std::collections::VecDeque::new();
    for _ in 0..400 {
        let at = check.read(|root, cx| {
            let sidebar = root.sidebar().read(cx);
            let ix = sidebar.list().entries.iter().position(which)?;
            // The top edge: the last entry can sit partly under the footer when scrolled to the end.
            sidebar.entry_bounds(ix).map(|b| point(b.center().x, b.top() + px(6.)))
        });
        seen.push_back(at.map(|at| f32::from(at.y)));
        if seen.len() > 6 {
            seen.pop_front();
        }
        if let Some(at) = at.filter(|at| at.y > px(60.) && at.y < px(805.)) {
            check.cx.update_window(check.window, |_, w, cx| {
                w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
            })?;
            check.frame();
            let started = Instant::now();
            check.cx.update_window(check.window, |_, w, cx| {
                w.dispatch_event(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }.to_platform_input(), cx);
                w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
                w.draw(cx).clear(cx);
            })?;
            let elapsed = started.elapsed();
            check.frame();
            return Ok(elapsed);
        }
        // The list is virtualized: jump to the end (Show more) or the top (headers) and retry.
        // Toward the entry when it has bounds, else to the end for Show more.
        let down = match at {
            Some(at) => at.y >= px(805.),
            None => label == "show more",
        };
        wheel_event(check, point(px(150.), px(400.)), ScrollDelta::Pixels(point(px(0.), px(if down { -400. } else { 400. }))), TouchPhase::Moved);
        check.frame();
    }
    bail!("no {label} entry in view (last top edges {seen:?})")
}

/// Wheel frames over the sidebar: 120 trackpad steps down and back. Returns frame times.
fn wheel_frames(check: &mut Check) -> Vec<f64> {
    let at = point(px(150.), px(400.));
    wheel_event(check, at, ScrollDelta::Pixels(point(px(0.), px(1_000_000.))), TouchPhase::Moved);
    check.frame();
    let mut frames = Vec::new();
    for step in 0..120 {
        let dy = if step < 60 { -90. } else { 90. };
        wheel_event(check, at, ScrollDelta::Pixels(point(px(0.), px(dy))), TouchPhase::Moved);
        check.cx.advance_clock(Duration::from_millis(16));
        frames.push(ms(check.frame()));
    }
    frames
}

fn frame_summary(frames: &[f64]) -> String {
    let mut sorted = frames.to_vec();
    let p50 = percentile(&mut sorted, 0.5);
    let p99 = percentile(&mut sorted, 0.99);
    let worst = sorted.last().copied().unwrap_or(0.);
    let over = frames.iter().filter(|f| **f > 16.).count();
    format!("{} frames, p50 {p50:.2} ms, p99 {p99:.2} ms, worst {worst:.2} ms, {over} over 16 ms", frames.len())
}

/// The environments' page state: (loaded pages, rows, status) per environment.
fn page_state(check: &mut Check, ids: &[t3code_contracts::EnvironmentId]) -> Vec<(usize, usize, PagesStatus, u64)> {
    check.cx.update(|cx| {
        ids.iter()
            .map(|id| {
                StateBridge::environment(id, cx)
                    .map(|env| {
                        let env = env.read(cx);
                        (env.older.pages.loaded, env.older.pages.rows, env.older.pages.status, env.shell.generation)
                    })
                    .unwrap_or((0, 0, PagesStatus::Ready, 0))
            })
            .collect()
    })
}

fn shelf_idle(check: &mut Check) -> bool {
    check.cx.update(|cx| {
        let shell = StateBridge::shell(cx);
        let shell = shell.read(cx);
        !shell.snapshot().older_settled.loading && shell.environments(cx).iter().all(|env| env.read(cx).older.pages.status != PagesStatus::Pending)
    })
}

/// Pumps until the shelf has no page in flight and the drawn rows stop changing.
fn settle_shelf(check: &mut Check) -> anyhow::Result<()> {
    let mut last = None;
    let mut stable = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        check.cx.advance_clock(Duration::from_millis(16));
        check.frame();
        let ids = environment_ids(check);
        let now = (drawn_settled(check), page_state(check, &ids));
        if shelf_idle(check) && last.as_ref() == Some(&now) {
            stable += 1;
            if stable >= 3 {
                return Ok(());
            }
        } else {
            stable = 0;
        }
        last = Some(now);
        anyhow::ensure!(Instant::now() < deadline, "the shelf did not settle");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Compares what the Settled shelf draws with the servers' rows: the drawn rows are the
/// servers' merged order cut at the shelf's frontier and at the visible count.
fn compare_shelf(check: &mut Check, truth: &[Truth], total: usize, step: &str) -> (usize, bool) {
    let (drawn, header, more) = drawn_settled(check);
    let frontier = check.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().older_settled.frontier.clone());
    let listed: Vec<&Truth> = truth
        .iter()
        .filter(|t| {
            frontier.as_ref().is_none_or(|f| {
                !(t.settled_at < f.settled_at.0 || f.thread_id.as_ref().is_some_and(|id| t.settled_at == f.settled_at.0 && t.key.thread_id.0.as_ref() > id.as_ref()))
            })
        })
        .collect();
    let expected: Vec<ThreadKey> = listed.iter().take(drawn.len()).map(|t| t.key.clone()).collect();
    // Every drawn row is the servers' next row, the header counts every settled thread, and
    // Show more stays while rows are hidden.
    let ok = drawn == expected && header == Some(total) && (more.is_some() == (drawn.len() < total));
    let first_diff = drawn.iter().zip(&expected).position(|(a, b)| a != b);
    check.check(
        &format!("{step}: the shelf draws the servers' rows in their order"),
        ok,
        format!("drawn {}, listed by the frontier {}, header {header:?} of {total}, show more {more:?}, first difference {first_diff:?}", drawn.len(), listed.len()),
    );
    (drawn.len(), ok)
}

pub(super) fn run_families(check: &mut Check) -> anyhow::Result<()> {
    let state: FamiliesState = serde_json::from_slice(&std::fs::read(check.out.join(FAMILIES_FILE))?)?;
    // A fresh profile pairs here: the seeding profile's shell resumed from the cut it took before
    // any thread existed, as the web's cache resume does.
    let fresh = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.is_empty());
    if fresh {
        pair_if_needed(check, 1, "T3_PAIRING_URL")?;
        pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    }
    let launch = Instant::now();
    // Launch: rows from the cache, before the network answers.
    // Every seeded thread is settled and the shelf starts closed, so the first rows are its header.
    check.pump(|c| drawn_settled(c).1.is_some_and(|count| count > 0), Duration::from_secs(30), "first sidebar rows")?;
    let first_rows = launch.elapsed();
    let cached = check.cx.update(|cx| StateBridge::shell(cx).read(cx).environments(cx).iter().all(|env| env.read(cx).shell.generation == 0));
    let (_, header_cached, _) = drawn_settled(check);
    let unloaded_cached = check.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().older_settled.unloaded_count(None));
    check.note(format!(
        "launch to first sidebar rows {:.0} ms (process start {:.0} ms); from cache {cached}; Settled header {header_cached:?} with {unloaded_cached} not loaded",
        ms(first_rows),
        ms(check.started.elapsed())
    ));
    check.pump(|c| all_connected(c, 2), Duration::from_secs(60), "both servers connected")?;
    let ids = vec![t3code_contracts::EnvironmentId::new(state.env_a.as_str()), t3code_contracts::EnvironmentId::new(state.env_b.as_str())];
    let id_list = ids.clone();
    check.pump(move |c| page_state(c, &id_list).iter().all(|(_, _, _, generation)| *generation > 0), Duration::from_secs(60), "authoritative snapshots")?;
    let live_rows = launch.elapsed();
    check.note(format!("launch to live rows {:.0} ms", ms(live_rows)));
    check.cx.update(|cx| {
        for env in StateBridge::shell(cx).read(cx).environments(cx) {
            let env = env.read(cx);
            let older = env.older.older.as_ref();
            println!(
                "  {}: shell rows {}, cut {:?}, older {:?}",
                env.environment_id(),
                env.shell.snapshot.as_ref().map_or(0, |s| s.threads.len()),
                older.map(|o| o.before.clone()),
                older.map(|o| o.counts.values().sum::<u64>())
            );
        }
    });

    let mut truth = Vec::new();
    let mut sizes = Vec::new();
    let mut total = 0usize;
    for id in &ids {
        let (rows, pages, counted) = server_truth(check, id)?;
        let held = rows.len().saturating_sub(pages.iter().sum::<usize>());
        check.note(format!("{id}: server holds {held} settled shell rows and {} older in {} pages (count {counted})", pages.iter().sum::<usize>(), pages.len()));
        check.check("the server's older count equals its paged rows", counted as usize == pages.iter().sum::<usize>(), format!("{counted} counted"));
        total += rows.len();
        truth.extend(rows);
        sizes.push(pages);
    }
    order(&mut truth);
    if cached {
        check.check("the cached Settled count already includes what is not loaded", header_cached == Some(total), format!("{header_cached:?} of {total}"));
    }
    let rss_start = rss_mib();
    let presented_start = check.cx.update(|cx| StateBridge::shell(cx).read(cx).presented().len());

    // The family: an older settled thread opened by its route merges its family.
    let env_a = &ids[0];
    let root = thread_key(env_a, &ThreadId::new(state.family_root.as_str()));
    let fork = thread_key(env_a, &ThreadId::new(state.family_fork.as_str()));
    let in_shell = |c: &mut Check, key: &ThreadKey| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(key).is_some());
    // Only a profile whose shell still leaves the root out can show the mount: the web mounts a
    // family only for a thread the shell does not have, and a visit makes the root a server row.
    if in_shell(check, &root) {
        check.note("the root is a server row since an earlier visit; the family section needs a fresh seed");
    } else {
    let absent = !in_shell(check, &root) && !in_shell(check, &fork);
    check.check("the old family is not in the shell", absent, "root and fork absent");
    check.go(&root);
    let (r, f) = (root.clone(), fork.clone());
    check.pump(move |c| in_shell(c, &r) && in_shell(c, &f), Duration::from_secs(30), "opening the old root merges its family (root and fork)")?;
    check.check("opening an old thread merges its family", true, "root and fork rows present");
    check.shot("fam-00-family-open.png")?;
    let (r, f) = (truth.iter().find(|t| t.key == root).cloned(), truth.iter().find(|t| t.key == fork).cloned());
    check.check("the family rows are the server's oldest settled rows", r.is_some() && f.is_some(), format!("root {:?}, fork {:?}", r.map(|t| t.settled_at), f.map(|t| t.settled_at)));
    let other = truth.iter().find(|t| t.key.environment_id.0.as_ref() == state.env_b.as_str()).map(|t| t.key.clone()).context("a thread on B")?;
    check.go(&other);
    // Opening the root sent a visit; the server's update delta brings the root into the shell
    // itself, so only the fork shows the release.
    let f = fork.clone();
    check.pump(move |c| !in_shell(c, &f), Duration::from_secs(10), "leaving the old thread releases its family (the fork leaves)")?;
    let root_in_source = check.cx.update(|cx| {
        StateBridge::environment(env_a, cx).is_some_and(|env| env.read(cx).store.shell().source().thread(&ThreadId::new(state.family_root.as_str())).is_some())
    });
    let root_shown = in_shell(check, &root);
    check.check("the visited root stays only as a server row", root_shown == root_in_source, format!("root in the server shell {root_in_source}"));

    }

    // Before any page: frames and memory with the shelf open at its first count.
    let frames_before = wheel_frames(check);
    check.note(format!("wheel before pages: {}", frame_summary(&frames_before)));
    check.note(format!("resident memory before pages {rss_start:.1} MiB, {presented_start} presented rows"));

    // `T3_FAMILY_CYCLES` repeats open, load every page and close, to tell retained allocator
    // pages from growth.
    let cycles: usize = std::env::var("T3_FAMILY_CYCLES").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut rss_cycles: Vec<(f64, f64)> = Vec::new();
    let (mut rss_all, mut rss_closed, mut presented_all, mut frames_after, mut step, mut slow_clicks) = (0., 0., 0, Vec::new(), 0usize, Vec::new());
    for cycle in 0..cycles {
    click_entry(check, |e| matches!(e, SidebarListEntry::ShelfHeader { shelf: Shelf::Settled, expanded: false, .. }), "settled header")?;
    settle_shelf(check)?;
    check.shot("fam-01-settled-open.png")?;
    step = 0;
    let (mut drawn, _) = compare_shelf(check, &truth, total, "open");
    slow_clicks.clear();
    while drawn < truth.len() {
        step += 1;
        anyhow::ensure!(step < 2000, "show more never reached the end");
        let elapsed = click_entry(check, |e| matches!(e, SidebarListEntry::ShowMore { .. }), "show more")?;
        slow_clicks.push(ms(elapsed));
        settle_shelf(check)?;
        let (next, ok) = compare_shelf(check, &truth, total, &format!("show more {step}"));
        if !ok {
            check.shot(&format!("fam-mismatch-{step}.png")).ok();
            break;
        }
        anyhow::ensure!(next > drawn, "show more {step} drew no new rows ({next})");
        drawn = next;
        if step == 6 && cycle == 0 {
            check.shot("fam-02-pages.png")?;
            // Each environment's pages are the server's pages, in order.
            let pages = page_state(check, &ids);
            for ((loaded, rows, status, _), sizes) in pages.iter().zip(&sizes) {
                let expected: usize = sizes.iter().take(*loaded).sum();
                check.check("an environment's loaded pages are the server's first pages", *rows == expected && *status == PagesStatus::Ready, format!("{loaded} pages, {rows} rows, server {expected}"));
            }
            // A dropped socket: the shell resumes from its cursor; a resume that caught up keeps
            // its generation, so the pages stay; a new snapshot would read them again.
            let before = page_state(check, &ids);
            let session_of = |c: &mut Check, id: &t3code_contracts::EnvironmentId| {
                c.cx.update(|cx| StateBridge::environment(id, cx).and_then(|env| env.read(cx).store.context().supervisor.session.borrow().clone()))
            };
            let session = session_of(check, &ids[0]).context("session A")?;
            session.client.close();
            let id = ids[0].clone();
            check.pump(
                move |c| session_of(c, &id).is_some_and(|next| !Arc::ptr_eq(&next, &session)) && all_connected(c, 2),
                Duration::from_secs(30),
                "server A reconnected",
            )?;
            settle_shelf(check)?;
            let generations: Vec<u64> = page_state(check, &ids).iter().map(|p| p.3).collect();
            check.note(format!("generations before {:?}, after {generations:?}", before.iter().map(|p| p.3).collect::<Vec<_>>()));
            let after = page_state(check, &ids);
            check.check(
                "after a reconnect each environment holds the same pages",
                before.iter().zip(&after).all(|(b, a)| b.0 == a.0 && b.1 == a.1),
                format!("before {:?}, after {:?}", before.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>(), after.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>()),
            );
            let (again, ok) = compare_shelf(check, &truth, total, "after reconnect");
            check.check("the shelf draws the same rows after a reconnect", ok && again == drawn, format!("{again} rows, was {drawn}"));
        }
    }
    let pages = page_state(check, &ids);
    for ((loaded, rows, _, _), sizes) in pages.iter().zip(&sizes) {
        check.check("every older page is loaded", *loaded == sizes.len() && *rows == sizes.iter().sum::<usize>(), format!("{loaded} of {} pages, {rows} rows", sizes.len()));
    }
    let (_, _, more) = drawn_settled(check);
    check.check("Show more is gone once every row shows", more.is_none(), format!("{more:?}"));
    let mut clicks = slow_clicks.clone();
    if !clicks.is_empty() {
        check.note(format!("{step} show more clicks: p50 {:.2} ms, worst {:.2} ms (event and draw)", percentile(&mut clicks, 0.5), clicks.last().copied().unwrap_or(0.)));
    }
    check.shot("fam-03-all-pages.png")?;

    rss_all = rss_mib();
    presented_all = check.cx.update(|cx| StateBridge::shell(cx).read(cx).presented().len());
    frames_after = wheel_frames(check);
    check.note(format!("wheel after every page: {}", frame_summary(&frames_after)));
    check.note(format!("resident memory after every page {rss_all:.1} MiB (+{:.1}), {presented_all} presented rows", rss_all - rss_start));

    // Closing the shelf drops the page rows; opening it again reads them again.
    click_entry(check, |e| matches!(e, SidebarListEntry::ShelfHeader { shelf: Shelf::Settled, expanded: true, .. }), "settled header")?;
    settle_shelf(check)?;
    let closed = page_state(check, &ids);
    let presented_closed = check.cx.update(|cx| StateBridge::shell(cx).read(cx).presented().len());
    check.check("a closed shelf keeps no pages", closed.iter().all(|p| p.0 == 0), format!("{presented_closed} presented rows, was {presented_all}"));
    check.wait(Duration::from_secs(2));
    rss_closed = rss_mib();
    check.note(format!("cycle {cycle}: resident memory with the shelf closed {rss_closed:.1} MiB"));
    rss_cycles.push((rss_all, rss_closed));
    }
    let record = serde_json::json!({
        "launch_to_first_rows_ms": ms(first_rows),
        "launch_to_live_rows_ms": ms(live_rows),
        "from_cache": cached,
        "settled_total": total,
        "server_pages": sizes,
        "rss_before_mib": rss_start,
        "rss_all_pages_mib": rss_all,
        "rss_shelf_closed_mib": rss_closed,
        "presented_before": presented_start,
        "presented_all": presented_all,
        "wheel_before_ms": frames_before,
        "wheel_after_ms": frames_after,
        "show_more_clicks": step,
        "show_more_click_ms": slow_clicks,
        "rss_cycles_all_closed_mib": rss_cycles,
    });
    std::fs::write(check.out.join("families-record.json"), serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}

/// Whether the sidebar lists a thread titled `title`, and whether the shell holds it.
fn listed_and_held(check: &mut Check, title: &str) -> (bool, bool) {
    let held = check.cx.update(|cx| StateBridge::shell(cx).read(cx).presented().iter().any(|row| row.title() == title));
    let listed = check.read(|root, cx| {
        root.sidebar().read(cx).list().entries.iter().any(|entry| matches!(entry, SidebarListEntry::Thread { thread, .. } if thread.title.as_ref() == title))
    });
    (listed, held)
}

fn set_sidebar_search(check: &mut Check, text: &str) {
    let sidebar = check.read(|root, _| root.sidebar().clone());
    let text = text.to_owned();
    check.cx.update_window(check.window, |_, window, cx| sidebar.update(cx, |sidebar, cx| sidebar.set_search(&text, window, cx))).ok();
}

pub(super) fn run_families_search(check: &mut Check) -> anyhow::Result<()> {
    let per_server: usize = std::env::var("T3_FAMILY_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(1100);
    // The last seeded thread on server A settled first, so it is the oldest and below the cut.
    let title = format!("Old alpha {:04}", per_server - 1);
    if check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.is_empty()) {
        pair_if_needed(check, 1, "T3_PAIRING_URL")?;
        pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    }
    check.pump(|c| all_connected(c, 2), Duration::from_secs(60), "both servers connected")?;
    check.pump(
        |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).environments(cx).iter().all(|env| env.read(cx).shell.generation > 0)),
        Duration::from_secs(60),
        "authoritative snapshots",
    )?;
    let (_, held) = listed_and_held(check, &title);
    check.check("the oldest settled thread is left out of the shell", !held, &title);
    set_sidebar_search(check, &title);
    let t = title.clone();
    check.pump(move |c| listed_and_held(c, &t).0, Duration::from_secs(20), "the search lists the older settled thread")?;
    let (listed, held) = listed_and_held(check, &title);
    check.check("search lists an older settled thread", listed && held, format!("listed {listed}, in shell {held}"));
    check.shot("fam-search-00-found.png")?;
    set_sidebar_search(check, "");
    let t = title.clone();
    check.pump(move |c| !listed_and_held(c, &t).1, Duration::from_secs(10), "clearing the search drops the older rows")?;
    check.check("clearing the search drops the older rows", true, "");
    Ok(())
}
