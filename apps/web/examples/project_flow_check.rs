//! Drives the palette's Add project flows in the real window root on fixture folders: the
//! Sources page, Local folder (Tab completion, entering a folder, a missing folder), Git URL
//! (URL, then destination) and New project, screenshotting each page and checking where each
//! submit lands. Writes `flow.txt` and PNGs to the out dir.
//!
//!   cargo run -p t3code-web --features test-support --example project_flow_check -- <out dir>

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore};
use t3code_web::app_root::{AppRootOptions, DataSource, WindowRoot};
use t3code_web::command_palette_store::OpenIntent;
use t3code_web::components::app_sidebar_layout::TitlebarPlatform;
use t3code_web::connection::{ConnectionRuntime, RuntimeConfig};
use t3code_web::fixtures::FixtureStore;
use t3code_web::router::Route;
use t3code_web::theme_palette::{ThemeMode, ThemeSettings};
use t3code_web::ui_state_store::StateDir;

struct Check {
    out: PathBuf,
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    parts: WindowRoot,
    log: String,
    failures: Vec<String>,
}

impl Check {
    fn frame(&mut self) {
        for _ in 0..2 {
            self.cx.update_window(self.window, |_, w, cx| w.render_frame(cx)).ok();
            self.cx.run_until_parked();
        }
    }

    fn note(&mut self, line: impl AsRef<str>) {
        println!("{}", line.as_ref());
        let _ = writeln!(self.log, "{}", line.as_ref());
    }

    fn expect(&mut self, what: &str, ok: bool, detail: String) {
        self.note(format!("{} {what}: {detail}", if ok { "PASS" } else { "FAIL" }));
        if !ok {
            self.failures.push(what.to_string());
        }
    }

    fn press(&mut self, key: &str) {
        self.cx.update_window(self.window, |_, w, cx| w.press(key, cx)).ok();
        self.frame();
    }

