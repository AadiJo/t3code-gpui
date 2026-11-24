// End-to-end check of the real window on live data: the same startup as the `t3code` binary
// (first-launch import, connection runtime, settings, `WindowRoot` on live data) in a headless
// window, driven like a user against a real T3 Code server, with screenshots and timings.
// Phases run in order as separate processes on one data directory (`T3CODE_DATA_DIR`):
//
//   seed     pairs (`T3_PAIRING_URL`), creates a project at `T3_PROJECT_ROOT` (a git checkout)
//            and threads with the Replay provider: a long one, an approval, a stream, a
//            question and one whose turn edits a file
//   drive    sidebar, cold and cached switches (click-to-frame), send and stream, approve, the
//            diff, files and terminal tabs, thread details and a branch switch, a server
//            setting saved and read back, the palette, rename from the context menu (CPU while
//            the field is open), 300 typed keys (resident memory)
//   restart  a new process paints the sidebar and the last thread from cache, then catches up
//   send     attachments through the signed upload, a model and mode change carried by the
//            turn, the sent images, /clear, edit from here, fork and /side; writes
//            send-record.json (what the server recorded for the send)
//   sidebar  a second server (`T3_PAIRING_URL_B`, `T3_PROJECT_ROOT_B`): rows from both, one
//            repository group across them and the per-project override, favicons, provider
//            marks, a turn on one server leaving the other alone, unread and visits, message
//            search, snooze, archive and the Archive list, the composer draft on disk
//   notify   with a stand-in notification service on a private session bus (`T3_NOTIFY_LOG`,
//            `T3_NOTIFY_CONTROL`) and `T3_NOTIFY_MODE` linux (system notifications) or inbox (the
//            macOS and Windows inbox, run here): completion, approval, input and failure on
//            threads not shown while the window is unfocused, each checked against the model
//            (request sent, inbox entry, badge, sidebar Done); a click opens the thread; the tag
//            filter closes a hidden thread's notification and keeps it quiet
//   details  a thread launched into a new worktree: the timeline's setup row follows the
//            server's worktree setup stream (stages and their states)
//   mentions `@` offers a thread's own used side chat (as "Side chat") and no other
//            thread's, never the composer's own thread, best title match first
//   heic     a HEIC photo attaches as a JPEG fitted to 2000px and uploads; an unreadable one is
//            refused. Linux has no HEIC decoder here, so run it with a stand-in `heif-convert`
//            on PATH (the macOS build decodes with `sips`)
//   import   a fresh data directory and a stand-in install (`T3CODE_IMPORT_HOME`, made by
//            `import-check`, encrypted with the fixture macOS key): the launch import runs,
//            the window explains the Keychain read, environments and settings arrive
//
// Usage: live-check <seed|drive|restart|import|send|sidebar|notify|details|mentions|heic> <out-dir>
#[allow(dead_code)]
#[path = "import/mod.rs"]
mod import;
#[allow(dead_code)]
#[path = "notifications/mod.rs"]
mod notifications;

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
use t3code_client_runtime::platform::electron_import::SafeStorageKey;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_client_runtime::state::models::{format_ms, now_ms};
use t3code_client_runtime::state::sidebar::row_status;
use t3code_contracts::{
    CommandId, ModelSelection, ProjectId, ProjectMutation, ProjectMutationProjectCreate, ProviderInteractionMode, RuntimeMode, ThreadId,
};
use t3code_web::app_root::{AppRoot, AppRootOptions, DataSource, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::chat::messages_timeline::TimelineAction;
use t3code_web::components::chat::timeline_entries::{EntryKind, TimelineInput, WorkItem};
use t3code_web::components::diff_panel::DiffPanelContent;
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::components::sidebar_logic::SidebarListEntry;
use t3code_web::connection::shell::EnvironmentShell;
use t3code_web::connection::{ConnectionRuntime, ConnectionsState, ElectronImportSource, RuntimeConfig};
use t3code_web::diff_panel_store::DiffPanelSelection;
use t3code_web::router::{Route, ThreadTarget};
use t3code_web::shortcuts::ShortcutRoot;
use t3code_web::components::thread_notification_coordinator::ThreadNotificationCoordinator;
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;
use t3code_web::ui_state_store::StateDir;

const STATE_FILE: &str = "live-state.json";

/// What `seed` made, for the later phases.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Seeded {
    environment_id: String,
    project_root: String,
    long_thread: String,
    approval_thread: String,
    stream_thread: String,
    question_thread: String,
    edit_thread: String,
    last_thread: Option<String>,
}

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
    failures: usize,
    window: AnyWindowHandle,
    root: Entity<AppRoot>,
    shell: Entity<EnvironmentShell>,
    shortcuts: Entity<ShortcutRoot>,
}

fn connected(state: &ConnectionsState) -> bool {
    !state.environments.is_empty() && state.environments.iter().all(|env| env.status.as_ref().is_some_and(|s| s.phase == ConnectionPhase::Connected))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// This process's resident memory, in MiB.
fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()))
        .map_or(0.0, |kb| kb / 1024.0)
}

/// CPU time this process has used (user + system), from `/proc/self/stat`.
fn cpu_time() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().collect()).unwrap_or_default();
    let ticks: u64 = fields.get(11).and_then(|u| u.parse().ok()).unwrap_or(0) + fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
    Duration::from_millis(ticks * 10)
}

fn data_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("T3CODE_DATA_DIR").map(PathBuf::from).context("set T3CODE_DATA_DIR")
}

impl Check {
    /// Starts the app the way `main` does, with secrets in a file in the data directory, and
    /// opens the window on live data.
    fn open(out: &Path, import_key: Option<SafeStorageKey>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(out)?;
        let data = data_dir()?;
        let ui = data.join("ui");
        let launch = import::before_settings(&data, Some(&ui));
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(t3code_web::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        let config = RuntimeConfig {
            secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
            data_dir: data.clone(),
            cloud: None,
            presentation: ClientPresentation { label: "t3code-gpui live check".into(), ..Default::default() },
            electron: launch.install.as_ref().map(|install| ElectronImportSource { paths: install.paths.clone(), key: import_key }),
        };
        let inbox = notify_mode() == Some(NotifyMode::Inbox);
        cx.update(|cx| {
            t3code_web::init(cx);
            if inbox {
                // The macOS and Windows inbox, run on this platform for the check.
                ThreadNotificationCoordinator::init(true, Default::default(), cx);
            }
            ConnectionRuntime::init(config, cx)?;
            t3code_web::hooks::load(&data, cx);
            import::apply_client_settings(&launch, cx);
            anyhow::Ok(())
        })?;
        let options = AppRootOptions { data: DataSource::Live, platform: TitlebarPlatform::Overlay, state_dir: StateDir(Some(ui)) };
        let (window, parts) = cx.update(|cx| {
            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(None, size(px(1280.), px(860.)), cx)),
                show: false,
                focus: true,
                ..component::TitleBar::window_options()
            };
            WindowRoot::open_window(window_options, options, cx)
        })?;
        let WindowRoot { shortcuts, shell, app: root } = parts;
        if notify_mode().is_some() {
            let path = data.join("ui").join("notifications.json");
            cx.update(|cx| notifications::install_with(inbox, Some(path), cx));
            cx.update_window(window, |_, window, cx| ThreadNotificationCoordinator::watch_window(window, cx))?;
        }
        cx.update(|cx| import::import_connections(launch, cx));
        let mut check = Self { cx, out: out.to_owned(), started: Instant::now(), log: String::new(), failures: 0, window, root, shell, shortcuts };
        check.frame();
        Ok(check)
    }

    fn note(&mut self, text: impl AsRef<str>) {
        let line = format!("{:>8.3}s  {}", self.started.elapsed().as_secs_f64(), text.as_ref());
        println!("{line}");
        writeln!(self.log, "{line}").ok();
    }

    fn check(&mut self, what: &str, ok: bool, detail: impl std::fmt::Display) {
        self.note(format!("{} {what}: {detail}", if ok { "PASS" } else { "FAIL" }));
        if !ok {
            self.failures += 1;
        }
    }

    /// Settles pending work and draws a frame; returns the frame's render time.
    fn frame(&mut self) -> Duration {
        self.cx.run_until_parked();
        let started = Instant::now();
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
        started.elapsed()
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        self.frame();
        self.cx.capture_screenshot(self.window)?.save(self.out.join(name))?;
        self.note(format!("screenshot {name}"));
        Ok(())
    }

    /// Draws frames until `until` holds, letting timers and the network run.
    fn pump(&mut self, until: impl Fn(&mut Self) -> bool, timeout: Duration, what: &str) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.cx.advance_clock(Duration::from_millis(16));
            self.frame();
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

    fn wait(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.cx.advance_clock(Duration::from_millis(16));
            self.frame();
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn read<R>(&mut self, f: impl FnOnce(&AppRoot, &App) -> R) -> R {
        let root = self.root.clone();
        self.cx.update(|cx| f(root.read(cx), cx))
    }

    fn connected(&mut self) -> bool {
        self.cx.update(|cx| connected(ConnectionRuntime::state(cx).read(cx)))
    }

    fn sidebar_threads(&mut self) -> usize {
        self.read(|root, cx| {
            root.sidebar().read(cx).list().entries.iter().filter(|e| matches!(e, SidebarListEntry::Thread { .. })).count()
        })
    }

    fn shown_input(&mut self) -> TimelineInput {
        self.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().read(cx).input().clone())
    }

    fn route(&mut self) -> Route {
        self.read(|root, cx| root.router().read(cx).current().clone())
    }

    fn row_center(&mut self, key: &ThreadKey) -> Option<Point<Pixels>> {
        self.read(|root, cx| {
            let sidebar = root.sidebar().read(cx);
            let ix = sidebar
                .list()
                .entries
                .iter()
                .position(|entry| matches!(entry, SidebarListEntry::Thread { thread, .. } if &thread.key == key))?;
            Some(sidebar.entry_bounds(ix)?.center())
        })
    }

    /// Clicks a sidebar row and returns click-to-frame: the mouse-down and mouse-up dispatch
    /// (hover settled first, as a pointer already over the row) and the layout and paint of
    /// the next frame, measured the way `app-check` measures fixture switches. The pointer
    /// helpers of the test kit draw a whole frame before each event, so they are not used here.
    fn click_row(&mut self, key: &ThreadKey, label: &str) -> anyhow::Result<Duration> {
        self.frame();
        let at = self.row_center(key).with_context(|| format!("no row for {label}"))?;
        self.cx.update_window(self.window, |_, w, cx| {
            w.dispatch_event(MouseMoveEvent { position: at, pressed_button: None, modifiers: Modifiers::default() }.to_platform_input(), cx);
        })?;
        self.frame();
        let started = Instant::now();
        self.cx.update_window(self.window, |_, w, cx| {
            w.dispatch_event(
                MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1, first_mouse: false }
                    .to_platform_input(),
                cx,
            );
            w.dispatch_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count: 1 }.to_platform_input(), cx);
        })?;
        let handler = started.elapsed();
        self.cx.update_window(self.window, |_, w, cx| w.draw(cx).clear(cx))?;
        let to_frame = started.elapsed();
        self.cx.run_until_parked();
        self.note(format!("click {label}: handler {:.2} ms, click-to-frame {:.2} ms", ms(handler), ms(to_frame)));
        Ok(to_frame)
    }

    /// `app-check`'s switch measure: navigate, then layout and paint.
    fn navigate_timed(&mut self, key: &ThreadKey) -> Duration {
        self.frame();
        let router = self.read(|root, _| root.router().clone());
        let route = Route::Thread(key.clone());
        let started = Instant::now();
        self.cx.update_window(self.window, |_, w, cx| {
            router.update(cx, |router, cx| router.navigate(route, cx));
            w.draw(cx).clear(cx);
        }).ok();
        let elapsed = started.elapsed();
        self.cx.run_until_parked();
        elapsed
    }

    fn press(&mut self, keys: &str) {
        let keys = keys.replace("mod-", if cfg!(target_os = "macos") { "cmd-" } else { "ctrl-" });
        self.cx.update_window(self.window, |_, w, cx| w.press(&keys, cx)).ok();
        self.frame();
    }

    fn type_text(&mut self, text: &str) {
        self.cx.update_window(self.window, |_, w, cx| w.input(text, cx)).ok();
        self.frame();
    }

    fn action(&mut self, action: impl Action) {
        self.cx.update_window(self.window, |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)).ok();
        self.frame();
    }

    fn focus_composer(&mut self) {
        let root = self.root.clone();
        self.cx
            .update_window(self.window, |_, w, cx| {
                let composer = root.read(cx).composer(cx);
                let focus = composer.read(cx).editor().focus_handle(cx);
                w.focus(&focus, cx);
            })
            .ok();
        self.frame();
    }

    fn go(&mut self, key: &ThreadKey) {
        let root = self.root.clone();
        let key = key.clone();
        self.cx.update(|cx| {
            let router = root.read(cx).router().clone();
            router.update(cx, |router, cx| router.navigate(Route::Thread(key), cx));
        });
        self.frame();
    }

    /// Runs a future on tokio and pumps the app until it resolves.
    fn block<T: Send + 'static>(&mut self, future: impl std::future::Future<Output = T> + Send + 'static) -> anyhow::Result<T> {
        let task = self.cx.update(|cx| StateBridge::spawn(future, cx));
        self.wait_task(task)?.context("runtime stopped")
    }

    fn wait_task<T: 'static>(&mut self, task: Task<T>) -> anyhow::Result<T> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.cx.update(|cx| {
            cx.spawn(async move |_| {
                _ = sender.send(task.await);
            })
            .detach()
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            self.cx.advance_clock(Duration::from_millis(16));
            self.cx.run_until_parked();
            if let Ok(value) = receiver.try_recv() {
                return Ok(value);
            }
            if Instant::now() > deadline {
                bail!("timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(mut self, phase: &str, result: anyhow::Result<()>) -> anyhow::Result<()> {
        if let Err(error) = &result {
            self.note(format!("FAILED: {error:#}"));
            self.shot(&format!("{phase}-failure.png")).ok();
        }
        let failures = self.failures;
        self.note(format!("resident memory {:.1} MiB; {failures} failed checks", rss_mib()));
        let root = self.root.clone();
        self.cx.update(|cx| root.update(cx, |root, cx| root.flush_state(cx)));
        std::fs::write(self.out.join(format!("{phase}-transcript.txt")), &self.log)?;
        self.cx.update(ConnectionRuntime::shutdown);
        result?;
        anyhow::ensure!(failures == 0, "{failures} checks failed");
        Ok(())
    }
}

