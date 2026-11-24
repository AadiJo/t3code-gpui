mod import;
mod menu;
mod notifications;
mod window;

fn main() {
    gpui_kit::application()
        .with_assets(t3code_web::Assets)
        .run(|cx| {
            t3code_web::init(cx);
            let data_dir = runtime_config_dir();
            let launch = import::before_settings(&data_dir, window::data_dir().as_deref());
            if let Err(error) = t3code_web::connection::ConnectionRuntime::init(runtime_config(&launch), cx) {
                eprintln!("could not start the connection runtime: {error:#}");
                cx.quit();
                return;
            }
            t3code_web::hooks::load(&data_dir, cx);
            import::apply_client_settings(&launch, cx);
            let options = window::main_window_options(cx);
            // `T3CODE_FIXTURES=1` shows the showcase data instead of the saved environments.
            let fixtures = std::env::var("T3CODE_FIXTURES").is_ok_and(|v| v == "1");
            let (handle, root) = window::open_main_window(options, fixtures, cx).expect("open main window");
            menu::install(root.app.clone(), cx);
            notifications::install(cx);
            handle
                .update(cx, |_, window, cx| {
                    t3code_web::components::thread_notification_coordinator::ThreadNotificationCoordinator::watch_window(window, cx)
                })
                .ok();
            import::import_connections(launch, handle, cx);
            cx.activate(true);
        });
}

/// Where the app keeps its catalog, caches and settings: the platform data directory, or
/// `T3CODE_DATA_DIR` for an isolated run (checks, a second profile).
fn runtime_config_dir() -> std::path::PathBuf {
    std::env::var_os("T3CODE_DATA_DIR").map(std::path::PathBuf::from).unwrap_or_else(t3code_client_runtime::platform::default_data_dir)
}

/// The app's runtime config. An isolated data directory keeps its secrets in a file there
/// instead of the OS keychain, so it never touches the real profile's items.
fn runtime_config(launch: &import::Launch) -> t3code_web::connection::RuntimeConfig {
    let mut config = t3code_web::connection::RuntimeConfig::for_app();
    // The install the launch import read is the one the catalog import reads.
    config.electron = launch.install.as_ref().map(|install| t3code_web::connection::ElectronImportSource { paths: install.paths.clone(), key: None });
    match std::env::var_os("T3CODE_DATA_DIR") {
        Some(dir) => {
            let dir = std::path::PathBuf::from(dir);
            t3code_web::connection::RuntimeConfig {
                secrets: Some(std::sync::Arc::new(t3code_client_runtime::platform::FileSecretStore::new(dir.join("secrets.json")))),
                data_dir: dir,
                ..config
            }
        }
        None => config,
    }
}
