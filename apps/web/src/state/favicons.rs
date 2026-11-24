//! Project favicons on live data (`projectFaviconUrlAtom`): the server finds a project's icon
//! (`assets.createUrl` with a `project-favicon` resource); a URL ending in the missing marker
//! means none, and the project keeps its monogram. Fetched icons are written next to the app's
//! data, so rows load them as files, and each project asks once per session.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui_kit::App;
use t3code_contracts::rpc::methods::AssetsCreateUrl;
use t3code_contracts::{AssetCreateUrlInput, AssetResource, AssetResourceProjectFavicon, EnvironmentId};

use super::StateBridge;

/// The last path segment of a URL the server mints for a project without an icon.
const MISSING_MARKER: &str = "project-favicon-missing";

#[derive(Clone)]
enum Phase {
    Pending,
    Missing,
    Ready(Arc<Path>),
}

type Key = (EnvironmentId, String, Option<String>);

thread_local! {
    static FAVICONS: RefCell<HashMap<Key, Phase>> = RefCell::default();
}

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets where fetched icons are kept. Called once at bridge start.
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("project-favicons");
    _ = std::fs::create_dir_all(&dir);
    _ = DIR.set(dir);
}

/// The project's fetched favicon, if it has one; the first call starts the lookup and the shell
/// rebuilds when it lands.
pub fn favicon(environment_id: &EnvironmentId, workspace_root: &str, favicon_path: Option<&str>, cx: &mut App) -> Option<Arc<Path>> {
    let key: Key = (environment_id.clone(), workspace_root.to_owned(), favicon_path.map(str::to_owned));
    if let Some(phase) = FAVICONS.with_borrow(|map| map.get(&key).cloned()) {
        return match phase {
            Phase::Ready(path) => Some(path),
            _ => None,
        };
    }
    let link = crate::live::rpc::RpcLink::new(environment_id, cx)?;
    FAVICONS.with_borrow_mut(|map| map.insert(key.clone(), Phase::Pending));
    let target = DIR.get()?.join(file_stem(&key));
    let resource = AssetResource::ProjectFavicon(AssetResourceProjectFavicon { cwd: key.1.clone(), path: key.2.clone() });
    let runner = link.clone();
    let task = runner.run(async move {
        let session = link.session().await?;
        let url = session.client.call::<AssetsCreateUrl>(AssetCreateUrlInput { resource }).await.map_err(|e| crate::live::rpc::error_message(&e))?;
        let name = url.relative_url.split(['?', '#']).next().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_owned();
        if name == MISSING_MARKER {
            return Ok(None);
        }
        let bytes = link.link.fetch_relative(reqwest::Method::GET, &url.relative_url, None).await.map_err(|e| gpui_kit::SharedString::from(e.to_string()))?;
        let ext = extension(&name, url.source_path.as_deref(), &bytes);
        let path = target.with_extension(ext);
        std::fs::write(&path, &bytes).map_err(|e| gpui_kit::SharedString::from(e.to_string()))?;
        Ok(Some(path))
    });
    cx.spawn(async move |cx| {
        let phase = match task.await {
            Ok(Some(path)) => Phase::Ready(Arc::from(path.as_path())),
            Ok(None) => Phase::Missing,
            // A failed lookup (disconnected, older server) leaves the monogram until next launch.
            Err(_) => Phase::Missing,
        };
        let ready = matches!(phase, Phase::Ready(_));
        FAVICONS.with_borrow_mut(|map| map.insert(key, phase));
        if ready {
            cx.update(|cx| {
                if StateBridge::is_running(cx) {
                    StateBridge::shell(cx).update(cx, |shell, cx| shell.rebuild_now(cx));
                }
            });
        }
    })
    .detach();
    None
}

/// A stable file name for a project's icon.
fn file_stem(key: &Key) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in format!("{}\0{}\0{}", key.0, key.1, key.2.as_deref().unwrap_or_default()).bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The image type of a fetched icon: from its name, its source path or its bytes.
fn extension(name: &str, source: Option<&str>, bytes: &[u8]) -> &'static str {
    let from = |s: &str| {
        let lower = s.to_ascii_lowercase();
        ["svg", "png", "ico", "jpg", "jpeg", "gif", "webp", "bmp"].into_iter().find(|ext| lower.ends_with(&format!(".{ext}")))
    };
    from(name)
        .or_else(|| source.and_then(from))
        .or_else(|| {
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_ascii_lowercase();
            if head.contains("<svg") {
                return Some("svg");
            }
            image::guess_format(bytes).ok().and_then(|f| f.extensions_str().first().copied())
        })
        .unwrap_or("png")
}
