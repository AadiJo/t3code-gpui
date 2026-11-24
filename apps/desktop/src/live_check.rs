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
//   import   a fresh data directory and a stand-in install (`T3CODE_IMPORT_HOME`, made by
//            `import-check`, encrypted with the fixture macOS key): the launch import runs,
//            the window explains the Keychain read, environments and settings arrive
//
// Usage: live-check <seed|drive|restart|import> <out-dir>
#[allow(dead_code)]
#[path = "import/mod.rs"]
mod import;

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
        cx.update(|cx| {
            t3code_web::init(cx);
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
    let usage = "usage: live-check <seed|drive|restart|import> <out-dir>";
    let phase = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    let key = (phase == "import").then(|| SafeStorageKey::from_mac_password(b"fixture keychain password"));
    let mut check = Check::open(&out, key)?;
    check.note(format!("phase {phase}, data dir {}", data_dir()?.display()));
    let result = match phase.as_str() {
        "seed" => run_seed(&mut check),
        "drive" => run_drive(&mut check),
        "restart" => run_restart(&mut check),
        "import" => run_import(&mut check),
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
