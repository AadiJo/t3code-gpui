//! Fixture sidebars for scenes, screenshots and the frame-time benchmark.

use std::sync::Arc;

use gpui_kit::SharedString;

use super::model::*;

/// A fixed clock: 2026-11-23 15:00 UTC.
pub const FIXTURE_NOW: TimestampMs = TimestampMs(1_795_446_000_000);

const MIN: i64 = TimestampMs::MINUTE;
const HOUR: i64 = TimestampMs::HOUR;
const DAY: i64 = TimestampMs::DAY;

fn env() -> EnvironmentId {
    EnvironmentId("env-local".into())
}

fn project(id: &str, title: &str, age_days: i64) -> SidebarProject {
    SidebarProject {
        project_ref: ProjectRef {
            environment_id: env(),
            project_id: ProjectId(id.to_string().into()),
        },
        title: title.to_string().into(),
        workspace_root: format!("/work/{id}").into(),
        repository_key: Some(format!("github.com/acme/{id}").into()),
        repository_relative_path: None,
        repository_display_name: None,
        repository_name: None,
        has_custom_title: false,
        created_at: FIXTURE_NOW.minus(age_days * DAY),
        updated_at: FIXTURE_NOW.minus(DAY),
        pinned: false,
        archived: false,
    }
}

/// A quiet, read, idle thread created `age` ms ago. Tweak fields from here.
fn thread(id: &str, project: &str, title: &str, age: i64) -> SidebarThread {
    let created = FIXTURE_NOW.minus(age);
    SidebarThread {
        key: ThreadKey {
            environment_id: env(),
            thread_id: ThreadId(id.to_string().into()),
        },
        project_id: ProjectId(project.to_string().into()),
        title: title.to_string().into(),
        branch: None,
        in_worktree: false,
        created_at: created,
        updated_at: created,
        latest_user_message_at: Some(created),
        latest_user_authored_message_at: Some(created),
        runtime: Some(ThreadRuntime {
            status: RuntimeStatus::Run(RunStatus::Completed),
            last_error_class: None,
        }),
        latest_run: Some(LatestRun {
            status: RunStatus::Completed,
            requested_at: Some(created),
            started_at: Some(created),
            completed_at: Some(created.minus(-2 * MIN)),
        }),
        blocked_on_thread_wait: false,
        has_pending_approvals: false,
        has_pending_user_input: false,
        pending_subagent_request: None,
        pending_guest_message_count: 0,
        goal_active: false,
        last_visited_at: Some(FIXTURE_NOW),
        has_unread_completion_notification: false,
        settled_override: None,
        settled_at: None,
        unsettled_at: None,
        snoozed_until: None,
        woke_at: None,
        pinned_at: None,
        pin_order_key: None,
        active_order_key: None,
        archived: false,
        has_unsent_draft: false,
        diff: None,
        pull_request: None,
        issue_key: None,
        running_terminal_count: 0,
        remote_machine: false,
        provider: None,
    }
}

fn running(mut t: SidebarThread, status: RunStatus, started_ago: i64) -> SidebarThread {
    t.runtime = Some(ThreadRuntime {
        status: RuntimeStatus::Run(status),
        last_error_class: None,
    });
    let started = FIXTURE_NOW.minus(started_ago);
    t.latest_run = Some(LatestRun {
        status,
        requested_at: Some(started),
        started_at: Some(started),
        completed_at: None,
    });
    t
}

fn unread(mut t: SidebarThread) -> SidebarThread {
    t.last_visited_at = t.latest_run.and_then(|r| r.completed_at).map(|c| c.minus(MIN));
    t
}

