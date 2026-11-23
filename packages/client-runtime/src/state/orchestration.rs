//! Bodies the server withholds from thread streams, loaded on demand (`orchestration.ts`):
//! full turn items (command and dynamic tool output marked `outputOmitted`) and the live output of
//! an expanded command row.
//!
//! A settled item's answer never goes stale; a live one is refetched after 60 s. Finished
//! command output stays cached, so reopening a row does not refetch.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use t3code_contracts::rpc::methods::{OrchestrationGetTurnItem, OrchestrationSubscribeCommandOutput};
use t3code_contracts::{
    OrchestrationV2CommandOutputFrame, OrchestrationV2CommandOutputFrameKind, OrchestrationV2GetTurnItemInput, OrchestrationV2SubscribeCommandOutputInput,
    OrchestrationV2TurnItem, ThreadId, TurnItemId,
};
use t3code_shared::turn_item::TurnItemExt;
use tokio::sync::watch;

use super::environment::EnvironmentStore;

const LIVE_ITEM_STALE: Duration = Duration::from_secs(60);
/// At most this many fetched bodies stay cached per environment.
const ITEM_CACHE_LIMIT: usize = 256;

type Key = (ThreadId, TurnItemId);

struct Cached {
    item: Arc<OrchestrationV2TurnItem>,
    fetched_at: Instant,
}

/// Command output as it streams: the text so far and whether the command still runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandOutput {
    pub text: String,
    pub truncated: bool,
    pub running: bool,
}

/// Loaders for one environment's withheld bodies. Cheap to clone.
#[derive(Clone)]
pub struct Bodies {
    store: EnvironmentStore,
    items: Arc<Mutex<HashMap<Key, Cached>>>,
    finished_output: Arc<Mutex<HashMap<Key, Arc<CommandOutput>>>>,
}

impl Bodies {
    pub fn new(store: EnvironmentStore) -> Self {
        Self { store, items: Arc::default(), finished_output: Arc::default() }
    }

    /// The full item, with bodies a stream left out.
    pub async fn turn_item(&self, thread_id: &ThreadId, item_id: &TurnItemId) -> Option<Arc<OrchestrationV2TurnItem>> {
        let key = (thread_id.clone(), item_id.clone());
        if let Some(cached) = self.items.lock().expect("items lock").get(&key)
            && (!cached.item.is_work_active() || cached.fetched_at.elapsed() < LIVE_ITEM_STALE)
        {
            return Some(Arc::clone(&cached.item));
        }
        let session = self.store.session()?;
        let input = OrchestrationV2GetTurnItemInput { thread_id: thread_id.clone(), item_id: item_id.clone(), revision: None };
        let item = Arc::new(session.client.call::<OrchestrationGetTurnItem>(input).await.ok()?.item?);
        let mut items = self.items.lock().expect("items lock");
        if items.len() >= ITEM_CACHE_LIMIT
            && let Some(oldest) = items.iter().min_by_key(|(_, cached)| cached.fetched_at).map(|(key, _)| key.clone())
        {
            items.remove(&oldest);
        }
        items.insert(key, Cached { item: Arc::clone(&item), fetched_at: Instant::now() });
        Some(item)
    }

    /// Follows an expanded command row's output. The stream stops when the receiver and the
    /// returned guard are dropped, so a closed row receives nothing.
    pub fn command_output(&self, thread_id: &ThreadId, item_id: &TurnItemId) -> (watch::Receiver<Arc<CommandOutput>>, tokio::task::AbortHandle) {
        let key = (thread_id.clone(), item_id.clone());
        if let Some(done) = self.finished_output.lock().expect("output lock").get(&key) {
            let (_, receiver) = watch::channel(Arc::clone(done));
            return (receiver, tokio::spawn(async {}).abort_handle());
        }
        let (sender, receiver) = watch::channel(Arc::new(CommandOutput { running: true, ..CommandOutput::default() }));
        let store = self.store.clone();
        let finished = Arc::clone(&self.finished_output);
        let _runtime = store.context().runtime.enter();
        let task = tokio::spawn(async move {
            let Some(session) = store.session() else { return };
            let input = OrchestrationV2SubscribeCommandOutputInput { thread_id: key.0.clone(), item_id: key.1.clone() };
            let mut stream = session.client.subscribe::<OrchestrationSubscribeCommandOutput>(input);
            let mut output = CommandOutput { running: true, ..CommandOutput::default() };
            while let Some(Ok(frames)) = stream.next().await {
                for OrchestrationV2CommandOutputFrame { kind, text, truncated, running } in frames {
                    if kind == OrchestrationV2CommandOutputFrameKind::Replace {
                        output.text = text;
                    } else {
                        output.text.push_str(&text);
                    }
                    output.truncated = truncated;
                    output.running = running;
                }
                let value = Arc::new(output.clone());
                if !output.running {
                    finished.lock().expect("output lock").insert(key.clone(), Arc::clone(&value));
                }
                if sender.send(value).is_err() {
                    return;
                }
            }
        });
        (receiver, task.abort_handle())
    }
}
