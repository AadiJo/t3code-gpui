// End-to-end check of the settings stores and pages in headless windows,
// with a fixture environment and no server. `run` changes each wired
// setting through the real controls or stores and screenshots the view it
// affects before and after, imports a made-up Electron settings file,
// records a keybinding and presses it; then it quits. `relaunch` is a new
// process on the same data directory that checks everything persisted.
//
// Usage: settings-check <out-dir> <data-dir> <run|relaunch>
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, bail, ensure};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use t3code_contracts::{ChatWidth, DiffLayout};
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand};
use t3code_web::components::settings::settings_search::{SettingsPath, search_settings};
use t3code_web::hooks::use_settings::ClientSettingsStore;
use t3code_web::hooks::use_theme::ThemeStore;
use t3code_web::routes::settings::SettingsRoute;
use t3code_web::shortcuts::commands::command_action;
use t3code_web::shortcuts::keymap::ShortcutState;

struct Check {
    cx: HeadlessAppContext,
    out: PathBuf,
    started: Instant,
    log: String,
}

impl Check {
    fn note(&mut self, text: impl AsRef<str>) {
        let line = format!("{:>8.3}s  {}", self.started.elapsed().as_secs_f64(), text.as_ref());
        println!("{line}");
        writeln!(self.log, "{line}").ok();
    }

    fn frame(&mut self, handle: AnyWindowHandle) -> anyhow::Result<()> {
        self.cx.update_window(handle, |_, w, cx| w.render_frame(cx))?;
        self.cx.run_until_parked();
        self.cx.update_window(handle, |_, w, cx| w.render_frame(cx))?;
        Ok(())
    }

    fn shot(&mut self, handle: AnyWindowHandle, name: &str) -> anyhow::Result<()> {
        self.frame(handle)?;
        let image = self.cx.capture_screenshot(handle)?;
        image.save(self.out.join(name))?;
        self.note(format!("screenshot {name}"));
        Ok(())
    }

    fn click(&mut self, handle: AnyWindowHandle, id: impl Into<SharedString>) -> anyhow::Result<()> {
        let id: SharedString = id.into();
        self.frame(handle)?;
        let element = id.clone();
        self.cx.update_window(handle, |_, w, cx| w.click(element, cx))?;
        self.frame(handle)?;
        self.note(format!("click {id}"));
        Ok(())
    }

    /// Scrolls the settings page by `dy` pixels (negative moves down the page).
    fn scroll(&mut self, handle: AnyWindowHandle, dy: f32) -> anyhow::Result<()> {
        self.frame(handle)?;
        self.cx.update_window(handle, |_, w, cx| w.scroll("settings-content", ScrollDelta::Pixels(point(px(0.), px(dy))), cx))?;
        self.frame(handle)?;
        self.note(format!("scroll {dy}"));
        Ok(())
    }

    fn settings<R>(&mut self, read: impl FnOnce(&t3code_web::hooks::use_settings::ClientSettings) -> R) -> R {
        self.cx.update(|cx| read(ClientSettingsStore::get(cx)))
    }
}

/// A window with the root key context the keymap's rules require, to see
/// what a keystroke resolves to.
struct KeyProbe {
    focus: FocusHandle,
    fired: usize,
}

impl Render for KeyProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context(ShortcutState::get(cx).root_key_context())
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &t3code_web::shortcuts::commands::ToggleCommandPalette, _, _| this.fired += 1))
    }
}

fn open_window<V: Render>(cx: &mut HeadlessAppContext, width: f32, height: f32, build: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> anyhow::Result<(AnyWindowHandle, Entity<V>)> {
    cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
            show: false,
            focus: true,
            ..component::TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| build(window, cx))
    })
    .map_err(Into::into)
}

struct Root(AnyView);
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

fn settings_window(check: &mut Check, path: SettingsPath) -> anyhow::Result<(AnyWindowHandle, Entity<SettingsRoute>)> {
    let mut route = None;
    let handle = open_window(&mut check.cx, 1400., 1000., |window, cx| {
        let (view, settings) = t3code_web::scenes::settings::open(path, window, cx);
        route = Some(settings);
        cx.new(|_| Root(view))
    })?;
    Ok((handle.0, route.expect("settings route")))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: settings-check <out-dir> <data-dir> <run|relaunch>";
    let out = PathBuf::from(args.next().context(usage)?);
    let data_dir = PathBuf::from(args.next().context(usage)?);
    let phase = args.next().context(usage)?;
    std::fs::create_dir_all(&out)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        t3code_web::init(cx);
        t3code_web::hooks::load(&data_dir, cx);
    });
    let mut check = Check { cx, out: out.clone(), started: Instant::now(), log: String::new() };
    check.note(format!("phase {phase}, data dir {}", data_dir.display()));
    let result = match phase.as_str() {
        "run" => run(&mut check, &data_dir),
        "relaunch" => relaunch(&mut check, &data_dir),
        other => bail!("unknown phase {other}"),
    };
    if let Err(error) = &result {
        check.note(format!("FAILED: {error:#}"));
    } else {
        check.note("PASSED");
    }
    std::fs::write(out.join(format!("{phase}-transcript.txt")), &check.log)?;
    result
}