/// Sidebar fixture that puts every row state on screen at once.
pub fn showcase_snapshot() -> SidebarSnapshot {
    let projects = vec![
        project("t3code", "t3code", 90),
        project("relay", "relay", 60),
        project("marketing", "marketing-site", 30),
        project("infra", "infra", 20),
    ];
    let mut threads = Vec::new();

    let mut pinned = unread(thread("p1", "t3code", "Release checklist for v0.1.12", 3 * HOUR));
    pinned.pinned_at = Some(FIXTURE_NOW.minus(DAY));
    pinned.pin_order_key = Some("a0".into());
    pinned.branch = Some("release/v0.1.12".into());
    pinned.pull_request = Some(PullRequestBadge {
        number: 41612,
        state: PullRequestState::Open,
    });
    threads.push(pinned);

    let mut working = running(
        thread("a1", "t3code", "Port sidebar virtualization to GPUI", 20 * MIN),
        RunStatus::Running,
        83 * 1000,
    );
    working.branch = Some("feature/gpui-sidebar-virtualized-list-with-fixed-heights".into());
    working.in_worktree = true;
    working.running_terminal_count = 1;
    working.diff = Some(DiffStat {
        insertions: 412,
        deletions: 37,
    });
    threads.push(working);

    let mut approval = thread("a2", "relay", "Rotate relay TLS certificates", 40 * MIN);
    approval.has_pending_approvals = true;
    approval.runtime = Some(ThreadRuntime {
        status: RuntimeStatus::Run(RunStatus::Waiting),
        last_error_class: None,
    });
    approval.branch = Some("ops/rotate-certs".into());
    threads.push(approval);

    let mut input = thread("a3", "marketing", "Draft launch copy for the changelog", 55 * MIN);
    input.has_pending_user_input = true;
    threads.push(input);

    let mut request = thread("a4", "t3code", "Guest asks about pairing codes", 70 * MIN);
    request.pending_guest_message_count = 2;
    threads.push(request);

    let mut failed = thread("a5", "infra", "Migrate CI runners to arm64", 2 * HOUR);
    failed.runtime = Some(ThreadRuntime {
        status: RuntimeStatus::Run(RunStatus::Failed),
        last_error_class: Some(ErrorClass::Other),
    });
    failed.latest_run = Some(LatestRun {
        status: RunStatus::Failed,
        requested_at: Some(FIXTURE_NOW.minus(2 * HOUR)),
        started_at: Some(FIXTURE_NOW.minus(2 * HOUR)),
        completed_at: Some(FIXTURE_NOW.minus(90 * MIN)),
    });
    failed.last_visited_at = Some(FIXTURE_NOW.minus(3 * HOUR));
    failed.issue_key = Some("INF-212".into());
    threads.push(failed);

    let mut limited = thread("a6", "relay", "Benchmark tunnel throughput", 3 * HOUR);
    limited.runtime = Some(ThreadRuntime {
        status: RuntimeStatus::Run(RunStatus::Failed),
        last_error_class: Some(ErrorClass::UsageLimit),
    });
    threads.push(limited);

    let mut woke = thread("a7", "t3code", "Check back on flaky e2e test", 6 * HOUR);
    woke.woke_at = Some(FIXTURE_NOW.minus(10 * MIN));
    woke.last_visited_at = Some(FIXTURE_NOW.minus(5 * HOUR));
    threads.push(woke);

    let done = unread(thread("a8", "marketing", "Fix hero image layout shift", 4 * HOUR));
    threads.push(done);

    let mut draft = thread("a9", "t3code", "Add keyboard shortcut for settle", 5 * HOUR);
    draft.has_unsent_draft = true;
    threads.push(draft);

    let mut waiting = thread("a10", "infra", "Wait for subagent results", 25 * MIN);
    waiting.runtime = Some(ThreadRuntime {
        status: RuntimeStatus::Idle,
        last_error_class: None,
    });
    threads.push(waiting);

    let mut read_idle = thread("a11", "relay", "Explain the relay handshake", 8 * HOUR);
    read_idle.branch = Some("main".into());
    threads.push(read_idle);

    let mut preparing = running(
        thread("w1", "infra", "Provision staging database", 3 * MIN),
        RunStatus::Preparing,
        40 * 1000,
    );
    preparing.in_worktree = true;
    threads.push(preparing);

    let mut goal = running(
        thread("w2", "t3code", "Get the test suite green", 15 * MIN),
        RunStatus::Running,
        14 * MIN,
    );
    goal.goal_active = true;
    threads.push(goal);

    for (ix, title) in ["Investigate memory growth", "Follow up on review comments"]
        .into_iter()
        .enumerate()
    {
        let mut snoozed = thread(&format!("s{ix}"), "t3code", title, (ix as i64 + 2) * DAY);
        snoozed.snoozed_until = Some(FIXTURE_NOW.minus(-((ix as i64 + 1) * 5 * HOUR)));
        threads.push(snoozed);
    }

    for (ix, title) in [
        "Update Graphite theme tokens",
        "Split connection runtime into crates",
        "Audit websocket payload sizes",
        "Remove unused icons",
    ]
    .into_iter()
    .enumerate()
    {
        let mut settled = thread(&format!("d{ix}"), "relay", title, (ix as i64 + 1) * DAY);
        settled.settled_override = Some(SettledOverride::Settled);
        settled.settled_at = Some(FIXTURE_NOW.minus((ix as i64 + 1) * 3 * HOUR));
        threads.push(settled);
    }

    let notifications = vec![
        notification("n1", NotificationKind::Approval, "a2", "Rotate relay TLS certificates", "relay", true, false),
        notification("n2", NotificationKind::Input, "a3", "Draft launch copy for the changelog", "marketing-site", true, false),
        notification("n3", NotificationKind::Failed, "a5", "Migrate CI runners to arm64", "infra", true, false),
        notification("n4", NotificationKind::Completed, "a8", "Fix hero image layout shift", "marketing-site", false, true),
    ];

    SidebarSnapshot {
        projects,
        threads: threads.into_iter().map(Arc::new).collect(),
        notifications,
        settings: SidebarSettings::default(),
        active_thread: Some(ThreadKey {
            environment_id: env(),
            thread_id: ThreadId("a1".into()),
        }),
        environment_pill: None,
        auto_settle_disabled: Default::default(),
        hover_details: Default::default(),
        drafts: Vec::new(),
        active_draft: None,
        now: FIXTURE_NOW,
    }
}

