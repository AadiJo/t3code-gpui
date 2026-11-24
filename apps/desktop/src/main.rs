mod menu;
mod notifications;
mod window;

fn main() {
    // `t3code-gpui://` links the OS opens (Info.plist registers the scheme on macOS).
    let (open_urls, mut opened_urls) = futures::channel::mpsc::unbounded::<String>();
    let app = gpui_kit::application().with_assets(t3code_web::Assets);
    app.on_open_urls(move |urls| {
        for url in urls {
            open_urls.unbounded_send(url).ok();
        }
    });
    app.run(move |cx| {
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
        let app_root = root.app.clone();
        cx.spawn(async move |cx| {
            use futures::StreamExt as _;
            while let Some(url) = opened_urls.next().await {
                let route = window::route_from_url(&url);
                if let Some(route) = route {
                    cx.update(|cx| app_root.update(cx, |root, cx| root.navigate(route, cx)));
                }
            }
        })
        .detach();
        notifications::install(cx);
        handle
            .update(cx, |_, window, cx| {
                t3code_web::components::thread_notification_coordinator::ThreadNotificationCoordinator::watch_window(window, cx)
            })
            .ok();
        cx.activate(true);
    });
}