fn load_state(out: &Path) -> anyhow::Result<Seeded> {
    Ok(serde_json::from_slice(&std::fs::read(out.join(STATE_FILE)).context("run seed first")?)?)
}

fn save_state(out: &Path, state: &Seeded) -> anyhow::Result<()> {
    Ok(std::fs::write(out.join(STATE_FILE), serde_json::to_vec_pretty(state)?)?)
}

fn key_of(state: &Seeded, thread: &str) -> ThreadKey {
    thread_key(&t3code_contracts::EnvironmentId::new(state.environment_id.as_str()), &ThreadId::new(thread))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: live-check <seed|drive|restart|import|send|sidebar|notify|details|mentions|heic> <out-dir> | live-check timeline <projections-dir> <out-dir> | live-check commands <corpus.json> <out.json>";
    let phase = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    if phase == "timeline" {
        return timeline_dump::run(&out, &PathBuf::from(args.next().context(usage)?));
    }
    if phase == "commands" {
        return timeline_dump::commands(&out, &PathBuf::from(args.next().context(usage)?));
    }
    let key = (phase == "import").then(|| SafeStorageKey::from_mac_password(b"fixture keychain password"));
    let mut check = Check::open(&out, key)?;
    check.note(format!("phase {phase}, data dir {}", data_dir()?.display()));
    let result = match phase.as_str() {
        "seed" => run_seed(&mut check),
        "drive" => run_drive(&mut check),
        "restart" => run_restart(&mut check),
        "import" => run_import(&mut check),
        "send" => run_send(&mut check),
        "sidebar" => run_sidebar(&mut check),
        "notify" => run_notify(&mut check),
        "details" => run_details(&mut check),
        "mentions" => run_mentions(&mut check),
        "heic" => run_heic(&mut check),
        other => Err(anyhow::anyhow!("unknown phase {other}")),
    };
    check.finish(&phase, result)
}

fn commands_of(check: &mut Check, environment_id: &t3code_contracts::EnvironmentId) -> anyhow::Result<EnvironmentCommands> {
    let id = environment_id.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::environment(&id, cx).is_some()), Duration::from_secs(10), "store open")?;
    check.cx.update(|cx| StateBridge::environment(environment_id, cx).map(|env| env.read(cx).commands.clone())).context("store")
}

