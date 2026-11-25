//! What context menus in a thread can do on its server's machine: the file menu
//! (`fileContextMenu.ts`), and a message's link and file link menus (`ChatMarkdown.tsx`,
//! `MarkdownFileLink.tsx`). Every action runs on the host through `shell.openInEditor`,
//! `assets.createUrl` and `preview.open`, as the fork's do.

use std::rc::Rc;

use gpui_kit::{App, Context, Entity, SharedString, Window};
use t3code_contracts::rpc::methods::{AssetsCreateUrl, PreviewOpen, ShellOpenInEditor};
use t3code_contracts::{
    AssetCreateUrlInput, AssetResource, AssetResourceMediaFile, AssetResourceWorkspaceFile, EditorId, EnvironmentId,
    ExecutionEnvironmentPlatformOs, FileManagerRevealKind, LaunchEditorInput, PreviewOpenInput, ServerConfig, ThreadId,
};

use super::rpc::RpcLink;
use crate::components::chat::messages_timeline::MessagesTimeline;
use crate::components::chat_markdown::{MarkdownMenuAction, MarkdownMenus};
use crate::components::sidebar::model::ThreadKey;
use crate::components::chat_view::ChatView;
use crate::components::files::file_preview_panel::FilesPanelEvent;
use crate::components::right_panel_tabs::RightPanel;
use crate::components::thread_route_view::{ThreadRouteEvent, ThreadRouteView};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::editor_preferences::{LastEditor, effective_editor};
use crate::file_context_menu::{FileContextMenuAction, FileContextMenuCapabilities, RevealKind, editor_label, reveal_label};
use crate::i18n::t;
use crate::router::ThreadTarget;
use crate::state::StateBridge;
use crate::state::shell::runtime_ids;

fn config(environment: &EnvironmentId, cx: &App) -> Option<std::sync::Arc<ServerConfig>> {
    StateBridge::environment(environment, cx)?.read(cx).config.clone()
}

/// How the host reveals files, when it can (`shellRevealInFileManager` with a file manager):
/// the server's own wording, else its OS's.
fn reveal_kind(config: &ServerConfig) -> Option<RevealKind> {
    if config.shell_reveal_in_file_manager != Some(true) || !config.available_editors.contains(&EditorId::FileManager) {
        return None;
    }
    Some(match config.shell_reveal_in_file_manager_kind {
        Some(FileManagerRevealKind::Finder) => RevealKind::Finder,
        Some(FileManagerRevealKind::FileExplorer) => RevealKind::FileExplorer,
        Some(FileManagerRevealKind::Files) => RevealKind::Files,
        None => match config.environment.platform.os {
            ExecutionEnvironmentPlatformOs::Darwin => RevealKind::Finder,
            ExecutionEnvironmentPlatformOs::Windows => RevealKind::FileExplorer,
            _ => RevealKind::Files,
        },
    })
}

/// The file menu's rows on `environment`'s host (`useFileContextMenu` capabilities).
pub fn file_menu_capabilities(environment: &EnvironmentId, cx: &App) -> FileContextMenuCapabilities {
    let Some(config) = config(environment, cx) else { return FileContextMenuCapabilities::default() };
    FileContextMenuCapabilities { reveal: reveal_kind(&config), editor_ids: config.available_editors.clone() }
}

/// `openInEditorMenuLabel`: "Open in {editor}" for the preferred editor.
fn open_in_editor_label(editor: Option<EditorId>) -> SharedString {
    match editor {
        None | Some(EditorId::FileManager) => t!("Open in editor"),
        Some(editor) => t!("Open in {editor}", editor = editor_label(&editor)),
    }
}

/// What a message's menus may offer on `environment`: the integrated browser, the preferred
/// editor's label and the reveal label. Opening and revealing need the server to run on this
/// machine (`canUseMarkdownFileShellActions`: remote open mode `local-exec`).
pub fn markdown_menus(environment: &EnvironmentId, cx: &App) -> (bool, Option<SharedString>, Option<SharedString>) {
    let Some(config) = config(environment, cx) else { return (false, None, None) };
    let local = super::git::is_loopback(RpcLink::new(environment, cx).as_ref());
    let editor = local.then(|| open_in_editor_label(effective_editor(LastEditor::get(cx), &config.available_editors)));
    let reveal = local.then(|| reveal_kind(&config)).flatten().map(reveal_label);
    (true, editor, reveal)
}