fn notification(
    id: &str,
    kind: NotificationKind,
    thread: &str,
    title: &str,
    project: &str,
    unread: bool,
    resolved: bool,
) -> SidebarNotification {
    SidebarNotification {
        id: id.to_string().into(),
        kind,
        thread: ThreadKey {
            environment_id: env(),
            thread_id: ThreadId(thread.to_string().into()),
        },
        title: title.to_string().into(),
        project_label: project.to_string().into(),
        environment_label: "Local".into(),
        resolved,
        unread,
        at: FIXTURE_NOW.minus(30 * MIN),
    }
}

/// The showcase with project folders ("boring sidebar") on.
pub fn folders_snapshot() -> SidebarSnapshot {
    let mut snapshot = showcase_snapshot();
    snapshot.settings.project_folders = true;
    snapshot
}

/// No projects at all.
pub fn empty_snapshot() -> SidebarSnapshot {
    SidebarSnapshot {
        now: FIXTURE_NOW,
        ..Default::default()
    }
}

/// `thread_count` threads spread over `project_count` projects with a mix of
/// states, for the virtualization benchmark.
pub fn large_snapshot(thread_count: usize, project_count: usize) -> SidebarSnapshot {
    let names: Vec<String> = (0..project_count).map(|i| format!("project-{i:02}")).collect();
    let projects = names.iter().enumerate().map(|(i, n)| project(n, n, i as i64 + 1)).collect();
    let mut seed: u64 = 0x5eed;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as u32
    };
    let words = [
        "Fix", "Refactor", "Investigate", "Add", "Remove", "Document", "Speed up", "Port",
    ];
    let topics = [
        "sidebar scroll", "relay auth", "diff viewer", "composer paste", "terminal resize",
        "theme tokens", "pairing flow", "settings sync",
    ];
    let threads = (0..thread_count)
        .map(|i| {
            let r = next();
            let project = &names[(r as usize) % project_count];
            let title = format!(
                "{} {} #{i}",
                words[(r >> 4) as usize % words.len()],
                topics[(r >> 8) as usize % topics.len()]
            );
            let mut t = thread(&format!("t{i}"), project, &title, (i as i64 + 1) * 7 * MIN);
            match r % 23 {
                0 => t = running(t, RunStatus::Running, 3 * MIN),
                1 => t.has_pending_approvals = true,
                2 => t = unread(t),
                3 => t.settled_override = Some(SettledOverride::Settled),
                4 => t.snoozed_until = Some(FIXTURE_NOW.minus(-DAY)),
                5 => t.branch = Some(SharedString::from(format!("feature/topic-{i}"))),
                6 => t.pinned_at = (i < 400).then_some(FIXTURE_NOW),
                _ => {}
            }
            Arc::new(t)
        })
        .collect();
    SidebarSnapshot {
        projects,
        threads,
        notifications: Vec::new(),
        settings: SidebarSettings {
            settled_page_size: 50,
            ..SidebarSettings::default()
        },
        active_thread: None,
        environment_pill: None,
        now: FIXTURE_NOW,
        auto_settle_disabled: Default::default(),
        hover_details: Default::default(),
        drafts: Vec::new(),
        active_draft: None,
    }
}

