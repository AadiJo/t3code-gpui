//! The thread details card's Lineage and Side chats sections on live data: the shown thread's
//! shells, projection, providers and projects, the tag filter's hidden threads, the titles of
//! related threads the shell does not list (`orchestration.getThreadTitles`), and the stop,
//! remote and delegated-from actions (`ThreadRelationshipsPanel`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::{App, Entity, SharedString, Task};
use t3code_contracts::rpc::methods::{OrchestrationGetThreadTitles, OrchestrationStopWorkflow};
use t3code_contracts::{EnvironmentId, NodeId, OrchestrationV2GetThreadTitlesInput, OrchestrationV2StopWorkflowInput, ThreadId};

use super::rpc::RpcLink;
use super::thread_details::{DetailsChanged, DetailsFeed};
use crate::components::chat::thread_details_relationships::RelationshipEvent;
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::state::StateBridge;
use crate::state::app_data::DetailsOutcome;
use crate::state::shell::{runtime_ids, thread_key};
use crate::state::thread_relationships::{RelationshipSources, RelationshipsInput, TitleLookup, derive};

/// The parent of a provider subagent thread (`lineage.relationshipToParent === "subagent"`) and
/// its title, "Parent thread" while the shell does not list it, for the header's parent crumb.
pub fn parent_subagent_crumb(key: &ThreadKey, cx: &App) -> Option<(ThreadKey, SharedString)> {
    use t3code_contracts::OrchestrationV2AppThreadLineageRelationshipToParent as Relationship;
    let (environment_id, thread_id) = runtime_ids(key);
    let snapshot = StateBridge::environment(&environment_id, cx)?.read(cx).shell.snapshot.clone()?;
    let shell = snapshot.threads.iter().find(|t| t.id == thread_id)?;
    if shell.lineage.relationship_to_parent != Some(Relationship::Subagent) {
        return None;
    }
    let parent_id = shell.lineage.parent_thread_id.clone()?;
    let title = snapshot.threads.iter().find(|t| t.id == parent_id).map_or_else(|| t!("Parent thread"), |p| p.title.clone().into());
    Some((thread_key(&environment_id, &parent_id), title))
}

/// What the sections keep between reads: looked-up titles and the stop in flight.
#[derive(Default)]
pub struct RelationshipsState {
    titles: HashMap<(EnvironmentId, ThreadId), TitleLookup>,
    asked: HashSet<(EnvironmentId, ThreadId)>,
    /// The subagent or workflow whose stop is waiting on the server.
    stopping: Option<SharedString>,
}

