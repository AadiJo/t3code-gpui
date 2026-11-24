//! The first-launch import from an existing T3 Code desktop install, in the order the app
//! starts: [`before_settings`] copies the unencrypted files before the settings stores load,
//! [`apply_client_settings`] lays the install's client settings over this app's once they
//! have, and [`import_connections`] reads the encrypted catalog and T3 Connect session once
//! the window shows a line explaining the Keychain prompt that read causes.

pub mod install;

use std::path::{Path, PathBuf};

use gpui_kit::{App, SharedString};
use t3code_client_runtime::platform::electron_import::SkipReason;
use t3code_web::components::ui::toast::{ToastData, ToastKind, Toaster};
use t3code_web::connection::ConnectionRuntime;
use t3code_web::hooks::use_settings::ClientSettingsStore;

use install::{FilesReport, ImportState, Install};

/// How long the Keychain notice shows before the read that prompts.
const NOTICE_LEAD: std::time::Duration = std::time::Duration::from_millis(250);

/// What the unencrypted half found, carried to the later steps.
pub struct Launch {
    pub install: Option<Install>,
    pub state: ImportState,
    pub report: FilesReport,
    pub data_dir: PathBuf,
}

/// Copies what needs no key. Call before `hooks::load`.
pub fn before_settings(data_dir: &Path, ui_dir: Option<&Path>) -> Launch {
    let mut state = ImportState::load(data_dir);
    let install = Install::detect();
    let report = match &install {
        Some(install) => install::import_files(install, data_dir, ui_dir, &mut state),
        None => FilesReport::default(),
    };
    for error in &report.errors {
        eprintln!("importing from the T3 Code install: {error}");
    }
    state.save(data_dir);
    Launch { install, state, report, data_dir: data_dir.to_owned() }
}

/// Applies the install's client settings over this app's (first launch, or the install's file
/// changed since the last launch). Call after `hooks::load`.
pub fn apply_client_settings(launch: &Launch, cx: &mut App) {
    let Some(path) = &launch.report.client_settings_path else { return };
    if let Err(error) = ClientSettingsStore::import_electron(path, cx) {
        eprintln!("applying the T3 Code install's client settings: {error:#}");
    }
}

fn toast(kind: ToastKind, title: &str, description: Option<String>, cx: &mut App) {
    let mut toast = ToastData::new(kind, SharedString::from(title.to_owned()));
    if let Some(description) = description {
        toast = toast.description(SharedString::from(description));
    }
    Toaster::push(toast, cx);
}

/// Reads the install's saved environments and T3 Connect session, once. Shows why macOS is
/// about to ask for the "T3 Code Safe Storage" Keychain item, and starts the read once it has
/// painted. Environments paired with the desktop app's own key come over without
/// a credential and show signed out in Settings → Environments until paired again.
pub fn import_connections(mut launch: Launch, cx: &mut App) {
    let Some(install) = launch.install.clone() else { return };
    if launch.state.connections_imported || !install.has_catalog() {
        return;
    }
    toast(
        ToastKind::Info,
        "Importing your T3 Code environments",
        Some(
            "macOS will ask to let this app read the \u{201c}T3 Code Safe Storage\u{201d} Keychain item. It holds the key \
             to the environments and T3 Connect sign-in saved by the T3 Code desktop app, which are read and never changed."
                .into(),
        ),
        cx,
    );
    // The read starts after the notice has had a frame to paint (one frame is ~16 ms).
    cx.spawn(async move |cx| {
        cx.background_executor().timer(NOTICE_LEAD).await;
        let preview = cx.update(|cx| ConnectionRuntime::global(cx).preview_electron_import(cx));
        let preview = preview.await;
        cx.update(|cx| finish(&mut launch, preview, cx));
    })
    .detach();
}

/// Saves what the preview found, then adopts the T3 Connect session.
fn finish(launch: &mut Launch, preview: Result<t3code_web::connection::ImportPreview, t3code_web::connection::CommandError>, cx: &mut App) {
    launch.state.connections_imported = true;
    launch.state.save(&launch.data_dir);
    let preview = match preview {
        Ok(preview) => preview,
        Err(error) => {
            toast(ToastKind::Error, "Could not import your T3 Code environments", Some(error.0.to_string()), cx);
            return;
        }
    };
    // Without T3 Connect configured there is no session to adopt.
    let has_clerk = preview.has_clerk_session && ConnectionRuntime::state(cx).read(cx).cloud.is_some();
    // Without a session to adopt, T3 Connect routes could only wait for a sign-in; signing in
    // later lists those environments again with Add, as the web does after a sign-out.
    let mut preview = preview;
    if !has_clerk {
        without_relay_routes(&mut preview.import);
    }
    let environments = preview.import.environments.len();
    let repair = preview
        .import
        .skipped
        .iter()
        .filter(|s| matches!(s.reason, SkipReason::DeviceBoundCredential | SkipReason::MissingCredential))
        .count();
    let runtime = ConnectionRuntime::global(cx);
    let saved = runtime.apply_electron_import(preview.clone(), false, cx);
    let adopt = has_clerk.then(|| runtime.apply_electron_import(preview, true, cx));
    let mut state = launch.state.clone();
    let data_dir = launch.data_dir.clone();
    cx.spawn(async move |cx| {
        let saved = saved.await;
        cx.update(|cx| match &saved {
            Ok(_) => {
                let pairing = (repair > 0).then(|| format!("{repair} need pairing again in Settings \u{2192} Environments."));
                toast(ToastKind::Success, &format!("Imported {environments} environments from T3 Code"), pairing, cx);
            }
            Err(error) => toast(ToastKind::Error, "Could not import your T3 Code environments", Some(error.0.to_string()), cx),
        });
        let Some(adopt) = adopt else { return };
        let adopted = adopt.await;
        cx.update(|cx| match adopted {
            Ok(_) if !state.clerk_notice_shown => {
                toast(ToastKind::Info, "T3 Connect now shares its sign-in with the T3 Code app, which may sign it out there.", None, cx);
                state.clerk_notice_shown = true;
                state.save(&data_dir);
            }
            Ok(_) => {}
            Err(error) => toast(
                ToastKind::Warning,
                "Sign in to T3 Connect again",
                Some(format!("The desktop app's session was not accepted ({}). Use Sign in to T3 Connect in Settings.", error.0)),
                cx,
            ),
        });
    })
    .detach();
}

/// Drops T3 Connect routes and the routes learned through them; an environment left with no
/// route is dropped too.
fn without_relay_routes(import: &mut t3code_client_runtime::platform::electron_import::ElectronImport) {
    use t3code_client_runtime::connection::catalog::{ProfileAuthorization, TargetRoute};
    for environment in &mut import.environments {
        environment.routes.retain(|route| {
            route.target.route != TargetRoute::Relay
                && route.bearer_profile().and_then(|profile| profile.authorization) != Some(ProfileAuthorization::T3Connect)
        });
    }
    import.environments.retain(|environment| !environment.routes.is_empty());
}