fn run(check: &mut Check, data_dir: &Path) -> anyhow::Result<()> {
    let (window, route) = settings_window(check, SettingsPath::Appearance)?;
    check.shot(window, "01-appearance-default.png")?;

    // Interface font size: the + stepper three times, 16 → 19.
    check.scroll(window, -2400.)?;
    for _ in 0..3 {
        check.click(window, "interface-font-size-up")?;
    }
    let size = check.settings(|s| s.font_size_interface);
    let rem = check.cx.update(|cx| t3code_web::ActiveAppTheme::app_theme(cx).fonts.interface);
    check.note(format!("fontSizeInterface = {size}, theme rem = {rem}"));
    ensure!(size == 19 && rem == 19., "interface size did not reach the theme");
    check.shot(window, "02-appearance-interface-19px.png")?;

    // Theme: the Ember card.
    check.scroll(window, 2400.)?;
    check.click(window, "theme-card-ember")?;
    let theme = check.cx.update(|cx| (ThemeStore::global(cx).preference().to_string(), t3code_web::ActiveAppTheme::app_theme(cx).id.clone()));
    check.note(format!("theme preference = {}, drawn theme = {}", theme.0, theme.1));
    ensure!(theme.1 == "ember", "theme did not apply");
    check.shot(window, "03-appearance-ember.png")?;

    // Diff layout: the diff panel before and after, through the settings store.
    let diffs = open_window(&mut check.cx, 1200., 800., |window, cx| {
        let view = t3code_web::scenes::diffs::build(window, cx);
        cx.new(|_| Root(view))
    })?
    .0;
    check.shot(diffs, "04-diffs-stacked.png")?;
    check.cx.update(|cx| ClientSettingsStore::update(cx, |s| s.diff_layout = DiffLayout::Split));
    check.frame(diffs)?;
    let panel = check.cx.update(|cx| t3code_web::diff_panel_store::DiffPanelStore::global(cx).settings.diff_layout);
    check.note(format!("diffLayout = split, diff panel store layout = {panel:?}"));
    ensure!(panel == t3code_web::diff_panel_store::DiffLayout::Split, "diff layout did not reach the panel");
    check.shot(diffs, "05-diffs-split.png")?;

    // Chat width has no consumer in this build; the setting is still stored.
    check.cx.update(|cx| ClientSettingsStore::update(cx, |s| s.chat_width = ChatWidth::Wide));

    // Language: German, then the General page.
    check.cx.update(|cx| route.update(cx, |route, cx| route.navigate(SettingsPath::General, cx)));
    check.shot(window, "06-general-english.png")?;
    check.cx.update(|cx| ClientSettingsStore::update(cx, |s| s.language = "de".into()));
    for _ in 0..20 {
        check.frame(window)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let locale = check.cx.update(|cx| cx.global::<t3code_web::i18n::runtime::I18n>().locale);
    check.note(format!("language = de, active locale = {locale:?}"));
    check.shot(window, "07-general-german.png")?;
    check.cx.update(|cx| ClientSettingsStore::update(cx, |s| s.language = "en".into()));
    for _ in 0..10 {
        check.frame(window)?;
    }

    // Search: the web's matching over the full catalog.
    for query in ["font", "diff split", "sidebar toggle", "dock badge"] {
        let hits: Vec<String> = check.cx.update(|_| search_settings(query).iter().take(4).map(|h| format!("{} ({})", h.title, h.path.slug())).collect());
        check.note(format!("search {query:?}: {}", hits.join(", ")));
    }

    // Import a made-up Electron client-settings.json.
    let fixture = data_dir.join("electron-fixture-client-settings.json");
    std::fs::write(
        &fixture,
        serde_json::to_vec_pretty(&serde_json::json!({
            "settings": {
                "chatWidth": "full",
                "fontSizeCode": 15,
                "sendShortcut": "mod-enter",
                "diffColorScheme": "blue-orange",
                "timestampFormat": "24-hour",
                "aSettingFromANewerApp": { "kept": true }
            }
        }))?,
    )?;
    let applied = check.cx.update(|cx| ClientSettingsStore::import_electron(&fixture, cx))?;
    let imported = check.settings(|s| (s.chat_width.clone(), s.font_size_code, s.send_shortcut.clone(), s.diff_color_scheme.clone()));
    check.note(format!("imported {applied} settings: {imported:?}"));
    ensure!(imported.0 == ChatWidth::Full && imported.1 == 15, "import did not apply");

    // Record Ctrl+Alt+Y for Command Palette: Toggle, then press it.
    check.cx.update(|cx| route.update(cx, |route, cx| route.navigate(SettingsPath::Keybindings, cx)));
    check.frame(window)?;
    let page = check.cx.update(|cx| route.read(cx).pages().read(cx).keybindings().clone());
    check.cx.update_window(window, |_, w, cx| page.update(cx, |page, cx| page.start_recording_for_scene("commandPalette.toggle", w, cx)))?;
    check.shot(window, "08-keybindings-recording.png")?;
    check.cx.update_window(window, |_, w, cx| w.press("ctrl-alt-y", cx))?;
    check.frame(window)?;
    check.shot(window, "09-keybindings-recorded.png")?;
    let probe = open_window(&mut check.cx, 200., 200., |window, cx| {
        let view = cx.new(|cx| KeyProbe { focus: cx.focus_handle(), fired: 0 });
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
        view
    })?;
    let diag = check.cx.update_window(probe.0, |_, w, cx| {
        let action = command_action(&KeybindingCommand::Static(StaticCommand::CommandPaletteToggle));
        let bindings: Vec<String> = w.bindings_for_action(&*action).iter().map(|b| format!("{:?}", b.keystrokes())).collect();
        let all = cx.key_bindings().borrow().bindings_for_action(&*action).map(|b| format!("{:?} {:?}", b.keystrokes(), b.predicate())).collect::<Vec<_>>();
        (w.focused(cx).is_some(), w.context_stack(), bindings, all)
    })?;
    check.note(format!("probe focused {}, context {:?}, active bindings {:?}", diag.0, diag.1, diag.2));
    check.note(format!("all bindings for the action: {:?}", diag.3));
    let fired = Arc::new(std::sync::Mutex::new(None::<String>));
    let sink = fired.clone();
    check.cx.update(|cx| {
        cx.observe_keystrokes(move |event, _, _| {
            *sink.lock().expect("lock") = event.action.as_ref().map(|a| a.name().to_owned());
        })
        .detach()
    });
    check.cx.update_window(probe.0, |_, w, cx| w.press("ctrl-alt-y", cx))?;
    check.cx.run_until_parked();
    let expected = command_action(&KeybindingCommand::Static(StaticCommand::CommandPaletteToggle)).name().to_owned();
    let got = fired.lock().expect("lock").clone();
    let handled = check.cx.update(|cx| probe.1.read(cx).fired);
    check.note(format!("ctrl-alt-y resolved to {got:?} (expected {expected}); the probe's handler ran {handled} time(s)"));
    ensure!(handled == 1, "recorded keybinding does not fire its command");

    // Quit: the quit hooks write any pending file; `relaunch` reads it.
    check.cx.update(|cx| cx.quit());
    check.cx.run_until_parked();
    Ok(())
}

fn relaunch(check: &mut Check, data_dir: &Path) -> anyhow::Result<()> {
    let file = std::fs::read_to_string(data_dir.join("client-settings.json")).context("client-settings.json written")?;
    let kept = file.contains("aSettingFromANewerApp");
    check.note(format!("client-settings.json: {} bytes, keeps the newer app's unknown key: {kept}", file.len()));
    ensure!(kept, "unknown key was not kept");
    let s = check.settings(|s| (s.font_size_interface, s.diff_layout.clone(), s.chat_width.clone(), s.font_size_code, s.language.clone()));
    let theme = check.cx.update(|cx| ThemeStore::global(cx).preference().to_string());
    check.note(format!("after relaunch: interface {}, diffLayout {:?}, chatWidth {:?}, code {}, language {}, theme {theme}", s.0, s.1, s.2, s.3, s.4));
    ensure!(s.0 == 19 && s.1 == DiffLayout::Split && s.2 == ChatWidth::Full && s.3 == 15 && theme == "ember", "settings did not persist");
    let (window, _) = settings_window(check, SettingsPath::Appearance)?;
    check.shot(window, "10-relaunch-appearance.png")?;
    Ok(())
}