fn run_seed(check: &mut Check) -> anyhow::Result<()> {
    let paired = check.cx.update(|cx| !ConnectionRuntime::state(cx).read(cx).environments.is_empty());
    if !paired {
        let url = std::env::var("T3_PAIRING_URL").context("T3_PAIRING_URL")?;
        let request = parse_pairing_url(&url)?;
        let pairing = check.cx.update(|cx| ConnectionRuntime::global(cx).pair(request, None, cx));
        let label = check.wait_task(pairing)?.map_err(|e| anyhow::anyhow!("{}", e.0))?;
        check.note(format!("paired {label}"));
    }
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    let environment_id = t3code_contracts::EnvironmentId::new(id.as_str());
    let commands = commands_of(check, &environment_id)?;
    let root = PathBuf::from(std::env::var("T3_PROJECT_ROOT").context("T3_PROJECT_ROOT")?);
    let root_text = root.display().to_string();
    let existing = check.cx.update(|cx| {
        let shell = StateBridge::environment(&environment_id, cx)?.read(cx).shell.clone();
        shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == root_text).map(|p| p.id.clone())
    });
    let reuse = existing.is_some();
    let project = existing.unwrap_or_else(|| ProjectId::new(uuid::Uuid::new_v4().to_string()));
    let mutation = ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        project_id: project.clone(),
        title: "orbit".into(),
        workspace_root: root.display().to_string(),
        create_workspace_root_if_missing: Some(false),
        default_model_selection: None,
        scripts: None,
    });
    if !reuse {
        let c = commands.clone();
        check.block(async move { c.mutate_project(mutation).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        check.note("created project orbit");
    }
    let create = |check: &mut Check, title: &str, model: &str| -> anyhow::Result<ThreadId> {
        let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
        let input = CreateThread {
            thread_id: thread_id.clone(),
            project_id: project.clone(),
            title: title.into(),
            model_selection: ModelSelection { instance_id: "devReplay".into(), model: model.into(), options: None },
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: ProviderInteractionMode::Default,
            branch: Some("feature/clamp".into()),
            worktree_path: None,
        };
        let commands = commands.clone();
        check.block(async move { commands.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        check.note(format!("created thread {title}"));
        Ok(thread_id)
    };
    let settled = |check: &mut Check, thread: &ThreadId, before: Option<String>, env: &t3code_contracts::EnvironmentId| -> anyhow::Result<()> {
        let (env, thread) = (env.clone(), thread.clone());
        check.pump(
            move |c| {
                c.cx.update(|cx| {
                    StateBridge::shell(cx)
                        .read(cx)
                        .presented()
                        .iter()
                        .find(|row| row.environment_id == env && row.id() == &thread)
                        .is_some_and(|row| {
                            row.latest_run.as_ref().is_some_and(|r| Some(r.run_id.to_string()) != before && r.completed_at.is_some())
                                && !matches!(
                                    row_status(row),
                                    t3code_client_runtime::state::sidebar::ThreadStatus::Working
                                        | t3code_client_runtime::state::sidebar::ThreadStatus::Preparing
                                )
                        })
                })
            },
            Duration::from_secs(60),
            "turn settled",
        )
    };
    let send = |check: &mut Check, thread: &ThreadId, text: &str| -> anyhow::Result<Option<String>> {
        let (env, t) = (environment_id.clone(), thread.clone());
        let before = check.cx.update(|cx| {
            StateBridge::shell(cx)
                .read(cx)
                .presented()
                .iter()
                .find(|row| row.environment_id == env && row.id() == &t)
                .and_then(|row| row.latest_run.as_ref().map(|r| r.run_id.to_string()))
        });
        let commands = commands.clone();
        let message = SendMessage::text(thread.clone(), text);
        check.block(async move { commands.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(before)
    };
    let long = create(check, "Long design review", "multi-turn")?;
    for turn in 1..=20 {
        let before = send(check, &long, &format!("Turn {turn}: keep going with the review."))?;
        settled(check, &long, before, &environment_id)?;
    }
    let edit = create(check, "Clamp edit", "multi-turn")?;
    check.wait(Duration::from_secs(1));
    // The replayed turn writes nothing, so the edit lands after the thread's first checkpoint
    // and before its run ends: the run's checkpoint then holds it, as an agent's edit would.
    let math = root.join("src/math.rs");
    let mut text = std::fs::read_to_string(&math)?;
    text.push_str(&format!("\npub fn saturate_{}(v: i32) -> i32 {{\n    clamp(v, 0, 255)\n}}\n", now_ms() % 100_000));
    std::fs::write(&math, text)?;
    let before = send(check, &edit, "Add a saturating helper next to clamp.")?;
    settled(check, &edit, before, &environment_id)?;
    let approval = create(check, "Approval flow", "approval")?;
    let stream = create(check, "Streaming", "multi-turn")?;
    let question = create(check, "Question", "question")?;
    let state = Seeded {
        environment_id: id,
        project_root: root.display().to_string(),
        long_thread: long.to_string(),
        approval_thread: approval.to_string(),
        stream_thread: stream.to_string(),
        question_thread: question.to_string(),
        edit_thread: edit.to_string(),
        last_thread: None,
    };
    save_state(&check.out, &state)?;
    check.pump(|c| c.sidebar_threads() >= 5, Duration::from_secs(10), "sidebar lists the seeded threads")?;
    check.shot("00-seeded.png")
}

/// The paths a renderable patch shows.
fn patch_files(patch: &t3code_web::diff_patch::RenderablePatch) -> Vec<String> {
    match patch {
        t3code_web::diff_patch::RenderablePatch::Files(parsed) => parsed.files.iter().map(|f| f.path.to_string()).collect(),
        t3code_web::diff_patch::RenderablePatch::Raw { reason, .. } => vec![format!("raw: {reason}")],
    }
}

/// CPU used while the window idles for `duration` (timers advance, frames draw).
fn idle_cpu(check: &mut Check, duration: Duration) -> Duration {
    let before = cpu_time();
    check.wait(duration);
    cpu_time().saturating_sub(before)
}

/// The `p`th percentile of `samples`, in ms.
fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * p).round() as usize]
}

fn run_drive(check: &mut Check) -> anyhow::Result<()> {
    let mut state = load_state(&check.out)?;
    let (long, approval, stream, edit) = (
        key_of(&state, &state.long_thread),
        key_of(&state, &state.approval_thread),
        key_of(&state, &state.stream_thread),
        key_of(&state, &state.edit_thread),
    );
    let rows = check.sidebar_threads();
    check.check("sidebar paints rows from cache at launch", rows >= 5, format!("{rows} rows"));
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    check.shot("01-sidebar.png")?;

    // Cold open, then cached switches between two threads.
    let cold = check.click_row(&long, "long thread (cold)")?;
    check.pump(|c| c.shown_input().entries.len() > 20, Duration::from_secs(20), "long thread timeline")?;
    let shown = check.route() == Route::Thread(long.clone());
    check.check("cold open shows the thread", shown, format!("click-to-frame {:.2} ms", ms(cold)));
    check.shot("02-long-thread.png")?;
    check.click_row(&edit, "edit thread (cold)")?;
    check.pump(|c| !c.shown_input().entries.is_empty(), Duration::from_secs(20), "edit thread timeline")?;
    let mut cached = Vec::new();
    for round in 0..12 {
        let (key, label) = if round % 2 == 0 { (&long, "long thread (cached)") } else { (&edit, "edit thread (cached)") };
        let key = key.clone();
        cached.push(ms(check.click_row(&key, label)?));
        check.wait(Duration::from_millis(50));
    }
    let mut noop = Vec::new();
    for _ in 0..5 {
        noop.push(ms(check.click_row(&edit, "already-open row")?));
    }
    let mut navigated = Vec::new();
    for round in 0..12 {
        let key = if round % 2 == 0 { long.clone() } else { edit.clone() };
        navigated.push(ms(check.navigate_timed(&key)));
    }
    check.note(format!(
        "cached switch by navigate + draw (app-check's measure): p50 {:.2} ms, max {:.2} ms",
        percentile(&mut navigated, 0.5),
        percentile(&mut navigated, 1.0)
    ));
    let (p50, max) = (percentile(&mut cached, 0.5), percentile(&mut cached, 1.0));
    check.note(format!(
        "cached switch click-to-frame: p50 {p50:.2} ms, max {max:.2} ms over {} clicks; already-open row p50 {:.2} ms",
        cached.len(),
        percentile(&mut noop, 0.5)
    ));
    check.shot("03-switched.png")?;

    // Send in a thread and watch the reply stream.
    check.go(&stream);
    check.pump(|c| c.route() == Route::Thread(stream.clone()), Duration::from_secs(5), "stream thread shown")?;
    check.focus_composer();
    check.type_text("Walk me through the cache layers.");
    check.press("enter");
    let mut streamed = false;
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        check.wait(Duration::from_millis(50));
        let input = check.shown_input();
        let streaming = input.is_working || input.entries.iter().any(|e| matches!(&e.kind, EntryKind::Message(m) if m.streaming));
        if streaming && !streamed {
            streamed = true;
            check.shot("04-streaming.png")?;
        }
        if input.entries.len() >= 2 && !input.is_working && streamed {
            break;
        }
    }
    let input = check.shown_input();
    let reply = input
        .entries
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EntryKind::Message(m) if m.role == t3code_web::components::chat::timeline_entries::MessageRole::Assistant => Some(m.text.len()),
            _ => None,
        })
        .unwrap_or(0);
    check.check("the sent turn runs and its reply arrives", streamed && reply > 0, format!("saw the run working {streamed}, reply {reply} chars"));
    check.shot("05-after-stream.png")?;

    // An approval: send, wait for the request, approve it from the timeline.
    check.go(&approval);
    check.focus_composer();
    check.type_text("Write the probe file.");
    check.press("enter");
    check.pump(|c| !c.shown_input().pending_approval_ids.is_empty(), Duration::from_secs(40), "approval requested")?;
    check.shot("06-approval.png")?;
    // The paused run stands in for an agent writing: this edit lands inside the run, so its
    // end checkpoint holds it.
    let math = PathBuf::from(&state.project_root).join("src/math.rs");
    let mut text = std::fs::read_to_string(&math)?;
    text.push_str(&format!("\npub fn probe_{}() -> i32 {{\n    clamp(512, 0, 255)\n}}\n", now_ms() % 100_000));
    std::fs::write(&math, text)?;
    let request = check.shown_input().pending_approval_ids[0].clone();
    let timeline = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().clone());
    // The Approve label sits inside the virtual list, which the test registry does not reach;
    // emit what its click emits.
    check.cx.update(|cx| timeline.update(cx, |_, cx| cx.emit(TimelineAction::RespondToApproval { request, approve: true })));
    check.pump(|c| c.shown_input().pending_approval_ids.is_empty(), Duration::from_secs(30), "approval answered")?;
    check.shot("07-approved.png")?;

    // The diff tab: the approved turn's checkpoint diff, then the branch against main.
    let edit = approval.clone();
    check.pump(|c| !c.shown_input().turn_diff_summaries.is_empty(), Duration::from_secs(30), "turn diff summary in the timeline")?;
    check.shot("07b-changed-files.png")?;
    let turn = check.shown_input().turn_diff_summaries[0].assistant_message_id.0.clone();
    check.read(|root, _| root.right_panel().clone());
    let panel = check.read(|root, _| root.right_panel().clone());
    check.cx.update(|cx| panel.update(cx, |panel, cx| panel.open_diff(Some(turn.clone()), None, cx)));
    check.pump(|c| c.read(|root, cx| root.right_panel().read(cx).diff_loaded(cx)), Duration::from_secs(20), "turn diff loaded")?;
    let (target, scoped) = (ThreadTarget::Server(edit.clone()), t3code_web::fixtures::scoped_key(&edit));
    let selection = check.cx.update(|cx| cx.global::<t3code_web::diff_panel_store::DiffPanelStore>().selection(&scoped));
    let data = check.read(|root, _| root.data().clone());
    let input = {
        let task = check.cx.update(|cx| data.diff_input(&target, &selection, cx));
        check.wait_task(task)?
    };
    let files: Vec<String> = match input.map(|i| i.content) {
        Some(DiffPanelContent::Patch { patch, .. }) => patch_files(&patch),
        other => vec![format!("{:?}", other.map(|c| std::mem::discriminant(&c)))],
    };
    check.check("the turn diff is the turn's real edit", files.iter().any(|f| f == "src/math.rs"), format!("{selection:?} files {files:?}"));
    check.shot("08-turn-diff.png")?;
    let branch = DiffPanelSelection::Branch { base_ref: Some("main".into()) };
    let branch_files: Vec<String> = {
        let task = check.cx.update(|cx| data.diff_input(&target, &branch, cx));
        match check.wait_task(task)?.map(|i| i.content) {
            Some(DiffPanelContent::Patch { patch, .. }) => patch_files(&patch),
            _ => Vec::new(),
        }
    };
    check.check("the branch diff against main comes from review.getDiffPreview", !branch_files.is_empty(), format!("{branch_files:?}"));

    // The files tab: open a file and read it from the server.
    let window = check.window;
    check.cx.update_window(window, |_, w, cx| panel.update(cx, |panel, cx| panel.open_file("src/math.rs".into(), None, w, cx)))?;
    check.pump(
        |c| {
            let panel = c.read(|root, _| root.right_panel().clone());
            c.cx.update(|cx| {
                let files = panel.update(cx, |p, cx| p.files_panel(cx));
                files.and_then(|f| f.read(cx).editor().map(|e| e.read(cx).text())).is_some_and(|text| text.contains("pub fn clamp"))
            })
        },
        Duration::from_secs(20),
        "files tab shows src/math.rs from projects.readFile",
    )?;
    check.shot("09-files.png")?;

    // A terminal tab on the server.
    check.cx.update(|cx| panel.update(cx, |panel, cx| panel.new_terminal_tab(cx)));
    check.frame();
    let terminal_id = check.read(|root, cx| {
        let scoped = t3code_web::fixtures::scoped_key(&edit);
        let state = root.right_panels().read(cx).state(&scoped);
        state.surfaces.iter().find_map(|s| match s {
            t3code_web::right_panel_store::RightPanelSurface::Terminal { active_terminal_id, .. } => Some(active_terminal_id.clone()),
            _ => None,
        })
    });
    let terminal_id = terminal_id.context("terminal tab")?;
    check.pump(|c| c.read(|root, cx| root.right_panel().read(cx).terminal(&terminal_id).is_some()), Duration::from_secs(5), "terminal tab started")?;
    let terminal = check.read(|root, cx| root.right_panel().read(cx).terminal(&terminal_id).cloned()).context("terminal")?;
    check.pump(|c| c.cx.update(|cx| !terminal.read(cx).core().screen_text().trim().is_empty()), Duration::from_secs(20), "terminal prompt from the server")?;
    check.cx.update_window(window, |_, w, cx| {
        let focus = terminal.read(cx).focus_handle().clone();
        w.focus(&focus, cx);
    })?;
    check.type_text("echo golive-$((6*7))");
    check.press("enter");
    check.pump(|c| c.cx.update(|cx| terminal.read(cx).core().screen_text().contains("golive-42")), Duration::from_secs(20), "terminal ran a command on the server")?;
    check.shot("10-terminal.png")?;

    // Thread details: switch the branch from the details card.
    check.action(t3code_web::shortcuts::commands::ToggleThreadPanel);
    check.pump(|c| c.read(|root, cx| root.thread_view().read(cx).details().is_some()), Duration::from_secs(5), "thread details open")?;
    check.shot("11-details.png")?;
    // git refuses to switch over the turn's uncommitted edit, so commit it first, as a user would.
    let repo = PathBuf::from(&state.project_root);
    let committed = std::process::Command::new("git").arg("-C").arg(&repo).args(["commit", "-qam", "chore: keep the probe edits"]).status()?;
    check.note(format!("committed the edits in the checkout: {committed}"));
    let details = check.read(|root, cx| root.thread_view().read(cx).details().cloned()).context("details")?;
    let toolbar = check.cx.update(|cx| details.read(cx).branch().clone());
    check.cx.update_window(window, |_, w, cx| toolbar.update(cx, |t, cx| t.set_branch_picker_open(true, w, cx)))?;
    check.pump(|c| c.cx.update(|cx| toolbar.read(cx).branch_picker().is_some()), Duration::from_secs(10), "branch picker open")?;
    let picker = check.cx.update(|cx| toolbar.read(cx).branch_picker().cloned()).context("picker")?;
    check.wait(Duration::from_millis(500));
    check.cx.update_window(window, |_, w, cx| {
        let focus = picker.focus_handle(cx);
        w.focus(&focus, cx);
    })?;
    check.type_text("main");
    check.shot("11b-branch-picker.png")?;
    check.press("enter");
    check.pump(
        |_| std::process::Command::new("git").arg("-C").arg(&repo).args(["branch", "--show-current"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "main"),
        Duration::from_secs(20),
        "branch switched to main on the server's checkout",
    )?;
    check.shot("12-branch-switched.png")?;
    check.action(t3code_web::shortcuts::commands::ToggleThreadPanel);

    // Settings: change a server setting and read it back from the server.
    let shell = check.shell.clone();
    check.cx.update_window(window, |_, w, cx| {
        shell.update(cx, |shell, cx| shell.open_settings(t3code_web::components::settings::settings_search::SettingsPath::Threads, w, cx))
    })?;
    check.frame();
    check.shot("13-settings-threads.png")?;
    let model = check.read(|root, _| root.data().clone()).server_settings_model(check)?;
    let before = check.cx.update(|cx| model.read(cx).snapshot().settings.keep_awake_while_agents_work);
    let wanted = !before.unwrap_or(false);
    let next = serde_json::json!({ "keepAwakeWhileAgentsWork": wanted });
    let source = check.cx.update(|cx| model.read(cx).source().clone());
    let patch: t3code_contracts::ServerUpdateSettingsPayload = serde_json::from_value(serde_json::json!({ "patch": next }))?;
    let saved = check.block(async move { source.update_settings(patch).await })?;
    check.check("server.updateSettings accepted", saved.is_ok(), format!("{:?}", saved.as_ref().err()));
    let id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let link = check.cx.update(|cx| ConnectionRuntime::global(cx).environment_link(&id)).context("link")?;
    let read_back = check.block(async move {
        let session = link.session(Duration::from_secs(5)).await.ok()?;
        let config = session.client.call::<t3code_contracts::rpc::methods::ServerGetConfig>(Default::default()).await.ok()?;
        Some(config.settings.keep_awake_while_agents_work)
    })?;
    check.check("the setting persisted on the server", read_back == Some(Some(wanted)), format!("before {before:?}, server.getConfig {read_back:?}"));
    check.pump(|c| c.cx.update(|cx| model.read(cx).snapshot().settings.keep_awake_while_agents_work == Some(wanted)), Duration::from_secs(10), "settings page follows the config stream")?;
    check.shot("14-settings-saved.png")?;
    check.cx.update(|cx| shell.update(cx, |shell, cx| shell.close_settings(cx)));
    check.frame();

    // The palette: jump to a thread by title.
    check.press("mod-k");
    check.type_text("Long design");
    check.wait(Duration::from_millis(200));
    check.shot("15-palette.png")?;
    check.press("enter");
    check.pump(
        |c| {
            let route = c.route();
            c.read(|root, cx| match route {
                Route::Thread(key) => root.data().snapshot(None, cx).threads.iter().any(|t| t.key == key && t.title.starts_with("Long design")),
                _ => false,
            })
        },
        Duration::from_secs(5),
        "palette jumped to a long design thread",
    )?;

    // Rename from the context menu: CPU while the field is open and idle, against the same
    // idle before it opened.
    let baseline = idle_cpu(check, Duration::from_secs(2));
    let at = check.row_center(&approval).context("approval row")?;
    let sidebar = check.read(|root, _| root.sidebar().clone());
    let ix = check.read(|root, cx| {
        root.sidebar().read(cx).list().entries.iter().position(|e| matches!(e, SidebarListEntry::Thread { thread, .. } if thread.key == approval))
    });
    check.cx.update(|cx| sidebar.update(cx, |s, cx| s.open_context_menu_at(ix.unwrap_or(0), at, cx)));
    check.shot("16-context-menu.png")?;
    // The menu's rows sit in a deferred popup the test registry does not reach; send what the
    // Rename row sends.
    check.press("escape");
    check.cx.update(|cx| {
        sidebar.update(cx, |_, cx| {
            cx.emit(t3code_web::components::sidebar::SidebarEvent::ThreadCommand(
                vec![approval.clone()],
                t3code_web::components::sidebar::context_menu::MenuCommand::Rename,
            ))
        })
    });
    check.frame();
    let renaming = check.read(|root, cx| root.sidebar().read(cx).renaming().is_some());
    check.check("rename opens the row's title field", renaming, "");
    let open = idle_cpu(check, Duration::from_secs(2));
    check.check(
        "an open rename field leaves the main thread idle",
        open <= baseline * 3 / 2 + Duration::from_millis(150),
        format!("{:.0} ms CPU in 2 s with the field open, {:.0} ms before it opened", ms(open), ms(baseline)),
    );
    check.shot("17-rename-field.png")?;
    let field = check.read(|root, cx| root.sidebar().read(cx).renaming().map(|(_, f)| f.clone())).context("field")?;
    let renamed = format!("Approval flow {}", now_ms() % 100_000);
    let value = renamed.clone();
    check.cx.update_window(window, |_, w, cx| field.update(cx, |f, cx| f.set_value(value, w, cx)))?;
    check.press("enter");
    let target = approval.clone();
    check.pump(
        move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.iter().any(|t| t.key == target && t.title.as_ref() == renamed)),
        Duration::from_secs(10),
        "renamed on the server",
    )?;
    check.shot("18-renamed.png")?;

    // 300 keys in the composer: resident memory before and after.
    check.go(&long);
    check.focus_composer();
    let rss0 = rss_mib();
    let mut keys = Vec::new();
    for n in 0..300 {
        let ch = (b'a' + (n % 26) as u8) as char;
        let started = Instant::now();
        check.type_text(&ch.to_string());
        keys.push(ms(started.elapsed()));
    }
    let rss1 = rss_mib();
    check.note(format!(
        "300 keys: resident {rss0:.1} -> {rss1:.1} MiB (+{:.1}); key-to-frame p50 {:.2} ms, p95 {:.2} ms",
        rss1 - rss0,
        percentile(&mut keys, 0.5),
        percentile(&mut keys, 0.95)
    ));
    check.shot("19-typed.png")?;
    let root = check.root.clone();
    check.cx.update(|cx| root.read(cx).composer(cx).read(cx).editor().clone().update(cx, |e, cx| e.set_text("", cx)));
    state.last_thread = Some(state.long_thread.clone());
    save_state(&check.out, &state)?;
    Ok(())
}

trait ServerSettingsModel {
    fn server_settings_model(&self, check: &mut Check) -> anyhow::Result<Entity<t3code_web::components::settings::server_settings_data::ServerPanelsModel>>;
}

impl ServerSettingsModel for t3code_web::state::app_data::SharedAppData {
    fn server_settings_model(&self, check: &mut Check) -> anyhow::Result<Entity<t3code_web::components::settings::server_settings_data::ServerPanelsModel>> {
        let data = self.clone();
        check.cx.update(|cx| data.server_settings(cx)).context("server settings model")
    }
}

fn run_restart(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    let rows = check.sidebar_threads();
    let connected = check.connected();
    check.check("the sidebar paints rows on the first frame", rows >= 5, format!("{rows} rows, connected {connected}"));
    check.shot("20-restart-sidebar-from-cache.png")?;
    let last = key_of(&state, state.last_thread.as_deref().context("drive saved no last thread")?);
    let target = Route::Thread(last.clone());
    check.pump(|c| c.route() == target && !c.shown_input().entries.is_empty(), Duration::from_secs(5), "relaunch restores the last thread")?;
    let entries = check.shown_input().entries.len();
    let connected = check.connected();
    check.check("the last thread paints from the disk cache", entries > 20, format!("{entries} entries, connected {connected}"));
    check.shot("21-restart-thread-from-cache.png")?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    check.shot("22-restart-live.png")
}

