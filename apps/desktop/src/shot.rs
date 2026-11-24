// Headless screenshot of one scene, for visual checks without a display.
// Usage: shot <scene> <out.png> [width height]; `shot --list` prints every scene name.
// `T3CODE_CONTRAST`, `T3CODE_GLASS_OPACITY`, `T3CODE_INTERFACE_SIZE` and
// `T3CODE_PANEL_MS` render with those client settings; `T3CODE_CLIENT_SETTINGS=<file>`
// applies a whole client settings file (an install's `client-settings.json`) to every scene
// first, with the appearance still pinned (dark, or light for `-light` scenes) as the
// owner's Mac shows it; `T3CODE_SCALE` sets the
// device scale (2 by default). `T3CODE_SHOT_REPEAT=<n>` also renders the
// finished scene n more times, each read back so the GPU work is included,
// and prints the times.
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use gpui_kit::{
    AnyView, AppContext as _, HeadlessAppContext, IntoElement, Render, WindowBounds, WindowOptions,
    component::TitleBar, px, size, test::TestWindowExt as _,
};
use t3code_web::{
    hooks::{use_settings::ClientSettingsStore, use_theme::ThemeStore},
    scenes,
    theme_palette::{ThemeMode, ThemeSettings},
};

/// Wraps a scene so it can be the window's root view.
struct SceneHost(AnyView);

impl Render for SceneHost {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let names: Vec<_> = scenes::scenes().iter().map(|(name, _)| *name).collect();
    let usage = || anyhow!("usage: shot <{}> <out.png> [width height]", names.join("|"));
    let name = args.next().ok_or_else(usage)?;
    if name == "--list" {
        for name in scenes::scenes().iter().chain(scenes::ui::SCENES).map(|(name, _)| *name) {
            println!("{name}");
        }
        return Ok(());
    }
    let out = args.next().ok_or_else(usage)?;
    let width: f32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(1100.);
    let height: f32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(780.);
    let build = scenes::scene(&name).with_context(|| format!("unknown scene {name}"))?;

    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    // The headless platform reports a light system appearance; pin the mode
    // so a screenshot does not depend on it.
    let mode = if name.ends_with("-light") { ThemeMode::Light } else { ThemeMode::Dark };
    let settings = std::env::var_os("T3CODE_CLIENT_SETTINGS").map(std::path::PathBuf::from);
    cx.update(|cx| {
        t3code_web::init(cx);
        ThemeSettings::set_mode(mode, cx);
        if let Some(path) = &settings {
            ClientSettingsStore::import_electron(path, cx)?;
            ThemeStore::set_appearance_mode(mode, cx);
        }
        anyhow::Ok(())
    })?;
    let (handle, _host) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(width), px(height)), cx)),
            show: false,
            focus: false,
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            let view = build(window, cx);
            cx.new(|_| SceneHost(view))
        })
    })?;
    if let Some(scale) = std::env::var("T3CODE_SCALE").ok().and_then(|value| value.parse().ok()) {
        cx.simulate_window_scale_factor_change(handle.into(), scale);
    }
    cx.update(|cx| {
        // Scenes that load the settings file themselves replace the appearance mode with the
        // file's ("system"); pin it again.
        if settings.is_some() {
            ThemeStore::set_appearance_mode(mode, cx);
        }
        ThemeSettings::apply_env_overrides(cx);
    });
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
    cx.run_until_parked();
    // `T3CODE_SHOT_EVENTS="drag:x1,y1,x2,y2;right:x,y"`: pointer input before the capture.
    for step in std::env::var("T3CODE_SHOT_EVENTS").unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
        use gpui_kit::{InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, point};
        let (kind, args) = step.split_once(':').context("event kind")?;
        let v: Vec<f32> = args.split(',').map(str::parse).collect::<Result<_, _>>()?;
        let at = |i: usize| point(px(v[i]), px(v[i + 1]));
        cx.update_window(handle, |_, w, cx| {
            let none = Modifiers::default();
            let (button, from, to) = if kind == "right" { (MouseButton::Right, at(0), at(0)) } else { (MouseButton::Left, at(0), at(2)) };
            w.dispatch_event(MouseMoveEvent { position: from, pressed_button: None, modifiers: none }.to_platform_input(), cx);
            w.dispatch_event(MouseDownEvent { button, position: from, modifiers: none, click_count: 1, first_mouse: false }.to_platform_input(), cx);
            if from != to {
                w.dispatch_event(MouseMoveEvent { position: to, pressed_button: Some(button), modifiers: none }.to_platform_input(), cx);
            }
            w.dispatch_event(MouseUpEvent { button, position: to, modifiers: none, click_count: 1 }.to_platform_input(), cx);
        })?;
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
        cx.run_until_parked();
    }
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
    let image = cx.capture_screenshot(handle)?;
    image.save(&out)?;
    if let Some(repeat) = std::env::var("T3CODE_SHOT_REPEAT").ok().and_then(|value| value.parse::<usize>().ok()) {
        let mut times = Vec::with_capacity(repeat);
        for _ in 0..repeat {
            let start = std::time::Instant::now();
            cx.capture_screenshot(handle)?;
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
        times.sort_by(f64::total_cmp);
        let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
        println!("frame with readback, {repeat} frames: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms", at(0.5), at(0.95), at(1.));
    }
    println!("{out}: {}x{}", image.width(), image.height());
    Ok(())
}