/// The Lineage and Side chats rows for `key`. Asks the server once for the titles of rows the
/// shell does not list; the card re-reads when they arrive.
pub fn relationships(feed: &Entity<DetailsFeed>, key: &ThreadKey, config: Option<&t3code_contracts::ServerConfig>, cx: &mut App) -> Option<RelationshipsInput> {
    let (environment_id, thread_id) = runtime_ids(key);
    let environment = StateBridge::environment(&environment_id, cx)?;
    let snapshot = environment.read(cx).shell.snapshot.clone()?;
    let thread = StateBridge::threads(cx).read(cx).find(key, cx).cloned();
    let projection = thread.as_ref().and_then(|t| t.read(cx).projection().cloned());
    let shells: Vec<_> = snapshot.threads.iter().chain(snapshot.archived_threads.iter()).collect();
    let providers: Vec<_> = config.map(|c| c.providers.iter().cloned().map(Arc::new).collect()).unwrap_or_default();
    let entries = crate::provider_instances::derive_provider_instance_entries(&providers);
    let environments: Vec<_> = StateBridge::environments(cx)
        .read(cx)
        .list
        .iter()
        .filter_map(|e| {
            let env = e.read(cx);
            let config = env.config.clone()?;
            Some((env.environment_id().clone(), SharedString::from(config.environment.label.clone()), config))
        })
        .collect();
    let (hidden, hiding) = {
        let shell = StateBridge::shell(cx).read(cx);
        let (hidden, hiding) = shell.hidden();
        (hidden.clone(), hiding)
    };
    let state = &feed.read(cx).relationships;
    let titles = |id: &ThreadId| state.titles.get(&(environment_id.clone(), id.clone())).cloned().unwrap_or(TitleLookup::Unsettled);
    let mut input = derive(RelationshipSources {
        thread_id: &thread_id,
        live_shells: snapshot.threads.len(),
        shells,
        projection: projection.as_deref(),
        projects: snapshot.projects.iter().map(|p| &**p).collect(),
        providers: &providers,
        provider_entries: &entries,
        remote_provider: &|environment, instance| {
            let (_, _, config) = environments.iter().find(|(e, _, _)| e == environment)?;
            config.providers.iter().find(|p| p.instance_id == instance).cloned().map(Arc::new)
        },
        is_hidden: &|id| hidden.contains(&(environment_id.clone(), id.clone())),
        hiding_anywhere: hiding,
        show_lineage: true,
        show_side_chats: true,
        // The server is authoritative; a client without the scope gets its rejection.
        can_operate_workflow: true,
        connected_label: &|id| environments.iter().find(|(e, _, _)| e == id).map(|(_, label, _)| label.clone()),
        titles: &titles,
    })?;
    input.stopping = state.stopping.clone();
    let ask: Vec<ThreadId> = input.unlisted.iter().filter(|id| !state.asked.contains(&(environment_id.clone(), (*id).clone()))).cloned().collect();
    if !ask.is_empty() {
        ask_titles(feed, key, &environment_id, ask, cx);
    }
    Some(input)
}

/// `orchestration.getThreadTitles` for `ids`; a failed read leaves them unsettled.
fn ask_titles(feed: &Entity<DetailsFeed>, key: &ThreadKey, environment: &EnvironmentId, ids: Vec<ThreadId>, cx: &mut App) {
    let Some(link) = RpcLink::new(environment, cx) else { return };
    feed.update(cx, |feed, _| feed.relationships.asked.extend(ids.iter().map(|id| (environment.clone(), id.clone()))));
    let request = link.call::<OrchestrationGetThreadTitles>(OrchestrationV2GetThreadTitlesInput { thread_ids: ids.clone() });
    let (feed, key, environment) = (feed.clone(), key.clone(), environment.clone());
    cx.spawn(async move |cx| {
        let Ok(found) = request.await else { return };
        cx.update(|cx| {
            feed.update(cx, |feed, cx| {
                for id in ids {
                    let lookup = found.get(id.as_str()).map_or(TitleLookup::Absent, |title| TitleLookup::Found(title.clone().into()));
                    feed.relationships.titles.insert((environment.clone(), id), lookup);
                }
                cx.emit(DetailsChanged(key));
            })
        });
    })
    .detach();
}

fn toast(kind: ToastKind, title: SharedString, cx: &mut App) {
    Toaster::push(ToastData::new(kind, title), cx);
}

/// Marks `id` as stopping (or done) and has the card re-read.
fn set_stopping(feed: &Entity<DetailsFeed>, key: &ThreadKey, id: Option<SharedString>, cx: &mut App) {
    feed.update(cx, |feed, cx| {
        feed.relationships.stopping = id;
        cx.emit(DetailsChanged(key.clone()));
    });
}