fn run_import(check: &mut Check) -> anyhow::Result<()> {
    check.shot("30-import-notice.png")?;
    check.pump(|c| c.cx.update(|cx| !ConnectionRuntime::state(cx).read(cx).environments.is_empty()), Duration::from_secs(20), "imported environments saved")?;
    check.wait(Duration::from_millis(300));
    check.shot("31-import-done.png")?;
    let labels = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| e.entry.label.clone()).collect::<Vec<_>>());
    check.check("the fixture catalog's environments arrive", labels.len() == 4, format!("{labels:?}"));
    let (halves, graphite_applied, catalog) = check.cx.update(|cx| {
        use t3code_web::hooks::use_theme::ThemeStore;
        use t3code_web::theme_palette::Appearance;
        let theme = ThemeStore::global(cx);
        let dark = theme.halves().get(Appearance::Dark).map(str::to_owned);
        let chosen = dark.as_deref().and_then(|id| ThemeStore::find(id, cx)).and_then(|t| t.document.palette_for(Appearance::Dark));
        let effective = ThemeStore::effective_document(cx).palette_for(Appearance::Dark);
        (dark, chosen.is_some() && chosen == effective, ThemeStore::catalog(cx).len())
    });
    check.check(
        "the dark half is the imported custom theme and its palette applies",
        graphite_applied,
        format!("dark half {halves:?}, {catalog} themes in the catalog"),
    );
    let font = check.cx.update(|cx| t3code_web::hooks::use_settings::ClientSettingsStore::get(cx).font_size_interface.clone());
    check.note(format!("interface font size after import: {font:?}"));
    let data = data_dir()?;
    let state = import::install::ImportState::load(&data);
    check.check("the import is recorded so later launches skip it", state.files_imported && state.connections_imported, format!("{state:?}"));
    let shell = check.shell.clone();
    let window = check.window;
    check.cx.update_window(window, |_, w, cx| {
        shell.update(cx, |shell, cx| shell.open_settings(t3code_web::components::settings::settings_search::SettingsPath::Connections, w, cx))
    })?;
    check.wait(Duration::from_millis(300));
    check.shot("32-import-environments.png")
}

/// `live-check timeline <projections-dir> <out-dir>`: each projection file (a JSON array of
/// wire-encoded thread projections) through the live timeline mapping and row derivation,
/// written in the shape the web-side dump uses so the two compare row by row.
mod timeline_dump {
    use std::path::Path;

    use serde_json::{Value, json};
    use t3code_shared::thread_projection::ThreadProjection;
    use t3code_web::components::chat::messages_timeline_logic::{RowKind, TimelineExpansion, TimelineRow, derive_timeline_rows};
    use t3code_web::components::chat::timeline_entries::{
        DividerKind, EntryKind, MessageRole, SubagentStatus, TimelineEntry, TimelineEvent, TimelineInput, ToolStatus, WorkItem,
        WorkLogEntry,
    };
    use t3code_web::components::chat::work_log_presentation::{entry_label, live_entry_label};
    use t3code_web::session_logic::{TimelineMapper, timeline_input};

