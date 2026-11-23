mod notifications;
mod window;

fn main() {
    gpui_kit::application()
        .with_assets(t3code_web::Assets)
        .run(|cx| {
            t3code_web::init(cx);
            let options = window::main_window_options(cx);
            let (handle, _) = window::open_main_window(options, cx).expect("open main window");
            notifications::install(cx);
            handle
                .update(cx, |_, window, cx| {
                    t3code_web::components::thread_notification_coordinator::ThreadNotificationCoordinator::watch_window(window, cx)
                })
                .ok();
            cx.activate(true);
        });
}
