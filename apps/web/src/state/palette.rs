//! The command palette over live threads and projects (`CommandPalette.tsx` data,
//! `commandPaletteStore.ts` handlers). [`LivePaletteSource`] is refreshed from
//! [`LiveShell`] by [`LivePaletteSource::refresh`]; a palette thread keeps its `Arc` while its
//! sidebar row's `Arc` is unchanged, so a refresh after one thread changed builds one search
//! subject.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, SharedString};

use super::shell::LiveShell;
use super::sidebar_actions::{SidebarOutcome, handle as handle_sidebar};
use crate::components::command_palette_logic::PaletteEvent;
use crate::components::command_palette_lookups::{
    CommandPaletteSource, PaletteEnvironment, PaletteFeature, PalettePort, PaletteProject, PaletteSetting, PaletteTheme, PaletteThread,
};
use crate::components::sidebar::SidebarEvent;
use crate::components::sidebar::model::{EnvironmentId, ProjectRef, SidebarThread, ThreadKey};
use crate::theme_palette::{Appearance, ThemeDocument};
use t3code_client_runtime::state::models::ThreadRow;

#[derive(Default)]
struct PaletteData {
    revision: u64,
    threads: Arc<[Arc<PaletteThread>]>,
    projects: Arc<[PaletteProject]>,
    active_thread: Option<ThreadKey>,
    /// The active thread links pull requests or issues.
    active_has_links: bool,
    environments: Vec<PaletteEnvironment>,
    now_ms: i64,
    memo: HashMap<ThreadKey, (Arc<SidebarThread>, Arc<PaletteThread>)>,
    mentions: Arc<[Arc<MentionCandidate>]>,
    mention_memo: HashMap<ThreadKey, (Arc<ThreadRow>, Arc<MentionCandidate>)>,
}

/// A thread the composer's `@` menu may reference (`ComposerThreadCandidate`): every listed
/// thread and every side chat, not archived. Side chats are offered only from their own parent.
pub struct MentionCandidate {
    pub key: ThreadKey,
    pub title: SharedString,
    /// The thread it is a side chat of, if it is one.
    pub side_chat_of: Option<ThreadKey>,
    /// The user wrote in it; an unused side chat is deleted on close, so it is not offered.
    pub has_user_message: bool,
    /// Searched without the project title, as the composer searches.
    pub subject: t3code_shared::palette_search::ThreadSearchSubject,
}

impl MentionCandidate {
    fn new(row: &ThreadRow) -> Self {
        let source = &row.source;
        let key = super::shell::thread_key(&row.environment_id, row.id());
        let mut subject = t3code_shared::palette_search::ThreadSearchSubject::new(row.title()).with_branch(source.branch.as_deref());
        subject.id = Some(Arc::clone(&source.id.0));
        subject.activity_at = row.latest_user_message_at.or(row.updated_at).or(row.created_at);
        let side_chat_of = (source.lineage.side_chat == Some(true))
            .then(|| source.lineage.parent_thread_id.as_ref().map(|parent| super::shell::thread_key(&row.environment_id, parent)))
            .flatten();
        Self { key, title: SharedString::from(row.title().to_owned()), side_chat_of, has_user_message: row.latest_user_message_at.is_some(), subject }
    }
}

/// Palette data from the live shell.
#[derive(Default)]
pub struct LivePaletteSource {
    data: RefCell<PaletteData>,
}

impl LivePaletteSource {
    pub fn new() -> Rc<Self> {
        Rc::default()
    }

    /// The threads `@` may reference, as of the last refresh.
    pub fn mention_candidates(&self) -> Arc<[Arc<MentionCandidate>]> {
        self.data.borrow().mentions.clone()
    }

