//! The Files tab, go to file, content search and the media viewer on a server workspace:
//! [`WorkspaceFiles`] over `projects.*` and signed asset URLs.

use std::path::PathBuf;

use futures::FutureExt as _;
use t3code_contracts::rpc::methods::{AssetsCreateUrl, ProjectsListEntries, ProjectsReadFile, ProjectsSearchContents, ProjectsSearchEntries, ProjectsWriteFile};
use t3code_contracts::{
    AssetCreateUrlInput, AssetResource, AssetResourceWorkspaceFile, ProjectListEntriesInput, ProjectListEntriesResult,
    ProjectReadFileInput, ProjectReadFileResult, ProjectSearchContentsInput, ProjectSearchContentsResult, ProjectSearchEntriesInput,
    ProjectSearchEntriesResult, ProjectWriteFileInput, ProjectWriteFileResult, ThreadId,
};

use super::rpc::RpcLink;
use crate::file_source::{AssetBytes, ContentQuery, FileError, FileFuture, WorkspaceFiles};

/// One workspace directory on one server, reached through a thread that works in it (asset
/// URLs are scoped to a thread).
pub struct LiveWorkspaceFiles {
    link: RpcLink,
    cwd: String,
    thread_id: Option<ThreadId>,
}

impl LiveWorkspaceFiles {
    pub fn new(link: RpcLink, cwd: String, thread_id: Option<ThreadId>) -> Self {
        Self { link, cwd, thread_id }
    }

    fn map<T: Send + 'static>(future: futures::future::BoxFuture<'static, Result<T, gpui_kit::SharedString>>) -> FileFuture<T> {
        future.map(|result| result.map_err(|message| FileError::new(message.to_string()))).boxed()
    }
}

impl WorkspaceFiles for LiveWorkspaceFiles {
    fn root(&self) -> &str {
        &self.cwd
    }

    fn list_entries(&self, directory: Option<String>) -> FileFuture<ProjectListEntriesResult> {
        Self::map(self.link.call::<ProjectsListEntries>(ProjectListEntriesInput { cwd: self.cwd.clone(), directory_path: directory }))
    }

    fn read_file(&self, relative_path: String) -> FileFuture<ProjectReadFileResult> {
        Self::map(self.link.call::<ProjectsReadFile>(ProjectReadFileInput { cwd: self.cwd.clone(), relative_path, allow_missing: None }))
    }

    fn write_file(&self, relative_path: String, contents: String) -> FileFuture<ProjectWriteFileResult> {
        Self::map(self.link.call::<ProjectsWriteFile>(ProjectWriteFileInput { cwd: self.cwd.clone(), relative_path, contents }))
    }

    fn search_entries(&self, query: String, limit: u64) -> FileFuture<ProjectSearchEntriesResult> {
        let input = ProjectSearchEntriesInput { cwd: self.cwd.clone(), query, limit, kind: None, image_only: None };
        Self::map(self.link.call::<ProjectsSearchEntries>(input))
    }

    fn search_contents(&self, query: ContentQuery) -> FileFuture<ProjectSearchContentsResult> {
        let input = ProjectSearchContentsInput {
            cwd: self.cwd.clone(),
            query: query.query,
            limit: query.limit,
            case_sensitive: query.case_sensitive,
            whole_word: query.whole_word,
            use_regex: query.use_regex,
        };
        Self::map(self.link.call::<ProjectsSearchContents>(input))
    }

    fn read_asset(&self, relative_path: String) -> FileFuture<AssetBytes> {
        let Some(thread_id) = self.thread_id.clone() else {
            return async { Err(FileError::new("Files open through a thread on this server.")) }.boxed();
        };
        let link = self.link.clone();
        Self::map(self.link.run(async move {
            let resource = AssetResource::WorkspaceFile(AssetResourceWorkspaceFile { thread_id, path: relative_path });
            let session = link.session().await?;
            let url = session
                .client
                .call::<AssetsCreateUrl>(AssetCreateUrlInput { resource })
                .await
                .map_err(|error| super::rpc::error_message(&error))?;
            let bytes = link
                .link
                .fetch_relative(reqwest::Method::GET, &url.relative_url, None)
                .await
                .map_err(|error| gpui_kit::SharedString::from(error.to_string()))?;
            Ok(AssetBytes { bytes: bytes.into(), revision: url.file_revision })
        }))
    }

    fn local_copy(&self, relative_path: String) -> FileFuture<PathBuf> {
        let name = relative_path.rsplit('/').next().unwrap_or("file").to_owned();
        let asset = self.read_asset(relative_path);
        async move {
            let asset = asset.await?;
            let dir = std::env::temp_dir().join("t3code-gpui-open").join(uuid::Uuid::new_v4().simple().to_string());
            std::fs::create_dir_all(&dir).map_err(|error| FileError::new(error.to_string()))?;
            let path = dir.join(name);
            std::fs::write(&path, &asset.bytes).map_err(|error| FileError::new(error.to_string()))?;
            Ok(path)
        }
        .boxed()
    }
}

/// `openFileInPreview`: `assets.createUrl` for a workspace file, then `preview.open` on the
/// server's URL for it. Resolves with the preview tab's id.
pub fn open_file_in_preview(target: &crate::router::ThreadTarget, absolute_path: String, cx: &gpui_kit::App) -> gpui_kit::Task<Result<gpui_kit::SharedString, gpui_kit::SharedString>> {
    use t3code_contracts::rpc::methods::PreviewOpen;
    let crate::router::ThreadTarget::Server(key) = target else {
        return gpui_kit::Task::ready(Err(crate::i18n::t!("The integrated browser is unavailable in this runtime.")));
    };
    let (environment_id, thread_id) = crate::state::shell::runtime_ids(key);
    let Some(link) = RpcLink::new(&environment_id, cx) else {
        return gpui_kit::Task::ready(Err(crate::i18n::t!("This environment is not connected.")));
    };
    let run = link.clone();
    let future = link.run(async move {
        let session = run.session().await?;
        let resource = AssetResource::WorkspaceFile(AssetResourceWorkspaceFile { thread_id: thread_id.clone(), path: absolute_path });
        let asset = session.client.call::<AssetsCreateUrl>(AssetCreateUrlInput { resource }).await.map_err(|error| super::rpc::error_message(&error))?;
        let url = session
            .prepared
            .http_base_url
            .join(&asset.relative_url)
            .map_err(|_| gpui_kit::SharedString::from("The environment returned an invalid asset URL."))?;
        let input = t3code_contracts::PreviewOpenInput {
            thread_id,
            url: Some(url.to_string()),
            viewport: None,
            profile_id: None,
            runtime: None,
            color_scheme: None,
            zoom_factor: None,
            reveal: None,
        };
        let snapshot = session.client.call::<PreviewOpen>(input).await.map_err(|error| super::rpc::error_message(&error))?;
        Ok(gpui_kit::SharedString::from(snapshot.tab_id.to_string()))
    });
    cx.background_executor().spawn(future)
}
