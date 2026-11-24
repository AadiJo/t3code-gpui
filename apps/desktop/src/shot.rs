// Headless screenshot of one scene, for visual checks without a display.
// Usage: shot <scene> <out.png> [width height]
// `T3CODE_GLASS_OPACITY=<40..100>` renders with that Glass opacity setting;
// `T3CODE_INTERFACE_SIZE=<px>` with that interface font size (the rem).
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use gpui_kit::{
    AnyView, AppContext as _, HeadlessAppContext, IntoElement, Render, WindowBounds, WindowOptions,
    component::TitleBar, px, size, test::TestWindowExt as _,
};
use t3code_web::{
    scenes,
    theme_palette::{ThemeMode, ThemeSettings},
    theme_tokens::GlassOpacity,
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
    let out = args.next().ok_or_else(usage)?;
    let width: f32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(1100.);
    let height: f32 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(780.);
    let build = scenes::scene(&name).with_context(|| format!("unknown scene {name}"))?;

    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        t3code_web::init(cx);
        // The headless platform reports a light system appearance; pin the mode
        // so a screenshot does not depend on it.
        let mode = if name.ends_with("-light") { ThemeMode::Light } else { ThemeMode::Dark };
        ThemeSettings::set_mode(mode, cx);
        if let Some(percent) = std::env::var("T3CODE_GLASS_OPACITY").ok().and_then(|v| v.parse().ok()) {
            ThemeSettings::set_glass_opacity(GlassOpacity::new(percent), cx);
        }
        if let Some(size) = std::env::var("T3CODE_INTERFACE_SIZE").ok().and_then(|v| v.parse::<f32>().ok()) {
            ThemeSettings::update(cx, |settings| settings.fonts.interface = size);
        }
    });
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
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
    let image = cx.capture_screenshot(handle)?;
    image.save(&out)?;
    println!("{out}: {}x{}", image.width(), image.height());
    Ok(())
}