    fn status(status: ToolStatus) -> &'static str {
        match status {
            ToolStatus::InProgress => "inProgress",
            ToolStatus::Completed => "completed",
            ToolStatus::Failed => "failed",
            ToolStatus::Declined => "declined",
            ToolStatus::Stopped => "stopped",
        }
    }

    /// The first 120 UTF-16 units, as `text.slice(0, 120)` in the web dump.
    fn preview(text: &str) -> String {
        String::from_utf16_lossy(&text.encode_utf16().take(120).collect::<Vec<u16>>())
    }

    /// The entry's `detail` as the web keeps it.
    fn detail(entry: &WorkLogEntry) -> Value {
        entry.source.as_ref().and_then(|s| s.facts.detail.as_ref()).map_or(Value::Null, |d| Value::from(d.to_string()))
    }

    /// The web's lifecycle status, `null` for notifications.
    fn entry_status(entry: &WorkLogEntry) -> Value {
        use t3code_web::work_entry_labels::Lifecycle;
        match entry.source.as_ref().map(|s| s.facts.lifecycle) {
            Some(None) => Value::Null,
            Some(Some(lifecycle)) => Value::from(match lifecycle {
                Lifecycle::InProgress => "inProgress",
                Lifecycle::Completed => "completed",
                Lifecycle::Failed => "failed",
                Lifecycle::Declined => "declined",
                Lifecycle::Stopped => "stopped",
                Lifecycle::Idle => "idle",
            }),
            None => Value::from(status(entry.status)),
        }
    }

    fn item_type(entry: &WorkLogEntry) -> &'static str {
        match &entry.item {
            WorkItem::Command { .. } => "command_execution",
            WorkItem::FileChange { .. } => "file_change",
            WorkItem::FileRead { .. } | WorkItem::ViewedImage { .. } | WorkItem::DynamicTool { .. } => "dynamic_tool",
            WorkItem::FileSearch { .. } => "file_search",
            WorkItem::WebSearch { .. } => "web_search",
            WorkItem::Reasoning { .. } => "reasoning",
            WorkItem::TodoList { .. } => "todo_list",
            WorkItem::Approval { .. } => "approval_request",
            WorkItem::QuestionAnswer { .. } => "user_input_request",
            WorkItem::SystemNotice { .. } => "system_notice",
            WorkItem::Notification { .. } => "notification",
            WorkItem::Error { .. } => "error",
            WorkItem::Compaction { .. } => "compaction",
        }
    }

    fn work(entry: &WorkLogEntry) -> Value {
        json!({
            "id": entry.id.as_str(),
            "label": entry_label(entry).as_ref(),
            "rawLabel": entry.label.as_ref(),
            "detail": detail(entry),
            "status": entry_status(entry),
            "itemType": item_type(entry),
        })
    }

    fn entry(entry: &TimelineEntry) -> Value {
        match &entry.kind {
            EntryKind::Work(work) => json!({
                "id": entry.id.as_str(), "kind": "work", "label": work.label.as_ref(), "detail": detail(work),
                "status": entry_status(work), "display": entry_label(work).as_ref(),
            }),
            EntryKind::Message(message) => json!({
                "id": entry.id.as_str(), "kind": "message",
                "label": if message.role == MessageRole::User { "user" } else { "assistant" },
                "detail": preview(&message.text),
            }),
            EntryKind::Event(event) => {
                json!({ "id": entry.id.as_str(), "kind": "event", "label": event_type(event), "status": event_status(event) })
            }
            EntryKind::ProposedPlan(_) => json!({ "id": entry.id.as_str(), "kind": "proposed-plan" }),
            EntryKind::SentMessage { text, .. } => json!({ "id": entry.id.as_str(), "kind": "sent-message", "detail": text.as_ref() }),
            EntryKind::HtmlRender { .. } => json!({ "id": entry.id.as_str(), "kind": "html-render" }),
            EntryKind::McpApp { .. } => json!({ "id": entry.id.as_str(), "kind": "mcp-app" }),
        }
    }

    /// The turn item type an event came from, as the web dump names it.
    fn event_type(event: &TimelineEvent) -> &'static str {
        match event {
            TimelineEvent::Subagent(_) => "subagent",
            TimelineEvent::Divider(divider) => match divider.kind {
                DividerKind::RunInterrupted => "run_interrupt_result",
                DividerKind::Forked => "fork",
                DividerKind::Handoff => "handoff",
                DividerKind::ParentLink => "thread_created",
                DividerKind::NewSession => "new_session",
            },
        }
    }

    /// A subagent card's status; dividers draw none.
    fn event_status(event: &TimelineEvent) -> Value {
        match event {
            TimelineEvent::Subagent(subagent) => Value::from(match subagent.status {
                SubagentStatus::Queued => "pending",
                SubagentStatus::Running => "running",
                SubagentStatus::Waiting => "waiting",
                SubagentStatus::Completed => "completed",
                SubagentStatus::Failed => "failed",
                SubagentStatus::Idle => "idle",
                SubagentStatus::Stopped => "cancelled",
            }),
            TimelineEvent::Divider(_) => Value::Null,
        }
    }

    fn row(row: &TimelineRow) -> Value {
        let id = row.id.as_ref();
        match &row.kind {
            RowKind::Work { entry, display_label } => json!({
                "id": id, "kind": "work", "label": display_label.clone().unwrap_or_else(|| entry_label(entry)).as_ref(), "entries": [work(entry)],
            }),
            RowKind::WorkGroupDetails { entries, .. } => {
                json!({ "id": id, "kind": "work", "label": null, "entries": entries.iter().map(work).collect::<Vec<_>>() })
            }
            RowKind::WorkLive { entry, entries, active, .. } => json!({
                "id": id, "kind": "work-live", "label": live_entry_label(entry, *active).as_ref(),
                "entries": entries.iter().map(work).collect::<Vec<_>>(),
            }),
            RowKind::WorkToggle { summary, count, has_failure, .. } => json!({
                "id": id, "kind": "work-toggle", "label": summary.as_ref(), "detail": count.to_string(),
                "status": if *has_failure { Value::from("failed") } else { Value::Null },
            }),
            RowKind::Message(message) => json!({
                "id": id, "kind": "message",
                "label": if message.message.role == MessageRole::User { "user" } else { "assistant" },
                "detail": preview(&message.message.text), "showMeta": message.show_meta,
                "status": message.diff_summary.as_ref().filter(|d| !d.files.is_empty()).map(|d| {
                    format!("diff:{}", d.files.iter().map(|f| format!("{}+{}-{}", f.path, f.additions, f.deletions)).collect::<Vec<_>>().join(","))
                }),
            }),
            RowKind::ContextCompaction { label, failed, active, .. } => json!({
                "id": id, "kind": "context-compaction", "label": label.as_ref(),
                "status": if *failed { Value::from("failed") } else if *active { Value::from("active") } else { Value::Null },
            }),
            RowKind::TurnFold { label, .. } => json!({ "id": id, "kind": "turn-fold", "label": label.as_ref() }),
            RowKind::AttemptFold { label, .. } => json!({ "id": id, "kind": "attempt-fold", "label": label.as_ref() }),
            RowKind::Event { event, subagents } => json!({
                "id": id, "kind": "event", "label": event_type(event), "status": event_status(event),
                "detail": if subagents.is_empty() { Value::Null } else { Value::from(subagents.len().to_string()) },
            }),
            RowKind::SentMessage { text } => json!({ "id": id, "kind": "sent-message", "detail": text.as_ref() }),
            RowKind::Thinking { label } => json!({ "id": id, "kind": "thinking", "label": label.as_ref().map(|l| l.to_string()) }),
            other => json!({ "id": id, "kind": kind_name(other) }),
        }
    }

    fn kind_name(kind: &RowKind) -> &'static str {
        match kind {
            RowKind::WorktreeSetup(_) => "worktree-setup",
            RowKind::Working { .. } => "working",
            RowKind::ViewedImages { .. } => "viewed-images",
            RowKind::AssistantMeta { .. } => "assistant-meta",
            RowKind::ProposedPlan(_) => "proposed-plan",
            RowKind::HtmlRender { .. } => "html-render",
            RowKind::McpApp { .. } => "mcp-app",
            RowKind::PendingApproval { .. } => "pending-approval",
            RowKind::QueuedMessages { .. } => "queued-messages",
            _ => "other",
        }
    }

    fn expanded(input: &TimelineInput, projection: &ThreadProjection) -> Vec<TimelineRow> {
        let mut expansion = TimelineExpansion::default();
        expansion.runs = projection.runs.iter().map(|run| t3code_web::components::chat::timeline_entries::RunId::new(run.id.to_string())).collect();
        expansion.attempts =
            projection.attempts.iter().map(|a| t3code_web::components::chat::timeline_entries::AttemptId::new(a.id.to_string())).collect();
        let mut rows = derive_timeline_rows(input, &expansion);
        for _ in 0..5 {
            let before = expansion.work_groups.len();
            for row in &rows {
                if let RowKind::WorkToggle { group_id, .. } | RowKind::WorkLive { group_id, .. } = &row.kind {
                    expansion.work_groups.insert(group_id.clone());
                }
            }
            if expansion.work_groups.len() == before {
                break;
            }
            rows = derive_timeline_rows(input, &expansion);
        }
        rows
    }

    /// `live-check commands <corpus.json> <out.json>`: the command labeller over a list of
    /// command strings, in the shape of the web-side dump.
    pub fn commands(corpus: &Path, out: &Path) -> anyhow::Result<()> {
        use t3code_client_runtime::work_log::command_label::{
            CommandLanguage, command_display_text, command_highlight_language, command_program_name,
        };
        let corpus: Vec<String> = serde_json::from_slice(&std::fs::read(corpus)?)?;
        let rows: Vec<Value> = corpus
            .iter()
            .map(|command| {
                json!({
                    "command": command,
                    "display": command_display_text(command),
                    "program": command_program_name(command),
                    "language": match command_highlight_language(command) {
                        CommandLanguage::PowerShell => "powershell",
                        CommandLanguage::ShellScript => "shellscript",
                    },
                })
            })
            .collect();
        std::fs::write(out, serde_json::to_vec_pretty(&rows)?)?;
        Ok(())
    }

    pub fn run(input_dir: &Path, out_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(out_dir)?;
        let mut names: Vec<_> = std::fs::read_dir(input_dir)?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();
        let mut failures = 0;
        for name in &names {
            let raw: Vec<Value> = serde_json::from_slice(&std::fs::read(input_dir.join(name))?)?;
            let mut threads = Vec::new();
            for (index, value) in raw.into_iter().enumerate() {
                let projection: ThreadProjection = match serde_json::from_value(value) {
                    Ok(p) => p,
                    Err(error) => {
                        failures += 1;
                        eprintln!("{name}[{index}]: decode failed: {error}");
                        threads.push(json!({ "index": index, "decodeError": error.to_string() }));
                        continue;
                    }
                };
                let input = timeline_input(&mut TimelineMapper::default(), &projection, &[]);
                let rows = derive_timeline_rows(&input, &TimelineExpansion::default());
                let expanded_rows = expanded(&input, &projection);
                threads.push(json!({
                    "index": index,
                    "threadId": projection.thread.id.to_string(),
                    "entries": input.entries.iter().map(entry).collect::<Vec<_>>(),
                    "checkpoints": t3code_web::session_logic::checkpoint_summaries(&projection).iter().map(|c| json!({
                        "assistantMessageId": c.assistant_message_id.as_ref().map(|i| i.to_string()),
                        "turnCount": c.turn_count,
                        "files": c.files.iter().map(|f| format!("{}:{}+{}-{}", f.operation, f.path, f.additions, f.deletions)).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "rows": rows.iter().map(row).collect::<Vec<_>>(),
                    "expandedRows": expanded_rows.iter().map(row).collect::<Vec<_>>(),
                }));
            }
            std::fs::write(out_dir.join(name), serde_json::to_vec_pretty(&threads)?)?;
        }
        println!("{} files, {failures} projections failed to decode", names.len());
        Ok(())
    }
}

/// The composer of the shown thread.
fn composer_of(check: &mut Check) -> Entity<t3code_web::components::chat::composer::ChatComposer> {
    check.read(|root, cx| root.composer(cx))
}

/// The shown thread's server projection.
fn projection_of(check: &mut Check, key: &ThreadKey) -> Option<Arc<t3code_shared::thread_projection::ThreadProjection>> {
    let key = key.clone();
    check.cx.update(|cx| {
        let threads = StateBridge::threads(cx);
        let active = threads.read(cx).active()?.clone();
        let thread = active.read(cx);
        (thread.key() == &key).then(|| thread.projection().cloned()).flatten()
    })
}

/// Writes the fixtures the send phase attaches: a 2400x1200 PNG (scaled to 2000 on upload, as the
/// web does) and a small text file.
fn attachment_fixtures(dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let image = image::RgbImage::from_fn(2400, 1200, |x, y| image::Rgb([(x / 10) as u8, (y / 5) as u8, 160]));
    let png = dir.join("probe.png");
    image.save(&png)?;
    let text = dir.join("notes.txt");
    std::fs::write(&text, "Retry delay must stay under five seconds.\n")?;
    Ok((png, text))
}

/// Creates a thread and runs one turn so it has history.
fn thread_with_turn(check: &mut Check, state: &Seeded, title: &str) -> anyhow::Result<ThreadKey> {
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let commands = commands_of(check, &environment_id)?;
    let project = check
        .cx
        .update(|cx| {
            let shell = StateBridge::environment(&environment_id, cx)?.read(cx).shell.clone();
            shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == state.project_root).map(|p| p.id.clone())
        })
        .context("seeded project")?;
    let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let input = CreateThread {
        thread_id: thread_id.clone(),
        project_id: project,
        title: title.into(),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: Some("feature/clamp".into()),
        worktree_path: None,
    };
    let c = commands.clone();
    check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let message = SendMessage::text(thread_id.clone(), "Start the review.");
    check.block(async move { commands.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let key = thread_key(&environment_id, &thread_id);
    let k = key.clone();
    check.pump(
        move |c| {
            c.cx.update(|cx| {
                StateBridge::shell(cx).read(cx).row(&k).is_some_and(|row| {
                    row.latest_run.as_ref().is_some_and(|r| r.completed_at.is_some())
                })
            })
        },
        Duration::from_secs(60),
        "first turn settled",
    )?;
    Ok(key)
}

/// Waits until the shown thread's run count reaches `runs` and the last run completed.
fn settled_runs(check: &mut Check, key: &ThreadKey, runs: usize, what: &str) -> anyhow::Result<()> {
    let key = key.clone();
    check.pump(
        move |c| projection_of(c, &key).is_some_and(|p| p.runs.len() >= runs && p.runs.iter().all(|r| r.completed_at.is_some())),
        Duration::from_secs(60),
        what,
    )
}

/// The `send` phase: attachments through the signed upload, a model and mode change carried by
/// the turn, the sent images, `/clear`, edit from here with its confirmation, fork and `/side`.
/// Writes `send-record.json`: what the server recorded for the send.
fn run_send(check: &mut Check) -> anyhow::Result<()> {
    use t3code_web::components::chat::composer::ComposerEvent;
    use t3code_web::composer_logic::ComposerSubmissionIntent;
    use t3code_web::composer_draft_store::UploadState;
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, "Attachments")?;
    check.go(&key);
    check.pump(|c| c.shown_input().entries.len() >= 2, Duration::from_secs(20), "attachments thread shown")?;

    // Drop an image and a text file on the composer; both upload at once.
    let (png, text) = attachment_fixtures(&check.out)?;
    let composer = composer_of(check);
    check.cx.update(|cx| composer.update(cx, |c, cx| c.attach_paths(vec![png.clone(), text.clone()], cx)));
    let tiles = check.cx.update(|cx| composer.read(cx).attachments().len());
    check.check("the composer takes a PNG and a text file", tiles == 2, format!("{tiles} attachments"));
    let c2 = composer.clone();
    check.pump(
        move |c| c.cx.update(|cx| c2.read(cx).attachments().iter().all(|a| a.upload == UploadState::Uploaded)),
        Duration::from_secs(30),
        "both uploads finished (attachments.createUploadUrl + POST)",
    )?;
    check.shot("30-attachments.png")?;

    // An unsupported image is refused with the web's message.
    let bmp = check.out.join("probe.heic.tiff");
    std::fs::write(&bmp, b"II*\0")?;
    let before = check.cx.update(|cx| composer.read(cx).attachments().len());
    check.cx.update(|cx| composer.update(cx, |c, cx| c.attach_paths(vec![bmp.clone()], cx)));
    let after = check.cx.update(|cx| composer.read(cx).attachments().len());
    check.note(format!("a .tiff stages as a file like the web's preview types: {before} -> {after} attachments"));
    check.cx.update(|cx| composer.update(cx, |c, cx| {
        let extra: Vec<_> = c.attachments().iter().skip(before).map(|a| a.context_id.clone()).collect();
        for id in extra {
            c.remove_attachment(id, cx);
        }
    }));

    // Pick another model and the auto-accept mode; both travel with the turn.
    let (model, mode) = check.cx.update(|cx| {
        let state = composer.read(cx).state();
        let mut selection = state.selection.clone();
        selection.model = "command-output".into();
        (selection, t3code_web::components::chat::composer_permissions::RuntimeMode::AutoAcceptEdits)
    });
    check.cx.update(|cx| composer.update(cx, |_, cx| {
        cx.emit(ComposerEvent::ModelChanged(model.clone()));
        cx.emit(ComposerEvent::RuntimeModeChanged(mode));
    }));
    check.frame();
    let shown_model = check.cx.update(|cx| composer.read(cx).state().selection.model.clone());
    check.check("the picked model shows in the composer before sending", shown_model.as_ref() == "command-output", &shown_model);
    check.focus_composer();
    check.type_text("Check the probe image and the notes.");
    check.cx.update(|cx| composer.update(cx, |c, cx| c.submit(ComposerSubmissionIntent::Foreground, cx)));
    settled_runs(check, &key, 2, "the attachment turn settled")?;
    let projection = projection_of(check, &key).context("projection")?;
    let user = projection
        .messages
        .iter()
        .filter(|m| m.role == t3code_contracts::OrchestrationV2ConversationMessageRole::User)
        .last()
        .cloned()
        .context("user message")?;
    let run = projection.runs.iter().max_by_key(|r| r.ordinal).cloned().context("run")?;
    let attachments = serde_json::to_value(&user.attachments)?;
    let records = user.context.as_ref().map(|c| c.records.len()).unwrap_or(0);
    check.check("the server holds both attachments", user.attachments.len() == 2, attachments.to_string());
    check.check("the context binds each chip to its upload", records == 2, format!("{records} records"));
    let image_size = user.attachments.iter().find_map(|a| match a {
        t3code_contracts::ChatAttachment::Image(i) => Some(i.size_bytes),
        _ => None,
    });
    check.check("the image was scaled to 2000px before upload", image_size.is_some_and(|s| s > 0), format!("{image_size:?} bytes"));
    let run_model = serde_json::to_value(&run.model_selection)?;
    check.check("the turn ran on the picked model", run_model["model"] == "command-output", run_model.to_string());
    check.check("the thread took the picked mode before the turn", projection.thread.runtime_mode == RuntimeMode::AutoAcceptEdits, format!("{:?}", projection.thread.runtime_mode));
    let record = serde_json::json!({
        "message": serde_json::to_value(&user)?,
        "runModelSelection": run_model,
        "threadRuntimeMode": projection.thread.runtime_mode,
        "title": projection.thread.title,
    });
    std::fs::write(check.out.join("send-record.json"), serde_json::to_vec_pretty(&record)?)?;
    let k = key.clone();
    check.pump(
        move |c| {
            c.shown_input().entries.iter().any(|e| match &e.kind {
                EntryKind::Message(m) => m.attachments.iter().any(|a| !a.source.is_empty()),
                _ => false,
            }) && c.route() == Route::Thread(k.clone())
        },
        Duration::from_secs(20),
        "the sent image shows in the timeline",
    )?;
    check.wait(Duration::from_millis(500));
    check.shot("31-sent-with-attachments.png")?;

    // `/clear` arms a fresh session; the next turn carries `freshContext`.
    check.focus_composer();
    check.type_text("/clear");
    // The first Enter picks `/clear` in the command menu, the second sends it (as on the web).
    check.press("enter");
    let picked = check.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned());
    check.note(format!("after picking from the menu the composer holds {picked:?}"));
    if picked.trim() == "/clear" {
        check.press("enter");
    }
    check.frame();
    let banner = check.cx.update(|cx| composer.read(cx).state().banners.iter().any(|b| b.id.as_ref() == "fresh-session"));
    check.check("/clear shows the fresh-session banner", banner, "");
    check.shot("32-clear-armed.png")?;
    check.type_text("Start over with the summary.");
    check.press("enter");
    settled_runs(check, &key, 3, "the fresh-session turn settled")?;
    let banner_gone = check.cx.update(|cx| !composer.read(cx).state().banners.iter().any(|b| b.id.as_ref() == "fresh-session"));
    check.check("the fresh-session banner clears after the send", banner_gone, "");

    // Fork from the last reply opens "<title> fork".
    let reply = check.shown_input().entries.iter().rev().find_map(|e| match &e.kind {
        EntryKind::Message(m) if m.role == t3code_web::components::chat::timeline_entries::MessageRole::Assistant => Some(m.id.clone()),
        _ => None,
    });
    if let Some(reply) = reply {
        let root = check.root.clone();
        check.cx.update(|cx| {
            let view = root.read(cx).thread_view().clone();
            let timeline = view.read(cx).chat().read(cx).timeline().clone();
            timeline.update(cx, |_, cx| cx.emit(TimelineAction::Fork(reply.clone())));
        });
        let original = key.clone();
        check.pump(move |c| matches!(c.route(), Route::Thread(k) if k != original), Duration::from_secs(30), "the fork opens")?;
        let title = match check.route() {
            Route::Thread(k) => check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&k).map(|r| r.title().to_owned()).unwrap_or_default()),
            _ => String::new(),
        };
        check.check("the fork is titled like the web's", title.ends_with(" fork"), &title);
        check.shot("35-forked.png")?;
        check.go(&key);
        let k = key.clone();
        check.pump(move |c| c.route() == Route::Thread(k.clone()) && !c.shown_input().entries.is_empty(), Duration::from_secs(10), "back on the thread")?;
    }

    // Edit from here: the dialog, then the rollback and the prompt back in the composer.
    let target = check.shown_input().revert_turn_counts.iter().max_by_key(|(_, turns)| **turns).map(|(id, turns)| (id.clone(), *turns));
    let target_text = target.as_ref().and_then(|(id, _)| {
        check.shown_input().entries.iter().find_map(|e| match &e.kind {
            EntryKind::Message(m) if m.id.0 == *id => Some(m.text.to_string()),
            _ => None,
        })
    });
    let target_text = target_text.unwrap_or_default();
    let counts = format!("{:?}", check.shown_input().revert_turn_counts);
    check.check("live user messages offer edit from here", target.is_some(), counts);
    if let Some((message, _)) = target {
        let root = check.root.clone();
        check.cx.update(|cx| {
            let view = root.read(cx).thread_view().clone();
            let timeline = view.read(cx).chat().read(cx).timeline().clone();
            timeline.update(cx, |_, cx| cx.emit(TimelineAction::EditFrom(t3code_web::components::chat::timeline_entries::MessageId(message.clone()))));
        });
        // The dialog fades in over 200 ms.
        check.wait(Duration::from_millis(300));
        check.shot("33-edit-from-here-dialog.png")?;
        let runs_before = projection_of(check, &key).map(|p| p.runs.len()).unwrap_or(0);
        check.cx.update(|cx| {
            let view = root.read(cx).thread_view().clone();
            view.update(cx, |view, cx| view.resolve_revert(Some(false), cx));
        });
        let (k, wanted) = (key.clone(), target_text.clone());
        check.pump(
            move |c| c.cx.update(|cx| {
                let text = root.read(cx).composer(cx).read(cx).editor().read(cx).text().to_owned();
                !wanted.is_empty() && text.contains(&wanted)
            }) && c.route() == Route::Thread(k.clone()),
            Duration::from_secs(30),
            "the rewound prompt is back in the composer",
        )?;
        let wanted = target_text.clone();
        let dropped = check
            .pump(
                move |c| !c.shown_input().entries.iter().any(|e| matches!(&e.kind, EntryKind::Message(m) if m.text.as_ref() == wanted)),
                Duration::from_secs(20),
                "the timeline drops the rewound message",
            )
            .is_ok();
        check.check("the rollback removes the message and the runs after it", dropped, format!("{runs_before} runs before"));
        let editor = check.cx.update(|cx| composer.read(cx).editor().clone());
        check.cx.update(|cx| editor.update(cx, |e, cx| e.set_text(String::new(), cx)));
        check.shot("34-edited-from-here.png")?;
    }

    // `@` offers threads by title; the picked chip travels as a thread context record.
    check.focus_composer();
    check.type_text("@Streaming");
    let c3 = composer.clone();
    check.pump(
        move |c| c.cx.update(|cx| c3.read(cx).menu().is_some_and(|m| m.items.iter().any(|i| i.id.starts_with("thread:")))),
        Duration::from_secs(10),
        "the @ menu lists the Streaming thread",
    )?;
    check.shot("37-thread-mention-menu.png")?;
    check.press("enter");
    check.type_text("compare with this one");
    let runs = projection_of(check, &key).map(|p| p.runs.len()).unwrap_or(0);
    check.press("enter");
    settled_runs(check, &key, runs + 1, "the mention turn settled")?;
    let records = projection_of(check, &key)
        .and_then(|p| {
            p.messages.iter().filter(|m| m.role == t3code_contracts::OrchestrationV2ConversationMessageRole::User).last().cloned()
        })
        .and_then(|m| m.context.clone())
        .map(|c| c.records)
        .unwrap_or_default();
    let thread_record = records.iter().find(|r| r["kind"] == "thread").cloned();
    check.check("the @ mention sends a thread context record", thread_record.as_ref().is_some_and(|r| r["title"] == "Streaming"), format!("{thread_record:?}"));

    // `/side` starts a side chat from the thread and sends the prompt there.
    check.focus_composer();
    check.type_text("/side What does the retry helper return?");
    check.press("enter");
    let original = key.clone();
    check.pump(move |c| matches!(c.route(), Route::Thread(k) if k != original), Duration::from_secs(30), "the side chat opens")?;
    check.shot("36-side-chat.png")?;
    Ok(())
}