/// Runs the actions this module owns: stop a subagent or workflow, open a remote task, open the
/// delegating thread after checking it still exists.
pub fn run(feed: &Entity<DetailsFeed>, key: &ThreadKey, event: &RelationshipEvent, cx: &mut App) -> Task<DetailsOutcome> {
    let (environment_id, thread_id) = runtime_ids(key);
    match event {
        RelationshipEvent::StopSubagent { owner, subagent } => {
            if feed.read(cx).relationships.stopping.is_some() {
                return Task::ready(DetailsOutcome::Done);
            }
            let Some(environment) = StateBridge::environment(&environment_id, cx) else { return Task::ready(DetailsOutcome::Done) };
            let commands = environment.read(cx).commands.clone();
            let (owner, subagent) = (owner.clone(), subagent.clone());
            let id = SharedString::from(subagent.to_string());
            let task = StateBridge::spawn(async move { commands.interrupt(&owner, None, Some(subagent)).await.map(|_| ()) }, cx);
            finish_stop(feed, key, id, task, t!("Could not stop subagent"), cx)
        }
        RelationshipEvent::StopWorkflow(subagent) => {
            if feed.read(cx).relationships.stopping.is_some() {
                return Task::ready(DetailsOutcome::Done);
            }
            let Some(link) = RpcLink::new(&environment_id, cx) else { return Task::ready(DetailsOutcome::Done) };
            let id = SharedString::from(subagent.to_string());
            let request = link.call::<OrchestrationStopWorkflow>(OrchestrationV2StopWorkflowInput { thread_id, subagent_id: NodeId::from(subagent.to_string()) });
            let task = cx.background_executor().spawn(async move { Some(request.await.map(|_| ())) });
            finish_stop(feed, key, id, task, t!("Could not stop workflow"), cx)
        }
        RelationshipEvent::OpenRemote { environment, thread } => {
            if StateBridge::environment(environment, cx).is_some() {
                crate::state::open_thread(thread_key(environment, thread), cx);
            }
            Task::ready(DetailsOutcome::Done)
        }
        RelationshipEvent::OpenDelegatedFrom => open_delegated_from(feed, key, cx),
        _ => Task::ready(DetailsOutcome::Done),
    }
}

/// Shows the stop as busy until `task` ends, with an error toast on failure.
fn finish_stop<E: std::fmt::Display + 'static>(
    feed: &Entity<DetailsFeed>,
    key: &ThreadKey,
    id: SharedString,
    task: Task<Option<Result<(), E>>>,
    failure: SharedString,
    cx: &mut App,
) -> Task<DetailsOutcome> {
    set_stopping(feed, key, Some(id), cx);
    let (feed, key) = (feed.clone(), key.clone());
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            set_stopping(&feed, &key, None, cx);
            if !matches!(result, Some(Ok(()))) {
                toast(ToastKind::Error, failure, cx);
            }
        });
        DetailsOutcome::Done
    })
}

/// `DelegatedFromRow`'s `openParent`: the parent may have been deleted on its environment since
/// delegation, and the shell cannot tell that from never loaded, so its environment is asked.
fn open_delegated_from(feed: &Entity<DetailsFeed>, key: &ThreadKey, cx: &mut App) -> Task<DetailsOutcome> {
    let Some(from) = StateBridge::shell(cx).read(cx).row(key).and_then(|row| row.source.delegated_from.clone()) else {
        return Task::ready(DetailsOutcome::Done);
    };
    let Some(link) = RpcLink::new(&from.environment_id, cx) else { return Task::ready(DetailsOutcome::Done) };
    let request = link.call::<OrchestrationGetThreadTitles>(OrchestrationV2GetThreadTitlesInput { thread_ids: vec![from.thread_id.clone()] });
    let (feed, key) = (feed.clone(), key.clone());
    cx.spawn(async move |cx| {
        let result = request.await;
        cx.update(|cx| {
            // The read can outlast the user's stay on this thread; a late answer must not pull them back.
            if feed.read(cx).shown() != Some(&key) {
                return;
            }
            match result {
                Err(_) => toast(ToastKind::Error, t!("Could not open the parent thread"), cx),
                Ok(titles) if !titles.contains_key(from.thread_id.as_str()) => toast(ToastKind::Info, t!("The parent thread was deleted"), cx),
                Ok(_) => crate::state::open_thread(thread_key(&from.environment_id, &from.thread_id), cx),
            }
        });
        DetailsOutcome::Done
    })
}