/// Fills the timeline's message and changed-files menus for the shown thread. The route view
/// calls it whenever it shows or refreshes a thread; fixture data keeps the defaults.
pub fn sync_timeline(chat: &Entity<ChatView>, target: Option<&ThreadTarget>, cwd: &SharedString, cx: &mut App) {
    if !StateBridge::is_running(cx) {
        return;
    }
    let timeline = chat.read(cx).timeline().clone();
    let cwd = (!cwd.is_empty()).then(|| cwd.clone());
    let (environment, files) = match target {
        Some(ThreadTarget::Server(key)) => {
            let environment = runtime_ids(key).0;
            let files = file_menu_capabilities(&environment, cx);
            (Some(environment), files)
        }
        _ => (None, FileContextMenuCapabilities::default()),
    };
    let (preview, editor, reveal) = environment.map_or((false, None, None), |environment| markdown_menus(&environment, cx));
    timeline.update(cx, |timeline, cx| {
        timeline.set_markdown_menus(preview, editor, reveal, cwd, cx);
        timeline.set_file_menu(files, cx);
    });
}

/// `resolvePathLinkTarget`: a relative path against the working directory.
fn resolve(path: &str, cwd: Option<&str>) -> String {
    let path = path.trim_start_matches("./");
    match cwd {
        Some(cwd) if !path.starts_with('/') && !path.starts_with('~') => crate::file_source::absolute_path(cwd, path),
        _ => path.to_owned(),
    }
}

/// A file link's path with its `:line:column` suffix split off.
fn split_position(path: &str) -> (&str, Option<(u64, Option<u64>)>) {
    let mut parts = path.rsplitn(3, ':');
    let last = parts.next().and_then(|p| p.parse::<u64>().ok());
    let middle = parts.next();
    match (last, middle.and_then(|m| m.parse::<u64>().ok()), parts.next()) {
        (Some(column), Some(line), Some(file)) => (file, Some((line, Some(column)))),
        (Some(line), _, _) => (path.rsplit_once(':').map_or(path, |(file, _)| file), Some((line, None))),
        _ => (path, None),
    }
}

/// Runs one `shell.openInEditor` and toasts `failure` with the path when it fails.
fn open_on_host(link: RpcLink, input: LaunchEditorInput, failure: SharedString, cx: &mut App) {
    let path = input.cwd.clone();
    let request = link.call::<ShellOpenInEditor>(input);
    cx.spawn(async move |cx| {
        if let Err(error) = request.await {
            cx.update(|cx| _ = Toaster::push(ToastData::new(ToastKind::Error, failure).description(format!("{path}\n{error}")), cx));
        }
    })
    .detach();
}

fn launch(cwd: String, editor: EditorId, reveal: bool) -> LaunchEditorInput {
    LaunchEditorInput { cwd, editor, position: None, project_root: None, reveal: reveal.then_some(true) }
}

/// Where a menu row's result goes in the app: an opened browser tab, and media to preview.
#[derive(Clone)]
pub struct MenuRoutes {
    pub open_browser: Rc<dyn Fn(SharedString, &mut App)>,
    pub preview_media: Rc<dyn Fn(SharedString, &mut Window, &mut App)>,
}

/// Carries out a message menu row on `key`'s server thread, resolving file links against `cwd`.
pub fn run_markdown_action(key: &ThreadKey, cwd: Option<&str>, action: &MarkdownMenuAction, routes: &MenuRoutes, window: &mut Window, cx: &mut App) {
    if let MarkdownMenuAction::PreviewMedia(path) = action {
        return (routes.preview_media)(path.clone(), window, cx);
    }
    let (environment, thread) = runtime_ids(key);
    let Some(link) = RpcLink::new(&environment, cx) else { return };
    match action.clone() {
        MarkdownMenuAction::PreviewMedia(_) => {}
        MarkdownMenuAction::OpenInPreview(target) => {
            let task = open_in_preview(link, thread, &target, cwd, cx);
            let open = routes.open_browser.clone();
            cx.spawn(async move |cx| {
                if let Some(tab) = task.await {
                    cx.update(|cx| open(tab, cx));
                }
            })
            .detach();
        }
        MarkdownMenuAction::OpenFileInEditor(path) => {
            let Some(config) = config(&environment, cx) else { return };
            let Some(editor) = effective_editor(LastEditor::get(cx), &config.available_editors) else { return };
            LastEditor::set(editor, cx);
            let (file, position) = split_position(&path);
            let mut input = launch(resolve(file, cwd), editor, false);
            input.position = position.map(|(line, column)| t3code_contracts::LaunchEditorInputPosition { line: Some(line), column });
            input.project_root = cwd.map(str::to_owned);
            open_on_host(link, input, t!("Unable to open file"), cx);
        }
        MarkdownMenuAction::RevealFile(path) => {
            let file = split_position(&path).0;
            open_on_host(link, launch(resolve(file, cwd), EditorId::FileManager, true), t!("Unable to reveal file"), cx);
        }
        MarkdownMenuAction::File(action, path) => run_file_action(link, action, resolve(&path, cwd), cx),
    }
}