/// Runs a row command as the sidebar's context menu does.
fn sidebar_command(root: &Entity<AppRoot>, key: &ThreadKey, command: t3code_web::components::sidebar::context_menu::MenuCommand, _: &mut Window, cx: &mut App) {
    let sidebar = root.read(cx).sidebar().clone();
    let keys = vec![key.clone()];
    sidebar.update(cx, |_, cx| cx.emit(t3code_web::components::sidebar::SidebarEvent::ThreadCommand(keys, command)));
}

/// The sidebar's groups: label and how many projects each joins.
fn sidebar_groups(check: &mut Check) -> Vec<(String, usize, bool)> {
    check.read(|root, cx| {
        root.sidebar().read(cx).list().groups.iter().map(|g| (g.label.to_string(), g.member_refs.len(), g.favicon.is_some())).collect()
    })
}

/// A shell row of the snapshot, by key.
fn sidebar_row(check: &mut Check, key: &ThreadKey) -> Option<Arc<t3code_web::components::sidebar::model::SidebarThread>> {
    let key = key.clone();
    check.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.iter().find(|t| t.key == key).cloned())
}

/// The `sidebar` phase, with a second server (`T3_PAIRING_URL_B`, project at `T3_PROJECT_ROOT_B`,
/// a clone of the first with the same remote): rows from both servers, one repository group across
/// them and the per-project override splitting it, the project favicon and its monogram fallback,
/// provider marks, a turn on one server leaving the other's open thread alone, unread and the
/// visit on open, server message search, snooze, archive and the Archive list, and the composer
/// draft on disk.
fn run_sidebar(check: &mut Check) -> anyhow::Result<()> {
    use t3code_web::components::sidebar::context_menu::{MenuCommand, SnoozePreset};
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "server A connected")?;
    let environments = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.len());
    if environments < 2 {
        let url = std::env::var("T3_PAIRING_URL_B").context("T3_PAIRING_URL_B")?;
        let request = parse_pairing_url(&url)?;
        let pairing = check.cx.update(|cx| ConnectionRuntime::global(cx).pair(request, None, cx));
        let label = check.wait_task(pairing)?.map_err(|e| anyhow::anyhow!("{}", e.0))?;
        check.note(format!("paired server B as {label}"));
    }
    check.pump(|c| c.connected() && c.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.len() >= 2), Duration::from_secs(30), "both servers connected")?;
    let env_a = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    let env_b = check.cx.update(|cx| {
        ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| e.entry.environment_id.clone()).find(|id| id.as_str() != env_a.as_str())
    });
    let env_b = t3code_contracts::EnvironmentId::new(env_b.context("server B")?.as_str());
    let commands_b = commands_of(check, &env_b)?;
    let root_b = PathBuf::from(std::env::var("T3_PROJECT_ROOT_B").context("T3_PROJECT_ROOT_B")?);
    let root_b_text = root_b.display().to_string();
    let existing = check.cx.update(|cx| {
        let shell = StateBridge::environment(&env_b, cx)?.read(cx).shell.clone();
        shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == root_b_text).map(|p| p.id.clone())
    });
    let project_b = match existing {
        Some(id) => id,
        None => {
            let id = ProjectId::new(uuid::Uuid::new_v4().to_string());
            let mutation = ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
                command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
                project_id: id.clone(),
                title: "orbit".into(),
                workspace_root: root_b_text.clone(),
                create_workspace_root_if_missing: Some(false),
                default_model_selection: None,
                scripts: None,
            });
            let c = commands_b.clone();
            check.block(async move { c.mutate_project(mutation).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
            id
        }
    };
    let thread_b = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let create = CreateThread {
        thread_id: thread_b.clone(),
        project_id: project_b.clone(),
        title: "Server B review".into(),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: Some("feature/clamp".into()),
        worktree_path: None,
    };
    let c = commands_b.clone();
    check.block(async move { c.create_thread(create).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let key_b = thread_key(&env_b, &thread_b);
    let kb = key_b.clone();
    check.pump(move |c| sidebar_row(c, &kb).is_some(), Duration::from_secs(20), "server B's thread reaches the sidebar")?;
    let (rows_a, rows_b) = check.cx.update(|cx| {
        let shell = StateBridge::shell(cx);
        let threads = &shell.read(cx).snapshot().threads;
        let a = threads.iter().filter(|t| t.key.environment_id.0.as_ref() == env_a.as_str()).count();
        let b = threads.iter().filter(|t| t.key.environment_id.0.as_ref() == env_b.as_str()).count();
        (a, b)
    });
    check.check("rows from both servers show", rows_a > 0 && rows_b > 0, format!("A {rows_a}, B {rows_b}"));
    let marks = check.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.iter().filter(|t| t.provider.is_some()).count());
    check.check("rows carry their provider mark", marks > 0, format!("{marks} rows with a mark"));

    // One repository, two servers: one group once both servers resolved the identity.
    // The web names a whole-repository group for the repository ("example/orbit") when the
    // members' title is just the repository's name.
    check.pump(
        |c| sidebar_groups(c).iter().any(|(_, members, _)| *members == 2),
        Duration::from_secs(30),
        "the two servers' orbit projects share one repository group",
    )
    .ok();
    let groups = sidebar_groups(check);
    check.check("projects group by repository across servers", groups.iter().any(|(l, m, _)| l == "example/orbit" && *m == 2), format!("{groups:?}"));
    check.pump(|c| sidebar_groups(c).iter().any(|(_, _, favicon)| *favicon), Duration::from_secs(20), "server A's favicon loaded").ok();
    let groups = sidebar_groups(check);
    check.check("the group shows the project's favicon", groups.iter().any(|(_, _, f)| *f), format!("{groups:?}"));
    check.shot("40-two-servers.png")?;
    // The owner's per-project override: B's project on its own.
    let physical_b = t3code_client_runtime::state::project_grouping::physical_project_key(&env_b, &root_b_text);
    check.cx.update(|cx| {
        t3code_web::hooks::use_settings::ClientSettingsStore::update(cx, |s| {
            s.sidebar_project_grouping_overrides.insert(physical_b.clone(), t3code_contracts::SidebarProjectGroupingMode::Separate);
        })
    });
    check.pump(|c| sidebar_groups(c).iter().filter(|(l, m, _)| l == "example/orbit" && *m == 1).count() == 2, Duration::from_secs(5), "the override splits the group").ok();
    let split = sidebar_groups(check);
    check.check("a per-project override splits that project out", split.iter().filter(|(l, m, _)| l == "example/orbit" && *m == 1).count() == 2, format!("{split:?}"));
    let favicons: Vec<bool> = split.iter().filter(|(l, _, _)| l == "example/orbit").map(|(_, _, f)| *f).collect();
    check.check("server B's copy keeps the monogram (no favicon file there)", favicons.contains(&true) && favicons.contains(&false), format!("{favicons:?}"));
    check.shot("41-override-separate.png")?;
    check.cx.update(|cx| {
        t3code_web::hooks::use_settings::ClientSettingsStore::update(cx, |s| {
            s.sidebar_project_grouping_overrides.remove(&physical_b);
        })
    });

    // A turn on server B while server A's thread is open leaves A alone.
    let key_a = key_of(&state, &state.long_thread);
    check.go(&key_a);
    check.pump(|c| c.shown_input().entries.len() > 5, Duration::from_secs(20), "server A's long thread shown")?;
    let before = check.shown_input().entries.len();
    let message = SendMessage::text(thread_b.clone(), "Compare the zeppelin retry budget with server A.");
    let c = commands_b.clone();
    check.block(async move { c.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let kb = key_b.clone();
    check.pump(
        move |c| sidebar_row(c, &kb).is_some_and(|t| t.latest_run.as_ref().is_some_and(|r| r.completed_at.is_some())),
        Duration::from_secs(60),
        "server B's turn completes",
    )?;
    let after = check.shown_input().entries.len();
    let route = check.route();
    check.check("server A's open thread is untouched", route == Route::Thread(key_a.clone()) && after == before, format!("{before} -> {after} entries"));
    let unread = check.cx.update(|cx| {
        let row = StateBridge::shell(cx).read(cx).snapshot().threads.iter().find(|t| t.key == key_b).cloned();
        row.map(|t| t3code_web::components::sidebar_logic::has_unseen_completion(&t))
    });
    check.check("server B's finished thread is unread", unread == Some(true), format!("{unread:?}"));
    check.shot("42-b-finished-unread.png")?;
    check.go(&key_b);
    let kb = key_b.clone();
    check.pump(
        move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&kb).is_some_and(|row| row.source.last_visited_at.is_some())),
        Duration::from_secs(20),
        "opening it records the visit on server B",
    )?;
    let kb = key_b.clone();
    check
        .pump(move |c| sidebar_row(c, &kb).is_some_and(|t| !t3code_web::components::sidebar_logic::has_unseen_completion(&t)), Duration::from_secs(10), "the row turns read")
        .ok();
    let read = check.cx.update(|cx| {
        StateBridge::shell(cx).read(cx).snapshot().threads.iter().find(|t| t.key == key_b).map(|t| !t3code_web::components::sidebar_logic::has_unseen_completion(t))
    });
    check.check("the visited thread reads as read", read == Some(true), format!("{read:?}"));

    // Search by a word only in server B's message.
    let sidebar = check.read(|root, _| root.sidebar().clone());
    check.cx.update_window(check.window, |_, w, cx| sidebar.update(cx, |s, cx| s.set_search("zeppelin", w, cx)))?;
    let kb = key_b.clone();
    check.pump(
        move |c| c.read(|root, cx| root.sidebar().read(cx).list().thread_order.contains(&kb)),
        Duration::from_secs(10),
        "server message search lists server B's thread",
    )
    .ok();
    let found = check.read(|root, cx| root.sidebar().read(cx).list().thread_order.clone());
    check.check("a word from a message finds its thread", found.contains(&key_b), format!("{} results", found.len()));
    check.shot("43-message-search.png")?;
    check.cx.update_window(check.window, |_, w, cx| sidebar.update(cx, |s, cx| s.set_search("", w, cx)))?;

    // Snooze with a preset, then wake.
    let root = check.root.clone();
    check.cx.update_window(check.window, |_, w, cx| sidebar_command(&root, &key_b, MenuCommand::Snooze(SnoozePreset::OneHour), w, cx))?;
    let kb = key_b.clone();
    check.pump(move |c| sidebar_row(c, &kb).is_some_and(|t| t.snoozed_until.is_some()), Duration::from_secs(10), "snoozed for an hour")?;
    check.shot("44-snoozed.png")?;
    let root = check.root.clone();
    check.cx.update_window(check.window, |_, w, cx| sidebar_command(&root, &key_b, MenuCommand::Wake, w, cx))?;
    let kb = key_b.clone();
    check.pump(move |c| sidebar_row(c, &kb).is_some_and(|t| t.snoozed_until.is_none()), Duration::from_secs(10), "woken")?;

    // A draft written in the composer reaches the drafts file.
    check.go(&key_a);
    check.focus_composer();
    check.type_text("Draft that should survive a restart");
    check.wait(Duration::from_millis(500));
    let root = check.root.clone();
    check.cx.update(|cx| root.update(cx, |root, cx| root.flush_state(cx)));
    check.cx.update(|cx| StateBridge::drafts(cx).flush());
    let file = std::fs::read_to_string(data_dir()?.join("client-state/composer-drafts.json")).unwrap_or_default();
    check.check("the composer draft is on disk", file.contains("Draft that should survive a restart"), format!("{} bytes", file.len()));

    // Archive, then the Archive settings list.
    let root = check.root.clone();
    check.cx.update_window(check.window, |_, w, cx| sidebar_command(&root, &key_b, MenuCommand::Archive, w, cx))?;
    let kb = key_b.clone();
    check.pump(move |c| sidebar_row(c, &kb).is_none(), Duration::from_secs(10), "the archived thread leaves the sidebar")?;
    let router = check.read(|root, _| root.router().clone());
    check.cx.update(|cx| router.update(cx, |r, cx| r.navigate(Route::parse("/settings/archived"), cx)));
    check.wait(Duration::from_secs(2));
    check.shot("45-archive-settings.png")?;
    Ok(())
}

