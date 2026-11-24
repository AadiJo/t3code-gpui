// The first-launch import as the `t3code` binary runs it (`main.rs` order: file import, runtime,
// settings, window, connection import), in a headless window, against a stand-in install
// (`T3CODE_IMPORT_HOME`, built by `import-check <out> <owner-dir>` around the owner's settings
// and localStorage and a fixture catalog encrypted with the macOS scheme). Every phase hashes
// every install file before and after and fails if one changed.
//
//   first    fresh data dir: settings and localStorage arrive, the Keychain notice shows, the
//            catalog is decrypted with the fixture key and its environments saved (T3 Connect
//            routes only with a session to adopt, so none here); the
//            device-bound route reads like a signed-out route with a way to pair again
//   second   same data dir, after the install's client-settings.json changed: the change is
//            applied, nothing is imported twice, no environment or route is duplicated
//   denied   fresh data dir, and the key read fails (as a Keychain "Deny" does): the app still
//            starts with the imported plaintext settings and offers pairing
//
// Usage: launch-check <first|second|denied> <out-dir> <data-dir>
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
use sha2::{Digest, Sha256};
use t3code_client_runtime::connection::routes::route_id;
use t3code_client_runtime::platform::electron_import::SafeStorageKey;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, DataSource, WindowRoot};
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::components::settings::settings_search::SettingsPath;
use t3code_web::connection::presentation::diagnose;
use t3code_web::connection::shell::EnvironmentShell;
use t3code_web::connection::{ConnectionRuntime, ElectronImportSource, RuntimeConfig};
use t3code_web::theme_palette::ThemeSettings;
use t3code_web::ui_state_store::StateDir;

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
    failures: usize,
    window: AnyWindowHandle,
    shell: Entity<EnvironmentShell>,
}

impl Check {
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

    fn frame(&mut self) {
        self.cx.run_until_parked();
        self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
    }

    /// Pumps for `duration` of wall time, advancing the headless clock with it.
    fn pump(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.cx.advance_clock(Duration::from_millis(20));
            self.frame();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        self.cx.capture_screenshot(self.window)?.save(self.out.join(name))?;
        self.note(format!("screenshot {name}"));
        Ok(())
    }

    fn open_environments(&mut self) -> anyhow::Result<()> {
        let shell = self.shell.clone();
        self.cx
            .update_window(self.window, |_, w, cx| shell.update(cx, |shell, cx| shell.open_settings(SettingsPath::Connections, w, cx)))?;
        self.pump(Duration::from_millis(300));
        Ok(())
    }

    /// `(environment ids, route ids)` in the saved catalog.
    fn catalog(&mut self) -> (Vec<String>, Vec<String>) {
        self.cx.update(|cx| {
            let state = ConnectionRuntime::state(cx).read(cx);
            let ids = state.environments.iter().map(|e| e.id().to_string()).collect();
            let routes = state
                .environments
                .iter()
                .flat_map(|e| e.entry.routes.iter().map(move |r| format!("{} {}", e.id(), route_id(&r.target))))
                .collect();
            (ids, routes)
        })
    }
}

fn digest(path: &Path) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}

/// Every file under `dir` with its digest.
fn tree_digest(dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                files.push((entry.path().strip_prefix(dir)?.display().to_string(), digest(&entry.path())?));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn open(out: &Path, data: &Path, key: Option<SafeStorageKey>) -> anyhow::Result<Check> {
    let ui = data.join("ui");
    let launch = import::before_settings(data, Some(&ui));
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
        data_dir: data.to_owned(),
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui launch check".into(), ..Default::default() },
        // `None` reads the key from the OS, which fails on Linux exactly where a denied
        // Keychain read fails on macOS (`ImportError::KeyUnavailable`/`UnsupportedPlatform`).
        electron: launch.install.as_ref().map(|install| ElectronImportSource { paths: install.paths.clone(), key }),
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ConnectionRuntime::init(config, cx)?;
        t3code_web::hooks::load(data, cx);
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
        let opened = WindowRoot::open_window(window_options, options, cx);
        ThemeSettings::apply_env_overrides(cx);
        opened
    })?;
    let shell = parts.shell.clone();
    cx.update(|cx| import::import_connections(launch, cx));
    let mut check = Check { cx, out: out.to_owned(), started: Instant::now(), log: String::new(), failures: 0, window, shell };
    check.frame();
    Ok(check)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: launch-check <first|second|denied> <out-dir> <data-dir>";
    let phase = args.next().context(usage)?;
    let out = PathBuf::from(args.next().context(usage)?);
    let data = PathBuf::from(args.next().context(usage)?);
    std::fs::create_dir_all(&out)?;
    let install = PathBuf::from(std::env::var_os("T3CODE_IMPORT_HOME").context("set T3CODE_IMPORT_HOME")?);
    let before = tree_digest(&install)?;
    let key = (phase != "denied").then(|| SafeStorageKey::from_mac_password(b"fixture keychain password"));
    let mut check = open(&out, &data, key)?;
    check.note(format!("phase {phase}, install has {} files", before.len()));
    let result = match phase.as_str() {
        "first" => run_first(&mut check),
        "second" => run_second(&mut check),
        "denied" => run_denied(&mut check),
        other => bail!("unknown phase {other}"),
    };
    if let Err(error) = &result {
        check.note(format!("FAILED: {error:#}"));
    }
    let after = tree_digest(&install)?;
    check.check("every install file is byte-identical", before == after, format!("{} files", after.len()));
    check.cx.update(ConnectionRuntime::shutdown);
    std::fs::write(out.join(format!("{phase}-transcript.txt")), &check.log)?;
    result?;
    if check.failures > 0 {
        bail!("{} checks failed", check.failures);
    }
    Ok(())
}