/// The sidebar the visual-parity reference was captured with: four projects,
/// seven threads on `main`, all on the Replay agent ("RR" badge), the open
/// thread "Sync engine audit" at the top and one waiting on input.
pub fn reference_snapshot() -> SidebarSnapshot {
    let projects = vec![
        project("t3code", "t3code", 4),
        project("relay", "relay", 4),
        project("marketing", "marketing-site", 4),
        project("infra", "infra", 4),
    ];
    let replay = SidebarProvider {
        mark: SidebarProviderMark::AcpRegistry,
        initials: "RR".into(),
        show_badge: true,
    };
    // Oldest first, as the reference server seeded them.
    let rows = [
        ("r-ci", "infra", "Migrate CI runners to arm64"),
        ("r-handshake", "relay", "Explain the relay handshake"),
        ("r-hero", "marketing", "Fix hero image layout shift"),
        ("r-tls", "relay", "Rotate relay TLS certificates"),
        ("r-launch", "marketing", "Draft launch copy for the changelog"),
        ("r-sidebar", "t3code", "Port sidebar virtualization to GPUI"),
        ("r-sync", "t3code", "Sync engine audit"),
    ];
    let threads = rows
        .iter()
        .enumerate()
        .map(|(ix, (id, project, title))| {
            // Runs finish 2 minutes after they start, 10s apart, newest last.
            let finished_ago = (rows.len() - 1 - ix) as i64 * 10_000;
            let mut t = thread(id, project, title, 2 * MIN + finished_ago);
            t.branch = Some("main".into());
            t.provider = Some(replay.clone());
            match *id {
                "r-sync" => t.last_visited_at = Some(FIXTURE_NOW),
                "r-launch" => {
                    t = running(t, RunStatus::Waiting, finished_ago);
                    t.has_pending_user_input = true;
                }
                _ => t = unread(t),
            }
            Arc::new(t)
        })
        .collect();
    let notifications = vec![
        notification("rn1", NotificationKind::Approval, "r-tls", "Rotate relay TLS certificates", "relay", true, false),
        notification("rn2", NotificationKind::Input, "r-launch", "Draft launch copy for the changelog", "marketing-site", true, false),
    ];
    SidebarSnapshot {
        projects,
        threads,
        notifications,
        settings: SidebarSettings::default(),
        active_thread: Some(ThreadKey { environment_id: env(), thread_id: ThreadId("r-sync".into()) }),
        environment_pill: None,
        now: FIXTURE_NOW,
        auto_settle_disabled: Default::default(),
        hover_details: reference_hover_details(),
        drafts: Vec::new(),
        active_draft: None,
    }
}