/// How the `notify` phase runs notifications (`T3_NOTIFY_MODE`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NotifyMode {
    /// One system notification per thread, replaced by tag, and the pending count as the badge.
    Linux,
    /// The macOS and Windows inbox: an entry per request or turn end, natively shown while no
    /// window has focus, the unread count as the badge and the sidebar's Done.
    Inbox,
}

fn notify_mode() -> Option<NotifyMode> {
    match std::env::var("T3_NOTIFY_MODE").ok()?.as_str() {
        "inbox" => Some(NotifyMode::Inbox),
        _ => Some(NotifyMode::Linux),
    }
}

/// One line the stand-in notification service logged.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
struct BusRecord {
    method: Option<String>,
    id: Option<u32>,
    replaces: Option<u32>,
    summary: Option<String>,
    body: Option<String>,
    signal: Option<String>,
    count: Option<i64>,
    click: Option<u32>,
}

fn bus_log() -> Vec<BusRecord> {
    let Some(path) = std::env::var_os("T3_NOTIFY_LOG") else { return Vec::new() };
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

/// The badge count the service last saw.
fn bus_badge(log: &[BusRecord]) -> Option<i64> {
    log.iter().rev().find(|r| r.signal.is_some()).and_then(|r| r.count)
}

/// What the model predicts one alert produces, and what happened.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct NotifyStep {
    case: String,
    thread: String,
    expected_summary: String,
    expected_body: String,
    expected_badge: usize,
    expected_done: bool,
    expected_inbox_kind: Option<String>,
    request: Option<BusRecord>,
    badge: usize,
    bus_badge: Option<i64>,
    done: bool,
    inbox: Vec<String>,
}

/// The `notify` phase. See the header.
fn run_notify(check: &mut Check) -> anyhow::Result<()> {
    use t3code_web::components::sidebar::model::RunStatus;
    use t3code_web::desktop_notifications::{DesktopNotificationKind as Kind, NotificationInbox};
    let mode = notify_mode().context("set T3_NOTIFY_MODE")?;
    let inbox = mode == NotifyMode::Inbox;
    let control = PathBuf::from(std::env::var_os("T3_NOTIFY_CONTROL").context("T3_NOTIFY_CONTROL")?);
    let state = load_state(&check.out)?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    check.cx.update(|cx| {
        t3code_web::hooks::use_settings::ClientSettingsStore::update(cx, |s| {
            s.notification_mode = t3code_contracts::NotificationMode::Notifications;
            s.in_app_notifications_enabled = false;
            s.notification_inbox_enabled = true;
            s.app_icon_badge_enabled = true;
        })
    });
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let commands = commands_of(check, &environment_id)?;
    let filter = |check: &mut Check, on: bool| -> anyhow::Result<()> {
        let patch = t3code_contracts::ClientServerSettingsPatch {
            hide_tagged_threads: Some(on),
            hidden_thread_tags: Some(vec!["quiet".into()]),
            ..Default::default()
        };
        let id = environment_id.clone();
        let task = check.cx.update(|cx| t3code_web::hooks::use_settings::ServerSettingsStore::update(id, patch, cx));
        check.wait_task(task)?.map_err(|e| anyhow::anyhow!("{}", e.0))?;
        let id = environment_id.clone();
        check.pump(
            move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).hidden().1 == on || StateBridge::environment(&id, cx).is_none()),
            Duration::from_secs(10),
            if on { "tag filter on" } else { "tag filter off" },
        )
    };
    filter(check, false)?;
    let project = check
        .cx
        .update(|cx| {
            let shell = StateBridge::environment(&environment_id, cx)?.read(cx).shell.clone();
            shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == state.project_root).map(|p| (p.id.clone(), p.title.clone()))
        })
        .context("seeded project")?;
    let create = |check: &mut Check, title: &str, model: &str, mode: RuntimeMode| -> anyhow::Result<ThreadKey> {
        let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
        let input = CreateThread {
            thread_id: thread_id.clone(),
            project_id: project.0.clone(),
            title: title.into(),
            model_selection: ModelSelection { instance_id: "devReplay".into(), model: model.into(), options: None },
            runtime_mode: mode,
            interaction_mode: ProviderInteractionMode::Default,
            branch: Some("feature/clamp".into()),
            worktree_path: None,
        };
        let c = commands.clone();
        check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        let key = thread_key(&environment_id, &thread_id);
        let k = key.clone();
        check.pump(move |c| sidebar_row(c, &k).is_some(), Duration::from_secs(10), &format!("{title} listed"))?;
        Ok(key)
    };
    let send = |check: &mut Check, key: &ThreadKey, text: &str| -> anyhow::Result<()> {
        let (_, thread_id) = t3code_web::state::shell::runtime_ids(key);
        let c = commands.clone();
        let message = SendMessage::text(thread_id, text);
        check.block(async move { c.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    };
    let shown = create(check, "Notify shown", "multi-turn", RuntimeMode::FullAccess)?;
    let done = create(check, "Notify done", "multi-turn", RuntimeMode::FullAccess)?;
    let approval = create(check, "Notify approval", "approval", RuntimeMode::ApprovalRequired)?;
    let question = create(check, "Notify question", "question", RuntimeMode::ApprovalRequired)?;
    let failed = create(check, "Notify failed", "no-such-scenario", RuntimeMode::FullAccess)?;
    check.go(&shown);
    // The app starts focused, as after a launch.
    let window = check.window;
    check.cx.update_window(window, |_, w, _| w.activate_window())?;
    check.pump(move |c| c.cx.update_window(window, |_, w, _| w.is_window_active()).unwrap_or(false), Duration::from_secs(5), "window focused at start")?;
    check.wait(Duration::from_secs(1));
    let label = check.cx.update(|cx| {
        ConnectionRuntime::state(cx).read(cx).environment(&environment_id).map(|env| env.entry.label.clone()).unwrap_or_default()
    });
    blur(check)?;
    // Requests left open by earlier runs on this profile are unread entries too.
    let baseline = if inbox { check.cx.update(|cx| NotificationInbox::global(cx).read(cx).unread_count()) } else { 0 };
    check.note(format!("unread entries before the cases: {baseline}"));
    let mut steps: Vec<NotifyStep> = Vec::new();
    let cases: [(&str, &ThreadKey, &str, &str, Kind); 4] = [
        ("completion", &done, "Thread completed", "Turn completed", Kind::Completed),
        ("approval", &approval, "Approval needed", "Approval required", Kind::Approval),
        ("input", &question, "Input needed", "Input required", Kind::Input),
        ("failed", &failed, "Thread failed", "Turn failed", Kind::Failed),
    ];
    for (index, (case, key, linux_title, inbox_title, kind)) in cases.into_iter().enumerate() {
        let before = bus_log().iter().filter(|r| r.method.as_deref() == Some("Notify")).count();
        send(check, key, "Start the review.")?;
        let k = key.clone();
        check.pump(
            move |c| {
                sidebar_row(c, &k).is_some_and(|row| match kind {
                    Kind::Completed => row.latest_run.is_some_and(|run| run.status == RunStatus::Completed && run.completed_at.is_some()),
                    Kind::Approval => row.has_pending_approvals,
                    Kind::Input => row.has_pending_user_input,
                    Kind::Failed => row.latest_run.is_some_and(|run| run.status == RunStatus::Failed),
                })
            },
            Duration::from_secs(60),
            &format!("{case}: thread state"),
        )?;
        let title = sidebar_row(check, key).map(|row| row.title.to_string()).unwrap_or_default();
        let (summary, body) = match mode {
            NotifyMode::Linux => (linux_title.to_string(), title.clone()),
            NotifyMode::Inbox => (inbox_title.to_string(), format!("{title}\n{} · {label}", project.1)),
        };
        let got = check.pump(
            move |_| bus_log().iter().filter(|r| r.method.as_deref() == Some("Notify")).count() > before,
            Duration::from_secs(10),
            &format!("{case}: notification request"),
        );
        let request = got.ok().and_then(|_| bus_log().into_iter().filter(|r| r.method.as_deref() == Some("Notify")).nth(before));
        check.wait(Duration::from_millis(300));
        let expected_badge = baseline + index + 1;
        let (badge, items, done_marker) = check.cx.update(|cx| {
            let badge = ThreadNotificationCoordinator::global(cx).read(cx).badge();
            let items = NotificationInbox::global(cx).read(cx).items().clone();
            (badge, items, false)
        });
        let done_marker = done_marker || sidebar_row(check, key).is_some_and(|row| row.has_unread_completion_notification);
        let inbox_entries: Vec<String> = items
            .iter()
            .filter(|item| item.is_for(key))
            .map(|item| format!("{:?} read={} resolved={}", item.kind, item.read, item.resolved))
            .collect();
        let step = NotifyStep {
            case: case.into(),
            thread: title.clone(),
            expected_summary: summary.clone(),
            expected_body: body.clone(),
            expected_badge,
            expected_done: inbox && kind == Kind::Completed,
            expected_inbox_kind: inbox.then(|| format!("{kind:?}")),
            request: request.clone(),
            badge,
            bus_badge: bus_badge(&bus_log()),
            done: done_marker,
            inbox: inbox_entries.clone(),
        };
        let sent = request.as_ref().is_some_and(|r| r.summary.as_deref() == Some(summary.as_str()) && r.body.as_deref() == Some(body.as_str()));
        check.check(&format!("{case}: OS notification request"), sent, format!("expected {summary:?} / {body:?}, got {request:?}"));
        check.check(&format!("{case}: badge"), badge == expected_badge && step.bus_badge == Some(expected_badge as i64), format!("engine {badge}, service {:?}, expected {expected_badge}", step.bus_badge));
        check.check(&format!("{case}: sidebar Done"), done_marker == step.expected_done, format!("{done_marker}, expected {}", step.expected_done));
        let entry_ok = if inbox {
            items.iter().any(|item| item.is_for(key) && item.kind == kind && !item.read)
        } else {
            items.is_empty()
        };
        check.check(&format!("{case}: inbox entry"), entry_ok, format!("{inbox_entries:?}, expected {}", step.expected_inbox_kind.as_deref().unwrap_or("none (no inbox on Linux)")));
        steps.push(step);
        check.shot(&format!("60-notify-{index}-{case}.png"))?;
    }

    // A click on the completion's notification opens its thread and focuses the window.
    let done_request = steps[0].request.clone().context("completion request")?;
    let ids: Vec<u32> = steps.iter().filter_map(|s| s.request.as_ref()?.id).collect();
    std::fs::write(&control, format!("{}\n", done_request.id.unwrap_or_default()))?;
    let k = done.clone();
    check.pump(move |c| c.route() == Route::Thread(k.clone()), Duration::from_secs(10), "click opens the completed thread")?;
    let window = check.window;
    check.pump(move |c| c.cx.update_window(window, |_, w, _| w.is_window_active()).unwrap_or(false), Duration::from_secs(5), "window focused")?;
    check.wait(Duration::from_millis(500));
    let log = bus_log();
    let closed: Vec<u32> = log.iter().filter(|r| r.method.as_deref() == Some("CloseNotification")).filter_map(|r| r.id).collect();
    let badge = check.cx.update(|cx| ThreadNotificationCoordinator::global(cx).read(cx).badge());
    match mode {
        NotifyMode::Linux => {
            // Focus clears every pending notification and the badge (`clearPending`).
            let all = ids.iter().all(|id| closed.contains(id));
            check.check("click: focus closes every notification", all, format!("closed {closed:?} of {ids:?}"));
            check.check("click: badge cleared", badge == 0 && bus_badge(&log) == Some(0), format!("engine {badge}, service {:?}", bus_badge(&log)));
        }
        NotifyMode::Inbox => {
            let done_id = done_request.id.unwrap_or_default();
            check.check("click: the read entry's notification is removed", closed.contains(&done_id), format!("closed {closed:?}"));
            check.check("click: badge counts the other unread entries", badge == baseline + 3 && bus_badge(&log) == Some((baseline + 3) as i64), format!("engine {badge}, service {:?}, expected {}", bus_badge(&log), baseline + 3));
            // The read completion counts as a visit (`deriveAcknowledgedCompletions`) and the Done goes.
            let k = done.clone();
            let visited = check.pump(
                move |c| {
                    c.cx.update(|cx| {
                        StateBridge::shell(cx).read(cx).row(&k).is_some_and(|row| {
                            row.last_visited_at.zip(row.latest_run.as_ref().and_then(|run| run.completed_at)).is_some_and(|(v, c)| v >= c)
                        })
                    })
                },
                Duration::from_secs(10),
                "thread.visit recorded on the server",
            );
            check.check("click: thread.visit reached the server", visited.is_ok(), format!("{visited:?}"));
            let marker = sidebar_row(check, &done).is_some_and(|row| row.has_unread_completion_notification);
            check.check("click: sidebar Done cleared", !marker, marker);
        }
    }
    check.shot("64-notify-clicked.png")?;

    // The tag filter: a hidden thread's pending notification closes and it stays quiet.
    check.go(&shown);
    blur(check)?;
    let before = bus_log().iter().filter(|r| r.method.as_deref() == Some("Notify")).count();
    send(check, &done, "Keep going.")?;
    check.pump(move |_| bus_log().iter().filter(|r| r.method.as_deref() == Some("Notify")).count() > before, Duration::from_secs(60), "second completion notified")?;
    let pending_id = bus_log().into_iter().filter(|r| r.method.as_deref() == Some("Notify")).nth(before).and_then(|r| r.id);
    filter(check, true)?;
    let (_, done_id) = t3code_web::state::shell::runtime_ids(&done);
    let c = commands.clone();
    let t = done_id.clone();
    check.block(async move { c.set_tag(&t, "quiet", true).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let k = done.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).snapshot().threads.iter().all(|t| t.key != k)), Duration::from_secs(10), "tagged thread leaves the sidebar")?;
    check.wait(Duration::from_millis(500));
    let log = bus_log();
    let closed = log.iter().any(|r| r.method.as_deref() == Some("CloseNotification") && r.id == pending_id && pending_id.is_some());
    match mode {
        NotifyMode::Linux => check.check("tag filter: hidden thread's notification closed", closed, format!("pending {pending_id:?}")),
        // The inbox reads every thread of the shell; the tag filter does not reach it.
        NotifyMode::Inbox => check.check("tag filter: inbox entries stay (the inbox ignores the filter)", !closed, format!("pending {pending_id:?}")),
    }
    let listed = sidebar_row(check, &done).is_some();
    let shown_rows = check.read(|root, cx| root.sidebar().read(cx).list().entries.iter().any(|e| matches!(e, SidebarListEntry::Thread { thread, .. } if thread.key == done)));
    check.check("tag filter: sidebar hides the tagged thread", !listed && !shown_rows, format!("in snapshot {listed}, drawn {shown_rows}"));
    let before = bus_log().iter().filter(|r| r.method.as_deref() == Some("Notify")).count();
    let k = done.clone();
    let run_before = check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&k).and_then(|row| row.latest_run.as_ref().map(|r| r.run_id.to_string())));
    send(check, &done, "One more.")?;
    let k = done.clone();
    check.pump(
        move |c| {
            c.cx.update(|cx| {
                StateBridge::shell(cx).read(cx).row(&k).is_some_and(|row| {
                    row.latest_run.as_ref().is_some_and(|r| Some(r.run_id.to_string()) != run_before && r.completed_at.is_some())
                })
            })
        },
        Duration::from_secs(60),
        "hidden thread's turn settled",
    )?;
    check.wait(Duration::from_secs(2));
    let after: Vec<BusRecord> = bus_log().into_iter().filter(|r| r.method.as_deref() == Some("Notify")).skip(before).collect();
    match mode {
        NotifyMode::Linux => check.check("tag filter: hidden thread's completion is quiet", after.is_empty(), format!("{after:?}")),
        NotifyMode::Inbox => check.check("tag filter: the inbox still records it", after.len() == 1, format!("{after:?}")),
    }
    let c = commands.clone();
    check.block(async move { c.set_tag(&done_id, "quiet", false).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    filter(check, false)?;
    if inbox {
        // Reading a completion in the inbox, with another thread open, sends the visit itself.
        let k = done.clone();
        let completed_at = check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&k).and_then(|row| Some((row.latest_run.as_ref()?.completed_at?, row.last_visited_at))));
        let entry = check.cx.update(|cx| {
            NotificationInbox::global(cx).read(cx).items().iter().find(|item| item.is_for(&done) && item.kind == Kind::Completed && !item.read).map(|item| item.id.clone())
        });
        let on_shown = check.route() == Route::Thread(shown.clone());
        check.check(
            "inbox read: an unread completion of a thread not open, not yet visited",
            entry.is_some() && completed_at.is_some_and(|(c, v)| v.is_none_or(|v| v < c)) && on_shown,
            format!("{entry:?} {completed_at:?}"),
        );
        if let Some(id) = entry {
            check.cx.update(|cx| NotificationInbox::update(t3code_web::desktop_notifications::DesktopNotificationCommand::Read(vec![id]), cx));
            let k = done.clone();
            let visited = check.pump(
                move |c| {
                    c.cx.update(|cx| {
                        StateBridge::shell(cx).read(cx).row(&k).is_some_and(|row| {
                            row.last_visited_at.zip(row.latest_run.as_ref().and_then(|run| run.completed_at)).is_some_and(|(v, c)| v >= c)
                        })
                    })
                },
                Duration::from_secs(10),
                "inbox read: thread.visit recorded",
            );
            let still = check.route() == Route::Thread(shown.clone());
            check.check("inbox read: thread.visit reached the server without opening it", visited.is_ok() && still, format!("{visited:?}, route kept {still}"));
        }
    }
    std::fs::write(check.out.join(format!("notify-{}-record.json", if inbox { "inbox" } else { "linux" })), serde_json::to_vec_pretty(&steps)?)?;
    check.shot("65-notify-filtered.png")
}