/// Carries out a timeline menu row for the shown server thread. Subscribed by the route view
/// to its timeline's [`MarkdownMenuAction`] events.
pub fn on_timeline_menu(view: &mut ThreadRouteView, timeline: &Entity<MessagesTimeline>, action: &MarkdownMenuAction, window: &mut Window, cx: &mut Context<ThreadRouteView>) {
    let Some(ThreadTarget::Server(key)) = view.target().cloned() else { return };
    let cwd = timeline.read(cx).markdown_cwd();
    let (browser, media) = (cx.weak_entity(), cx.weak_entity());
    let routes = MenuRoutes {
        open_browser: Rc::new(move |tab, cx| _ = browser.update(cx, |_, cx| cx.emit(ThreadRouteEvent::OpenBrowser(tab)))),
        preview_media: Rc::new(move |path, _, cx| _ = media.update(cx, |_, cx| cx.emit(ThreadRouteEvent::OpenLink(path)))),
    };
    run_markdown_action(&key, cwd.as_deref(), action, &routes, window, cx);
}

/// The message menus of a right panel surface (a pull request, an issue, a Markdown file) for
/// the panel's thread: what the host can do, and where results go in that panel.
pub fn right_panel_menus(panel: &mut RightPanel, target: Option<&ThreadTarget>, cx: &mut Context<RightPanel>) -> MarkdownMenus {
    let Some(ThreadTarget::Server(key)) = target.filter(|_| StateBridge::is_running(cx)).cloned() else { return MarkdownMenus::default() };
    let environment = runtime_ids(&key).0;
    let cwd = panel.workspace(cx).map(|workspace| workspace.files.root().to_owned());
    let (preview, editor_label, reveal_label) = markdown_menus(&environment, cx);
    let (browser, media) = (cx.weak_entity(), cx.weak_entity());
    let routes = MenuRoutes {
        open_browser: Rc::new(move |tab, cx| _ = browser.update(cx, |panel, cx| panel.open_browser_tab(&tab, cx))),
        preview_media: Rc::new(move |path, window, cx| _ = media.update(cx, |panel, cx| panel.open_file(path, None, window, cx))),
    };
    let menu_cwd = cwd.clone();
    MarkdownMenus {
        actions: Some(Rc::new(move |action, window, cx| run_markdown_action(&key, menu_cwd.as_deref(), &action, &routes, window, cx))),
        preview,
        editor_label,
        reveal_label,
        cwd: cwd.map(SharedString::from),
        files: file_menu_capabilities(&environment, cx),
    }
}

/// `useFileContextMenu().activate`: Open, Reveal or Open with on the host, for an absolute path.
pub fn run_file_action(link: RpcLink, action: FileContextMenuAction, absolute_path: String, cx: &mut App) {
    let (editor, reveal, failure) = match action {
        FileContextMenuAction::Open => (EditorId::FileManager, false, t!("Could not open file")),
        FileContextMenuAction::RevealInFolder => (EditorId::FileManager, true, t!("Unable to reveal file")),
        FileContextMenuAction::OpenWith(editor) => (editor, false, t!("Could not open in {editor}", editor = editor_label(&editor))),
        FileContextMenuAction::CopyMention | FileContextMenuAction::AddToChat => return,
    };
    open_on_host(link, launch(absolute_path, editor, reveal), failure, cx);
}