/// The reference server's hover card facts: every thread on the "WSL-PC"
/// Linux environment, on the Replay models the scenario seeded.
fn reference_hover_details() -> std::collections::HashMap<ThreadKey, crate::components::thread_hover_card::ThreadHoverDetails> {
    let model = |id: &str| match id {
        "r-sidebar" => Some("Plan and reads · Replay (rich)"),
        "r-tls" => Some("Write approval · Replay (rich)"),
        "r-sync" => Some("Showcase · Replay (rich)"),
        _ => None,
    };
    ["r-ci", "r-handshake", "r-hero", "r-tls", "r-launch", "r-sidebar", "r-sync"]
        .into_iter()
        .map(|id| {
            let details = crate::components::thread_hover_card::ThreadHoverDetails {
                environment_label: Some("WSL-PC".into()),
                environment_machine: Some(t3code_contracts::EnvironmentMachineKind::Linux),
                model_label: model(id).map(SharedString::from),
                ..Default::default()
            };
            (ThreadKey { environment_id: env(), thread_id: ThreadId(id.into()) }, details)
        })
        .collect()
}

/// The world of the owner-configuration sidebar captures: projects that share
/// a repository (a checkout, its worktree and a subdirectory project), a
/// second repository whose other clone the settings keep separate, projects
/// without a remote, active cards, a Working shelf, snoozed and settled
/// threads and two unsent drafts, all on the Replay agent.
pub fn owner_snapshot() -> SidebarSnapshot {
    let repo = |mut p: SidebarProject, key: &str, display: &str| {
        p.repository_key = Some(key.to_string().into());
        p.repository_display_name = Some(display.to_string().into());
        p
    };
    let mut projects = vec![
        repo(project("t3code", "t3code", 1), "github.com/example/t3code", "example/t3code"),
        repo(project("t3code-review", "t3code-review", 1), "github.com/example/t3code", "example/t3code"),
        repo(project("web", "web", 1), "github.com/example/t3code", "example/t3code"),
        repo(project("relay", "relay", 1), "github.com/example/relay", "example/relay"),
        repo(project("relay-ops", "relay-ops", 1), "github.com/example/relay", "example/relay"),
        project("marketing", "marketing-site", 1),
        project("infra", "infra", 1),
    ];
    for p in &mut projects {
        p.repository_key = p.repository_key.take().filter(|_| p.title != "marketing-site" && p.title != "infra");
    }
    let replay = SidebarProvider { mark: SidebarProviderMark::AcpRegistry, initials: "RR".into(), show_badge: true };
    let sec = |s: i64| s * 1000;
    let finished = |id: &str, project: &str, title: &str, age: i64| {
        // Sent `age` ago, finished a few seconds later, not yet visited.
        let mut t = thread(id, project, title, age);
        if let Some(run) = t.latest_run.as_mut() {
            run.completed_at = Some(t.created_at.minus(-5_000));
        }
        let mut t = unread(t);
        t.branch = Some("main".into());
        t.provider = Some(replay.clone());
        t
    };
    let mut threads = Vec::new();
    for (ix, (id, project, title)) in [
        ("o-terraform", "infra", "Pin terraform provider versions"),
        ("o-video", "marketing", "Compress hero video"),
        ("o-keys", "relay-ops", "Rotate staging keys"),
        ("o-flags", "t3code", "Remove dead feature flags"),
        ("o-bundle", "web", "Audit bundle size"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut t = finished(id, project, title, 10 * MIN - ix as i64 * sec(30));
        t.last_visited_at = Some(FIXTURE_NOW);
        t.settled_override = Some(SettledOverride::Settled);
        t.settled_at = Some(FIXTURE_NOW.minus(MIN + sec(10) - ix as i64 * sec(2)));
        threads.push(t);
    }
    for (ix, (id, project, title)) in
        [("o-flaky", "relay", "Investigate flaky handshake test"), ("o-migration", "t3code-review", "Review the settings migration")]
            .into_iter()
            .enumerate()
    {
        let mut t = finished(id, project, title, 7 * MIN + sec(30) - ix as i64 * sec(30));
        t.last_visited_at = Some(FIXTURE_NOW);
        t.snoozed_until = Some(FIXTURE_NOW.minus(-14 * HOUR - 10 * MIN));
        threads.push(t);
    }
    for (ix, (id, project, title)) in [
        ("o-ci", "infra", "Migrate CI runners to arm64"),
        ("o-handshake", "relay", "Explain the relay handshake"),
        ("o-hero", "marketing", "Fix hero image layout shift"),
        ("o-tls", "relay", "Rotate relay TLS certificates"),
        ("o-launch", "marketing", "Draft launch copy for the changelog"),
        ("o-toolbar", "web", "Polish the composer toolbar"),
        ("o-sidebar", "t3code", "Port sidebar virtualization to GPUI"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut t = finished(id, project, title, 6 * MIN - ix as i64 * sec(30));
        match id {
            "o-launch" => {
                t = running(t, RunStatus::Waiting, 4 * MIN);
                t.has_pending_user_input = true;
            }
            "o-sidebar" => {
                t.last_visited_at = Some(FIXTURE_NOW);
                let started = FIXTURE_NOW.minus(MIN + sec(20));
                t.latest_run = Some(LatestRun {
                    status: RunStatus::Completed,
                    requested_at: Some(started),
                    started_at: Some(started),
                    completed_at: Some(started.minus(-sec(2))),
                });
                t.created_at = started;
                t.updated_at = started;
                t.latest_user_message_at = Some(started);
                t.latest_user_authored_message_at = Some(started);
            }
            _ => {}
        }
        threads.push(t);
    }
    for (id, project, title, branch, started) in [
        ("o-profile", "t3code", "Profile the sync engine", "main", 2 * MIN + sec(30)),
        ("o-rebase", "t3code-review", "Rebase the review branch", "review", 2 * MIN + sec(20)),
        ("o-deploy", "relay-ops", "Watch the relay deploy", "main", 2 * MIN + sec(10)),
        ("o-profile-2", "t3code", "Profile the sync engine", "main", MIN + sec(40)),
        ("o-deploy-2", "relay-ops", "Watch the relay deploy", "main", MIN + sec(30)),
        ("o-rebase-2", "t3code-review", "Rebase the review branch", "review", MIN + sec(20)),
    ] {
        let mut t = running(thread(id, project, title, started), RunStatus::Running, started);
        t.branch = Some(branch.into());
        t.provider = Some(replay.clone());
        threads.push(t);
    }
    let drafts = [
        ("draft-onboarding", "t3code", "Sketch the onboarding checklist", 50),
        ("draft-jitter", "relay", "Add retry jitter to the relay client", 40),
    ]
    .into_iter()
    .map(|(id, project, preview, ago)| SidebarDraft {
        id: id.into(),
        project: ProjectRef { environment_id: env(), project_id: ProjectId(project.into()) },
        preview: preview.into(),
        created_at: FIXTURE_NOW.minus(sec(ago)),
    })
    .collect();
    SidebarSnapshot {
        projects,
        threads: threads.into_iter().map(Arc::new).collect(),
        notifications: Vec::new(),
        settings: SidebarSettings::default(),
        active_thread: Some(ThreadKey { environment_id: env(), thread_id: ThreadId("o-sidebar".into()) }),
        environment_pill: None,
        now: FIXTURE_NOW,
        auto_settle_disabled: Default::default(),
        hover_details: Default::default(),
        drafts,
        active_draft: None,
    }
}
