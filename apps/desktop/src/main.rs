mod menu;
mod notifications;
mod window;

fn main() {
    gpui_kit::application()
        .with_assets(t3code_web::Assets)
        .run(|cx| {
            t3code_web::init(cx);
            if let Err(error) =
                t3code_web::connection::ConnectionRuntime::init(t3code_web::connection::RuntimeConfig::for_app(), cx)
            {
                eprintln!("could not start the connection runtime: {error:#}");
                cx.quit();
                return;
            }
            t3code_web::hooks::load(&t3code_client_runtime::platform::default_data_dir(), cx);
            let options = window::main_window_options(cx);
            // `T3CODE_FIXTURES=0` starts without the showcase data.
            let fixtures = std::env::var("T3CODE_FIXTURES").map_or(true, |v| v != "0");
            let (handle, root) = window::open_main_window(options, fixtures, cx).expect("open main window");
            menu::install(root.app.clone(), cx);
            notifications::install(cx);
            handle
                .update(cx, |_, window, cx| {
                    t3code_web::components::thread_notification_coordinator::ThreadNotificationCoordinator::watch_window(window, cx)
                })
                .ok();
            cx.activate(true);
        });
}