/// `openUrlInPreview` / `openFileInPreview`: a web link opens as is; a file first gets a signed
/// asset URL (a workspace file when it is inside the working directory, else a media file).
/// Resolves with the opened tab, after toasting a failure.
fn open_in_preview(link: RpcLink, thread: ThreadId, target: &str, cwd: Option<&str>, cx: &mut App) -> gpui_kit::Task<Option<SharedString>> {
    let web = target.starts_with("http://") || target.starts_with("https://");
    let (file, failure) = if web { (None, t!("Unable to open link in browser")) } else { (Some(split_position(target).0.to_owned()), t!("Unable to open file in browser")) };
    let cwd = cwd.map(str::to_owned);
    let url = target.to_owned();
    let request = link.clone().run(async move {
        let url = match file {
            None => url,
            Some(file) => {
                let absolute = resolve(&file, cwd.as_deref());
                let inside = cwd.as_deref().and_then(|cwd| absolute.strip_prefix(&format!("{}/", cwd.trim_end_matches('/'))).map(str::to_owned));
                let resource = match inside {
                    Some(path) => AssetResource::WorkspaceFile(AssetResourceWorkspaceFile { thread_id: thread.clone(), path }),
                    None => AssetResource::MediaFile(AssetResourceMediaFile { thread_id: thread.clone(), path: absolute, disposition: None, host_only: None }),
                };
                let session = link.session().await?;
                let asset = session.client.call::<AssetsCreateUrl>(AssetCreateUrlInput { resource }).await.map_err(|e| super::rpc::error_message(&e))?;
                session.prepared.http_base_url.join(&asset.relative_url).map_err(|e| SharedString::from(e.to_string()))?.to_string()
            }
        };
        let input = PreviewOpenInput {
            thread_id: thread,
            url: Some(url),
            viewport: None,
            profile_id: None,
            runtime: None,
            color_scheme: None,
            zoom_factor: None,
            reveal: None,
        };
        let session = link.session().await?;
        session.client.call::<PreviewOpen>(input).await.map_err(|e| super::rpc::error_message(&e))
    });
    cx.spawn(async move |cx| match request.await {
        Ok(snapshot) => Some(SharedString::from(snapshot.tab_id)),
        Err(error) => {
            cx.update(|cx| _ = Toaster::push(ToastData::new(ToastKind::Error, failure).description(error), cx));
            None
        }
    })
}

/// The file menu's rows for the right panel's thread (the Files tab and the diff headers).
pub fn target_file_menu(target: Option<&ThreadTarget>, cx: &App) -> FileContextMenuCapabilities {
    match target {
        Some(ThreadTarget::Server(key)) if StateBridge::is_running(cx) => file_menu_capabilities(&runtime_ids(key).0, cx),
        _ => FileContextMenuCapabilities::default(),
    }
}

/// Carries out the Files tab's host actions (its file menu, the diff header menu through it,
/// and "Open in browser" on an HTML file) for the right panel's thread.
pub fn on_files_event(panel: &mut RightPanel, target: Option<ThreadTarget>, event: &FilesPanelEvent, cx: &mut Context<RightPanel>) {
    let Some(ThreadTarget::Server(key)) = target else { return };
    let (environment, thread) = runtime_ids(&key);
    let Some(link) = RpcLink::new(&environment, cx) else { return };
    match event {
        FilesPanelEvent::OpenInEditor { absolute_path, editor, reveal } => {
            let action = match (editor, reveal) {
                (_, true) => FileContextMenuAction::RevealInFolder,
                (EditorId::FileManager, false) => FileContextMenuAction::Open,
                (editor, false) => FileContextMenuAction::OpenWith(*editor),
            };
            run_file_action(link, action, absolute_path.clone(), cx);
        }
        FilesPanelEvent::OpenInPreview { absolute_path } => {
            let root = panel.workspace(cx).map(|workspace| workspace.files.root().to_owned());
            let task = open_in_preview(link, thread, absolute_path, root.as_deref(), cx);
            cx.spawn(async move |this, cx| {
                if let Some(tab) = task.await {
                    _ = this.update(cx, |panel, cx| panel.open_browser_tab(&tab, cx));
                }
            })
            .detach();
        }
        FilesPanelEvent::AddToChat(_) => {}
    }
}