    fn type_text(&mut self, text: &str) {
        self.cx.update_window(self.window, |_, w, cx| w.input(text, cx)).ok();
        self.frame();
    }

    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        self.frame();
        self.cx.capture_screenshot(self.window)?.save(self.out.join(format!("{name}.png")))?;
        self.note(format!("shot {name}.png"));
        Ok(())
    }

    fn palette(&mut self) -> Entity<t3code_web::components::command_palette::CommandPalette> {
        self.cx.update(|cx| self.parts.shortcuts.read(cx).palette().clone())
    }

    fn titles(&mut self) -> Vec<String> {
        let palette = self.palette();
        self.cx.update(|cx| palette.read(cx).shown_titles().iter().map(|t| t.to_string()).collect())
    }

    fn query(&mut self) -> String {
        let palette = self.palette();
        self.cx.update(|cx| palette.read(cx).query().map(|q| q.to_string()).unwrap_or_default())
    }

    fn open_sources(&mut self) {
        let palette = self.palette();
        self.cx.update_window(self.window, |_, w, cx| palette.update(cx, |p, cx| p.open(OpenIntent::AddProject, w, cx))).ok();
        self.frame();
    }

    /// Highlights the row titled `title` with the arrows and runs it.
    fn run_row(&mut self, title: &str) -> anyhow::Result<()> {
        let index = self.titles().iter().position(|t| t == title).with_context(|| format!("row {title}"))?;
        for _ in 0..=index {
            self.press("down");
        }
        self.press("enter");
        Ok(())
    }

    fn route(&mut self) -> Route {
        let app = self.parts.app.clone();
        self.cx.update(|cx| app.read(cx).router().read(cx).current().clone())
    }
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).context("usage: project_flow_check <out dir>")?);
    std::fs::create_dir_all(&out)?;
    let data = out.join("data");
    std::fs::create_dir_all(&data)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    let config = RuntimeConfig {
        secrets: Some(Arc::new(FileSecretStore::new(data.join("secrets.json")))),
        data_dir: data,
        cloud: None,
        presentation: ClientPresentation { label: "t3code-gpui project flow check".into(), ..Default::default() },
        electron: None,
    };
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(ThemeMode::Dark, cx);
        ConnectionRuntime::init(config, cx)
    })?;
    let options = AppRootOptions { data: DataSource::Fixtures(FixtureStore::reference()), platform: TitlebarPlatform::Overlay, state_dir: StateDir(None) };
    let (window, parts) = cx.update(|cx| {
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(1710.), px(1074.)), cx)),
            show: false,
            focus: true,
            ..Default::default()
        };
        WindowRoot::open_window(window_options, options, cx)
    })?;
    let mut c = Check { out: out.clone(), cx, window, parts, log: String::new(), failures: Vec::new() };
    // The owner's interface size, contrast and glass from the environment (`T3CODE_*`).
    c.cx.update(ThemeSettings::apply_env_overrides);
    c.frame();

    // Sources.
    c.open_sources();
    let titles = c.titles();
    c.expect("sources rows", titles == ["New project", "Local folder", "Git URL"], format!("{titles:?}"));
    c.shot("add-project-sources")?;

    // Local folder.
    c.run_row("Local folder")?;
    let (query, titles) = (c.query(), c.titles());
    c.expect("browse opens on ~/", query == "~/", query.clone());
    c.expect("browse lists ~/ (no .. at home, as the fork)", titles == ["code", "Documents"], format!("{titles:?}"));
    c.shot("add-project-browse")?;
    c.type_text("co");
    let titles = c.titles();
    c.expect("filter co", titles == ["code"], format!("{titles:?}"));
    c.shot("add-project-browse-filter")?;
    c.press("tab");
    let (query, titles) = (c.query(), c.titles());
    c.expect("tab enters ~/code/", query == "~/code/", query);
    c.expect("lists ~/code/", titles == ["..", "infra", "marketing-site", "relay", "t3code"], format!("{titles:?}"));
    c.shot("add-project-browse-code")?;
    c.press("down");
    c.shot("add-project-browse-highlight")?;
    c.type_text("newdir");
    let titles = c.titles();
    c.expect("missing folder keeps ..", titles == [".."], format!("{titles:?}"));
    c.shot("add-project-browse-create")?;
    c.press("enter");
    let (route, open) = (c.route(), c.palette().read_with(&c.cx, |p, _| p.is_open()));
    c.expect("Create & Add opens a draft", matches!(route, Route::Draft(_)) && !open, format!("{route:?}, palette open {open}"));

    // Git URL.
    c.open_sources();
    c.run_row("Git URL")?;
    c.shot("add-project-clone-url")?;
    c.type_text("/srv/repos/widgets.git");
    c.shot("add-project-clone-url-typed")?;
    c.press("enter");
    let (query, titles) = (c.query(), c.titles());
    c.expect("destination is ~/widgets", query == "~/widgets", query);
    c.expect("destination lists ~/", titles == ["code", "Documents"], format!("{titles:?}"));
    c.shot("add-project-clone-destination")?;
    c.press("enter");
    let (route, open) = (c.route(), c.palette().read_with(&c.cx, |p, _| p.is_open()));
    c.expect("Create & Clone opens a draft", matches!(route, Route::Draft(_)) && !open, format!("{route:?}, palette open {open}"));

    // New project.
    c.open_sources();
    c.run_row("New project")?;
    c.shot("new-project-empty")?;
    c.type_text("Quokka Notes");
    c.shot("new-project-named")?;
    c.press("enter");
    let (route, open) = (c.route(), c.palette().read_with(&c.cx, |p, _| p.is_open()));
    c.expect("Create opens a draft", matches!(route, Route::Draft(_)) && !open, format!("{route:?}, palette open {open}"));

    std::fs::write(out.join("flow.txt"), &c.log)?;
    if !c.failures.is_empty() {
        bail!("{} failed: {:?}", c.failures.len(), c.failures);
    }
    println!("all passed");
    Ok(())
}
