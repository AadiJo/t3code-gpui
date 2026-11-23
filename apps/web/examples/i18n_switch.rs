//! Opens the `i18n` scene headlessly, clicks each language button, and saves a
//! screenshot after every switch, proving open views re-render in the new language.
//!
//!   cargo run -p t3code-web --features test-support --example i18n_switch -- <out dir>

use std::sync::Arc;

use gpui_kit::{
    AppContext as _, HeadlessAppContext, IntoElement, Render, WindowBounds, WindowOptions, px, size,
    test::TestWindowExt as _,
};
use t3code_web::{i18n::active_locale, scenes};

struct Host(gpui_kit::AnyView);

impl Render for Host {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("output directory"));
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(t3code_web::init);
    let build = scenes::scene("i18n").unwrap();
    let (handle, _host) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(None, size(px(900.), px(420.)), cx)),
            show: false,
            focus: false,
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            let view = build(window, cx);
            cx.new(|_| Host(view))
        })
    })?;
    cx.run_until_parked();
    for tag in ["en", "de", "ja", "ru", "pt-BR", "en"] {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("lang-{tag}"), cx);
        })?;
        // The catalog inflates on the background executor, then every window refreshes.
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
        let path = out.join(format!("i18n-switch-{tag}.png"));
        cx.capture_screenshot(handle)?.save(&path)?;
        println!("clicked {tag}: active locale {:?}, saved {}", active_locale(), path.display());
    }
    Ok(())
}
