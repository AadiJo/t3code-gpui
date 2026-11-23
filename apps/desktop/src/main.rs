mod window;

fn main() {
    gpui_kit::application()
        .with_assets(t3code_web::Assets)
        .run(|cx| {
            t3code_web::init(cx);
            let options = window::main_window_options(cx);
            window::open_main_window(options, cx).expect("open main window");
            cx.activate(true);
        });
}