    /// Takes the shell's current rows if they changed since the last refresh.
    pub fn refresh(&self, shell: &LiveShell, cx: &App) {
        let mut data = self.data.borrow_mut();
        if data.revision == shell.revision() && data.revision != 0 {
            return;
        }
        let snapshot = shell.snapshot();
        let projects: Arc<[PaletteProject]> = snapshot
            .projects
            .iter()
            .filter(|p| !p.archived)
            .map(|p| PaletteProject {
                project_ref: p.project_ref.clone(),
                title: p.title.clone(),
                workspace_root: p.workspace_root.clone(),
                pinned: p.pinned,
            })
            .collect();
        let mut memo = std::mem::take(&mut data.memo);
        let mut next = HashMap::with_capacity(snapshot.threads.len());
        let threads: Arc<[Arc<PaletteThread>]> = snapshot
            .threads
            .iter()
            .filter(|thread| !thread.archived)
            .map(|thread| {
                let palette = match memo.remove(&thread.key) {
                    Some((source, palette)) if Arc::ptr_eq(&source, thread) => palette,
                    _ => {
                        let project = projects.iter().find(|p| p.project_ref == thread.project_ref());
                        Arc::new(PaletteThread::from_sidebar(thread, project.map(|p| p.title.as_ref()), None, None))
                    }
                };
                next.insert(thread.key.clone(), (Arc::clone(thread), Arc::clone(&palette)));
                palette
            })
            .collect();
        data.memo = next;
        data.threads = threads;
        let mut memo = std::mem::take(&mut data.mention_memo);
        let mut next = HashMap::with_capacity(memo.len());
        let mentions: Arc<[Arc<MentionCandidate>]> = shell
            .presented()
            .iter()
            .filter(|row| row.source.archived_at.is_none() && (row.source.lineage.is_listed() || row.source.lineage.side_chat == Some(true)))
            .map(|row| {
                let key = super::shell::thread_key(&row.environment_id, row.id());
                let candidate = match memo.remove(&key) {
                    Some((source, candidate)) if Arc::ptr_eq(&source, row) => candidate,
                    _ => Arc::new(MentionCandidate::new(row)),
                };
                next.insert(key, (Arc::clone(row), Arc::clone(&candidate)));
                candidate
            })
            .collect();
        data.mention_memo = next;
        data.mentions = mentions;
        data.projects = projects;
        data.active_thread = snapshot.active_thread.clone();
        data.active_has_links = snapshot.active_thread.as_ref().and_then(|key| shell.row(key)).is_some_and(|row| {
            row.source.pull_requests.as_ref().is_some_and(|links| !links.is_empty()) || row.source.issues.as_ref().is_some_and(|issues| !issues.is_empty())
        });
        data.now_ms = snapshot.now.0;
        data.environments = shell
            .environments(cx)
            .iter()
            .map(|env| {
                let env = env.read(cx);
                PaletteEnvironment {
                    id: EnvironmentId(SharedString::from(Arc::clone(&env.environment_id().0))),
                    label: env.config.as_ref().map(|c| SharedString::from(c.environment.label.clone())).unwrap_or_default(),
                    connected: env.store.session().is_some(),
                }
            })
            .collect();
        data.revision = shell.revision();
    }
}

impl CommandPaletteSource for LivePaletteSource {
    fn threads(&self) -> Arc<[Arc<PaletteThread>]> {
        self.data.borrow().threads.clone()
    }

    fn projects(&self) -> Arc<[PaletteProject]> {
        self.data.borrow().projects.clone()
    }

    fn active_thread(&self) -> Option<ThreadKey> {
        self.data.borrow().active_thread.clone()
    }

    fn current_project(&self) -> Option<ProjectRef> {
        let data = self.data.borrow();
        let active = data.active_thread.as_ref()?;
        data.threads.iter().find(|t| &t.key == active).and_then(|t| t.project.clone())
    }

    fn environments(&self) -> Vec<PaletteEnvironment> {
        self.data.borrow().environments.clone()
    }

    fn themes(&self) -> Vec<PaletteTheme> {
        let graphite = Arc::new(ThemeDocument::graphite());
        let swatch = |appearance| {
            let palette = graphite.palette(appearance);
            (appearance, [palette.canvas, palette.accent])
        };
        vec![PaletteTheme {
            id: Some(graphite.id.clone().into()),
            label: graphite.name.clone().into(),
            previews: vec![swatch(Appearance::Light), swatch(Appearance::Dark)],
            document: graphite,
        }]
    }

    fn ports(&self) -> Vec<PalettePort> {
        Vec::new()
    }

    fn settings(&self) -> Arc<[PaletteSetting]> {
        Arc::from([])
    }

    fn supports(&self, feature: PaletteFeature) -> bool {
        matches!(feature, PaletteFeature::NewProject | PaletteFeature::ProjectSettings)
    }

    fn active_thread_has_linked_items(&self) -> bool {
        self.data.borrow().active_has_links
    }

    fn now_ms(&self) -> i64 {
        self.data.borrow().now_ms
    }
}

/// What the app must do after a palette event the bridge cannot finish alone.
pub type PaletteOutcome = SidebarOutcome;

/// Handles a palette event: opening a thread goes through the same path as a sidebar click.
pub fn handle(event: &PaletteEvent, cx: &mut App) -> PaletteOutcome {
    match event {
        PaletteEvent::OpenThread { thread, .. } => handle_sidebar(&SidebarEvent::ActivateThread(thread.clone()), cx),
        PaletteEvent::NewThreadIn(project) => SidebarOutcome::NewThread(Some(project.clone())),
        _ => SidebarOutcome::Done,
    }
}