fn interface_font(check: &mut Check) -> String {
    check.cx.update(|cx| format!("{:?}", t3code_web::hooks::use_settings::ClientSettingsStore::get(cx).font_size_interface))
}

fn run_first(check: &mut Check) -> anyhow::Result<()> {
    check.shot("l01-first-launch-notice.png")?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while check.catalog().0.is_empty() {
        check.pump(Duration::from_millis(100));
        if Instant::now() > deadline {
            bail!("no environments were imported");
        }
    }
    check.pump(Duration::from_millis(500));
    check.shot("l02-first-launch-imported.png")?;
    let (ids, routes) = check.catalog();
    check.note(format!("{} environments, routes {routes:?}", ids.len()));
    // This check runs without T3 Connect, so no session is adopted and the relay-only "Cloud box"
    // and Desk's relay route stay behind (signing in lists them again with Add).
    check.check("the fixture's three directly reachable environments arrive", ids.len() == 3, format!("{ids:?}"));
    check.check("no T3 Connect route without a session to use it", !routes.iter().any(|r| r.ends_with(" relay")), format!("{routes:?}"));
    let font = interface_font(check);
    check.check("the owner's interface size (15) applies", font.contains("15"), &font);
    check.open_environments()?;
    let device_bound = check.cx.update(|cx| {
        ConnectionRuntime::state(cx).read(cx).environments.iter().find(|e| e.entry.label == "Laptop").map(|e| {
            let diagnosis = diagnose(e);
            (diagnosis.as_ref().map(|d| d.headline.to_string()), diagnosis.map(|d| d.steps.join(" / ")))
        })
    });
    check.note(format!("device-bound Laptop: {device_bound:?}"));
    let wait_until = Instant::now() + Duration::from_secs(10);
    let mut headline = String::new();
    while Instant::now() < wait_until {
        check.pump(Duration::from_millis(200));
        headline = check.cx.update(|cx| {
            ConnectionRuntime::state(cx)
                .read(cx)
                .environments
                .iter()
                .find(|e| e.entry.label == "Laptop")
                .and_then(diagnose)
                .map(|d| format!("{} | {}", d.headline, d.steps.join(" / ")))
                .unwrap_or_default()
        });
        if !headline.is_empty() {
            break;
        }
    }
    check.check(
        "the device-bound route reads as signed out with a way to pair again",
        headline.contains("did not accept this device") && headline.contains("Pair it again"),
        &headline,
    );
    let laptop = check.cx.update(|cx| {
        ConnectionRuntime::state(cx).read(cx).environments.iter().find(|e| e.entry.label == "Laptop").map(|e| e.id().to_string())
    });
    if let Some(id) = laptop {
        let shell = check.shell.clone();
        check.cx.update_window(check.window, |_, _, cx| {
            if let Some(settings) = shell.read(cx).settings().cloned() {
                let page = settings.read(cx).environments().clone();
                page.update(cx, |page, cx| page.toggle_for_scene(&id, true, true, cx));
            }
        })?;
    }
    check.pump(Duration::from_millis(300));
    check.shot("l03-device-bound-signed-out.png")?;
    let state = import::install::ImportState::load(&data_dir_of(check));
    check.check("the import is recorded", state.files_imported && state.connections_imported, format!("{state:?}"));
    Ok(())
}

fn data_dir_of(check: &mut Check) -> PathBuf {
    check.cx.update(|cx| ConnectionRuntime::global(cx).data_dir().to_owned())
}

fn run_second(check: &mut Check) -> anyhow::Result<()> {
    check.pump(Duration::from_secs(2));
    let (ids, routes) = check.catalog();
    let mut unique_routes = routes.clone();
    unique_routes.sort();
    unique_routes.dedup();
    check.check("no environment was imported twice", ids.len() == 3, format!("{ids:?}"));
    check.check("no route was duplicated", unique_routes.len() == routes.len(), format!("{} routes", routes.len()));
    let expected = std::env::var("EXPECT_INTERFACE_SIZE").unwrap_or_default();
    let font = interface_font(check);
    check.check("the changed client-settings.json is applied", !expected.is_empty() && font.contains(&expected), format!("{font} (expected {expected})"));
    check.shot("l04-second-launch.png")?;
    Ok(())
}

fn run_denied(check: &mut Check) -> anyhow::Result<()> {
    check.pump(Duration::from_secs(3));
    check.shot("l05-key-denied.png")?;
    let (ids, _) = check.catalog();
    check.check("no environments without the key", ids.is_empty(), format!("{ids:?}"));
    let install = PathBuf::from(std::env::var_os("T3CODE_IMPORT_HOME").unwrap_or_default());
    let expected = std::fs::read(install.join("userdata/client-settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|settings| settings.get("fontSizeInterface").map(|size| size.to_string()))
        .unwrap_or_default();
    let font = interface_font(check);
    check.check("the plaintext settings still applied", !expected.is_empty() && font.contains(&expected), format!("{font} (install has {expected})"));
    let offers_pairing = check.cx.update_window(check.window, |_, w, _| w.try_find("empty-add-pairing").is_some())?;
    check.check("the window offers pairing", offers_pairing, "empty-add-pairing present");
    Ok(())
}
