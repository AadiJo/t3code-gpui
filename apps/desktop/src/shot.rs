// Headless screenshot of the main window, for visual checks without a display.
mod window;

use std::sync::Arc;

use gpui_kit::{HeadlessAppContext, test::TestWindowExt as _};

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "shot.png".into());
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(t3code_web::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(t3code_web::init);
    let (handle, _root) = cx.update(|cx| {
        let mut options = window::main_window_options(cx);
        options.show = false;
        options.focus = false;
        window::open_main_window(options, cx)
    })?;
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))?;
    let image = cx.capture_screenshot(handle)?;
    image.save(&out)?;
    println!("{out}: {}x{}", image.width(), image.height());
    Ok(())
}