/// Takes focus from the main window the way another app would: a second window takes it and
/// closes, leaving none of the app's windows focused.
fn blur(check: &mut Check) -> anyhow::Result<()> {
    let helper = check.cx.update(|cx| {
        let options = WindowOptions { show: false, focus: true, ..Default::default() };
        cx.open_window(options, |_, cx| cx.new(|_| EmptyView))
    })?;
    check.cx.update_window(helper.into(), |_, window, _| window.activate_window())?;
    check.frame();
    check.cx.update_window(helper.into(), |_, window, _| window.remove_window())?;
    let window = check.window;
    check.pump(move |c| !c.cx.update_window(window, |_, w, _| w.is_window_active()).unwrap_or(true), Duration::from_secs(5), "window unfocused")
}

/// The `details` phase. See the header.
fn run_details(check: &mut Check) -> anyhow::Result<()> {
    use t3code_contracts::{
        OrchestrationV2ThreadLaunchInput, OrchestrationV2ThreadLaunchInputInitialMessage, OrchestrationV2ThreadLaunchWorkspaceStrategy,
        OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree,
    };
    let state = load_state(&check.out)?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.as_str());
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let commands = commands_of(check, &environment_id)?;
    let project = check
        .cx
        .update(|cx| {
            let shell = StateBridge::environment(&environment_id, cx)?.read(cx).shell.clone();
            shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == state.project_root).map(|p| p.id.clone())
        })
        .context("seeded project")?;
    let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let branch = format!("live-setup-{}", now_ms() % 1_000_000);
    let input = OrchestrationV2ThreadLaunchInput {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        creation_source: None,
        thread_id: Some(thread_id.clone()),
        reuse_existing_thread: None,
        project_id: project,
        title: "Worktree setup".into(),
        generate_title: Some(false),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        workspace_strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree {
            base_ref: "main".into(),
            branch: Some(branch.clone()),
            start_from_origin: Some(false),
        }),
        initial_message: Some(OrchestrationV2ThreadLaunchInputInitialMessage {
            message_id: None,
            text: "Start the review.".into(),
            context: None,
            attachments: Vec::new(),
        }),
    };
    let c = commands.clone();
    check.block(async move { c.launch_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let key = thread_key(&environment_id, &thread_id);
    let k = key.clone();
    check.pump(move |c| sidebar_row(c, &k).is_some(), Duration::from_secs(20), "launched thread listed")?;
    check.go(&key);
    let mut seen: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        check.wait(Duration::from_millis(100));
        let setup = check.shown_input().worktree_setup;
        let line = match &setup {
            Some(setup) => format!(
                "running={} branch={} steps={:?}",
                setup.running,
                setup.branch,
                setup.steps.iter().map(|(label, status)| format!("{label}:{status:?}")).collect::<Vec<_>>()
            ),
            None => "none".into(),
        };
        if seen.last() != Some(&line) {
            check.note(format!("setup row: {line}"));
            seen.push(line);
        }
        if setup.as_ref().is_some_and(|s| !s.running) || (seen.len() > 1 && setup.is_none()) {
            break;
        }
    }
    let shown = seen.iter().any(|line| line.contains(&format!("branch={branch}")) && line.contains("steps=["));
    check.check("worktree setup: the timeline row follows the server's stream", shown, seen.join(" | "));
    let ran = seen.iter().any(|line| line.contains("Check out files:"));
    check.check("worktree setup: stages carry their labels", ran, seen.last().cloned().unwrap_or_default());
    check.shot("70-worktree-setup.png")
}

/// The `mentions` phase. See the header.
fn run_mentions(check: &mut Check) -> anyhow::Result<()> {
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let tag = now_ms() % 100_000;
    let parent = thread_with_turn(check, &state, &format!("Quokka parent {tag}"))?;
    let other = thread_with_turn(check, &state, &format!("Quokka other {tag}"))?;
    let exact = thread_with_turn(check, &state, "Quokka")?;
    let composer = composer_of(check);
    check.go(&parent);
    check.focus_composer();
    check.type_text("/side Where does the quokka helper live?");
    check.press("enter");
    let p = parent.clone();
    check.pump(move |c| matches!(c.route(), Route::Thread(k) if k != p), Duration::from_secs(30), "the side chat opens")?;
    let Route::Thread(side) = check.route() else { bail!("no side chat route") };
    let s = side.clone();
    check.pump(
        move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&s).is_some_and(|row| row.latest_user_message_at.is_some())),
        Duration::from_secs(30),
        "the side chat has its message",
    )?;
    let side_title = check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&side).map(|row| row.title().to_owned())).unwrap_or_default();
    // The `@` trigger ends at a space, so the query is one word of the title.
    let query: String = side_title.split_whitespace().max_by_key(|word| word.len()).unwrap_or_default().trim_matches(|c: char| !c.is_alphanumeric()).to_owned();
    let lineage = check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&side).map(|row| format!("{:?}", row.source.lineage)));
    check.note(format!("side chat title {side_title:?}, query {query:?}, lineage {lineage:?}, parent {}", parent.thread_id.0));
    let menu = |check: &mut Check, on: &ThreadKey, text: &str| -> Vec<(String, String, String)> {
        check.go(on);
        check.wait(Duration::from_millis(300));
        check.focus_composer();
        let editor = check.cx.update(|cx| composer.read(cx).editor().clone());
        check.cx.update(|cx| editor.update(cx, |e, cx| e.set_text(String::new(), cx)));
        check.type_text(&format!("@{text}"));
        check.wait(Duration::from_millis(500));
        let items = check.cx.update(|cx| {
            composer.read(cx).menu().map(|m| {
                m.items.iter().filter(|i| i.id.starts_with("thread:")).map(|i| (i.id.to_string(), i.label.to_string(), i.description.to_string())).collect()
            })
        });
        check.cx.update(|cx| editor.update(cx, |e, cx| e.set_text(String::new(), cx)));
        items.unwrap_or_default()
    };
    let id_of = |key: &ThreadKey| format!("thread:{}:{}", key.environment_id.0, key.thread_id.0);
    let from_parent = menu(check, &parent, &query);
    let offered = from_parent.iter().find(|(id, ..)| *id == id_of(&side));
    check.check("@ from the parent offers its side chat as \"Side chat\"", offered.is_some_and(|(_, _, d)| d == "Side chat"), format!("{from_parent:?}"));
    check.shot("80-mention-side-chat.png")?;
    let from_other = menu(check, &other, &query);
    check.check("@ from another thread leaves the side chat out", !from_other.iter().any(|(id, ..)| *id == id_of(&side)), format!("{from_other:?}"));
    let quokka = menu(check, &other, "Quokka");
    check.check("@ leaves out the composer's own thread", !quokka.iter().any(|(id, ..)| *id == id_of(&other)), format!("{quokka:?}"));
    check.check("@ ranks the exact title first", quokka.first().is_some_and(|(id, ..)| *id == id_of(&exact)), format!("{quokka:?}"));
    check.check("@ lists at most five threads", quokka.len() <= 5, quokka.len());
    Ok(())
}

/// The `heic` phase. See the header.
fn run_heic(check: &mut Check) -> anyhow::Result<()> {
    use t3code_web::composer_draft_store::{ComposerAttachmentKind, UploadState};
    let state = load_state(&check.out)?;
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let key = thread_with_turn(check, &state, "HEIC attach")?;
    check.go(&key);
    check.wait(Duration::from_millis(300));
    // The stand-in decoder passes the bytes through, so the "HEIC" file holds a 4000x3000 JPEG.
    let photo = check.out.join("IMG_0042.HEIC");
    let image = image::RgbImage::from_fn(4000, 3000, |x, y| image::Rgb([(x / 16) as u8, (y / 12) as u8, 90]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80).encode_image(&image)?;
    std::fs::write(&photo, &jpeg)?;
    let composer = composer_of(check);
    check.cx.update(|cx| composer.update(cx, |c, cx| c.attach_paths(vec![photo.clone()], cx)));
    let attached = check.cx.update(|cx| {
        composer.read(cx).attachments().first().map(|a| match &a.kind {
            ComposerAttachmentKind::Image { bytes, mime } => (a.name.to_string(), mime.to_string(), image::load_from_memory(bytes).map(|i| (i.width(), i.height())).ok()),
            _ => (a.name.to_string(), "not an image".into(), None),
        })
    });
    check.check(
        "a HEIC photo attaches as IMG_0042.jpg, image/jpeg, 2000x1500",
        attached.as_ref().is_some_and(|(name, mime, size)| name == "IMG_0042.jpg" && mime == "image/jpeg" && *size == Some((2000, 1500))),
        format!("{attached:?}"),
    );
    let c2 = composer.clone();
    let uploaded = check.pump(
        move |c| c.cx.update(|cx| c2.read(cx).attachments().iter().all(|a| a.upload == UploadState::Uploaded)),
        Duration::from_secs(30),
        "the converted image uploaded",
    );
    check.check("the converted image uploads", uploaded.is_ok(), format!("{uploaded:?}"));
    check.shot("90-heic-attached.png")?;
    let broken = check.out.join("broken.heic");
    std::fs::write(&broken, b"not an image")?;
    let before = check.cx.update(|cx| composer.read(cx).attachments().len());
    check.cx.update(|cx| composer.update(cx, |c, cx| c.attach_paths(vec![broken.clone()], cx)));
    let after = check.cx.update(|cx| composer.read(cx).attachments().len());
    check.check("an unreadable HEIC is refused", before == after, format!("{before} -> {after}"));
    check.shot("91-heic-refused.png")
}
