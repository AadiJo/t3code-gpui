//! One thread's detail state as views read it (`threadState.ts`), and the wire decoding of thread
//! snapshots, stream items and history pages with their opt-in compactions.

use std::borrow::Cow;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;
use t3code_contracts::orchestration_v2_id_table::{Container, expand_record_ids};
use t3code_contracts::{OrchestrationV2DomainEvent, OrchestrationV2Subagent, OrchestrationV2TurnItem, RuntimeRequestId};
use t3code_shared::orchestration_v2_bounded_snapshot::bounded_snapshot_projection;
use t3code_shared::thread_projection::{ThreadProjection, VisibleTurnItem};

use super::environment::SyncStatus;
use super::thread_history_merge::HistoryMeta;

/// Where a thread's detail stands (`EnvironmentThreadStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    Empty,
    Cached,
    Synchronizing,
    Live,
    /// The server deleted the thread, or answered 404 (which can also mean "not created yet").
    Deleted,
}

impl From<SyncStatus> for ThreadStatus {
    fn from(status: SyncStatus) -> Self {
        match status {
            SyncStatus::Empty => Self::Empty,
            SyncStatus::Cached => Self::Cached,
            SyncStatus::Synchronizing => Self::Synchronizing,
            SyncStatus::Live => Self::Live,
        }
    }
}

/// One thread's detail (`EnvironmentThreadState`). `data` is shared: a view that kept the
/// previous `Arc<ThreadProjection>` can compare collections and rows by pointer.
#[derive(Debug, Clone)]
pub struct ThreadState {
    pub data: Option<Arc<ThreadProjection>>,
    pub status: ThreadStatus,
    pub error: Option<Arc<str>>,
    pub history: HistoryMeta,
    /// The last server sequence reflected in `data`, the resume cursor.
    pub sequence: u64,
}

impl ThreadState {
    pub fn empty() -> Self {
        Self { data: None, status: ThreadStatus::Empty, error: None, history: HistoryMeta::default(), sequence: 0 }
    }

    /// The status without a live stream.
    pub fn offline_status(&self) -> ThreadStatus {
        match (self.status, &self.data) {
            (ThreadStatus::Deleted, _) => ThreadStatus::Deleted,
            (_, Some(_)) => ThreadStatus::Cached,
            (_, None) => ThreadStatus::Empty,
        }
    }

    /// A state handed out from a cache (`cachedThreadState`): a live state with data stays live
    /// until the stream says otherwise; errors and loading flags are cleared.
    pub fn as_cached(&self) -> Self {
        let status = match self.status {
            ThreadStatus::Deleted => ThreadStatus::Deleted,
            ThreadStatus::Live if self.data.is_some() => ThreadStatus::Live,
            _ => self.offline_status(),
        };
        Self {
            status,
            error: None,
            history: HistoryMeta { loading: false, error: None, ..self.history.clone() },
            ..self.clone()
        }
    }

    /// Whether the server's reply at `sequence` has reached this state (`threadHasCaughtUpTo`).
    pub fn has_caught_up_to(&self, sequence: u64) -> bool {
        self.sequence >= sequence
    }
}

/// A bounded cache can lack the card of a pending question, which replay cannot restore; such a
/// projection is not reused (`hasMissingPendingQuestionCard`).
pub fn has_missing_pending_question_card(projection: &ThreadProjection) -> bool {
    use t3code_contracts::{OrchestrationV2RuntimeRequestKind as Kind, OrchestrationV2RuntimeRequestStatus as Status};
    let mut pending: Vec<&RuntimeRequestId> = projection
        .runtime_requests
        .iter()
        .filter(|request| request.kind == Kind::UserInput && request.status == Status::Pending)
        .map(|request| &request.id)
        .collect();
    if pending.is_empty() {
        return false;
    }
    for item in projection.turn_items.iter() {
        if let OrchestrationV2TurnItem::UserInputRequest(card) = &**item {
            pending.retain(|id| *id != &card.request_id);
            if pending.is_empty() {
                return false;
            }
        }
    }
    true
}

/// A restored snapshot: from the socket, the bounded HTTP route, or the cache.
#[derive(Debug, Clone)]
pub struct ThreadSnapshot {
    pub snapshot_sequence: u64,
    pub projection: ThreadProjection,
    /// Present when the snapshot is a bounded window with paging meta.
    pub history: Option<HistoryMeta>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotWire {
    snapshot_sequence: u64,
    projection: ThreadProjection,
    #[serde(default)]
    history_cursor: Option<String>,
    #[serde(default)]
    has_more_history: Option<bool>,
    #[serde(default)]
    latest_local_turn_ordinal: Option<u64>,
    #[serde(default)]
    turn_items_omit_local_visible: Option<t3code_contracts::wire::True>,
}

#[derive(Deserialize)]
struct Head<'a> {
    #[serde(borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(default)]
    sequence: Option<u64>,
    #[serde(default)]
    ids: Option<serde::de::IgnoredAny>,
}

/// Decodes JSON that may carry an id table, expanding ids in the records `container` names.
fn decode_with_ids<T: for<'de> Deserialize<'de>>(text: &str, has_ids: bool, container: Container) -> Result<T, serde_json::Error> {
    if !has_ids {
        return serde_json::from_str(text);
    }
    let mut value: serde_json::Value = serde_json::from_str(text)?;
    if let serde_json::Value::Object(map) = &mut value {
        expand_record_ids(map, |_| container);
    }
    // Union decoders re-read raw JSON text, so decode from text rather than from the tree.
    serde_json::from_str(&serde_json::to_string(&value)?)
}

fn restore(wire: SnapshotWire) -> ThreadSnapshot {
    let mut projection = wire.projection;
    bounded_snapshot_projection(&mut projection, wire.turn_items_omit_local_visible.is_some());
    let progressive = wire.history_cursor.is_some() || wire.has_more_history.is_some() || wire.latest_local_turn_ordinal.is_some();
    ThreadSnapshot {
        snapshot_sequence: wire.snapshot_sequence,
        projection,
        history: progressive.then(|| HistoryMeta {
            history_cursor: wire.history_cursor,
            has_more_history: wire.has_more_history.unwrap_or(false),
            latest_local_turn_ordinal: wire.latest_local_turn_ordinal,
            ..HistoryMeta::default()
        }),
    }
}

/// Decodes a bounded HTTP snapshot (`/threads/:id/bounded`).
pub fn decode_bounded_snapshot(text: &str) -> Result<ThreadSnapshot, serde_json::Error> {
    let head: Head<'_> = serde_json::from_str(text)?;
    let wire: SnapshotWire = decode_with_ids(text, head.ids.is_some(), Container::Key("projection"))?;
    let mut snapshot = restore(wire);
    snapshot.history.get_or_insert_with(HistoryMeta::default);
    Ok(snapshot)
}

/// One decoded item of `orchestration.subscribeThread`.
pub enum ThreadStreamItem {
    Snapshot(Box<ThreadSnapshot>),
    /// An event; `None` when this client cannot apply it, which still advances the cursor.
    Event { sequence: u64, event: Option<OrchestrationV2DomainEvent> },
    Synchronized,
    Ignored,
}

/// Decodes one stream value. A known event whose shape does not decode still advances the
/// cursor, like an unknown event: a strict decoder would stall the stream on the first new field.
pub fn decode_stream_item(raw: &RawValue) -> ThreadStreamItem {
    let text = raw.get();
    let Ok(head) = serde_json::from_str::<Head<'_>>(text) else { return ThreadStreamItem::Ignored };
    match head.kind.as_deref() {
        Some("synchronized") => ThreadStreamItem::Synchronized,
        Some("snapshot") => match decode_with_ids::<SnapshotWire>(text, head.ids.is_some(), Container::Key("projection")) {
            Ok(wire) => ThreadStreamItem::Snapshot(Box::new(restore(wire))),
            Err(error) => {
                tracing::warn!(%error, "a thread snapshot did not decode");
                ThreadStreamItem::Ignored
            }
        },
        Some("event") => {
            #[derive(Deserialize)]
            struct EventWire {
                sequence: u64,
                event: OrchestrationV2DomainEvent,
            }
            match serde_json::from_str::<EventWire>(text) {
                Ok(wire) => ThreadStreamItem::Event { sequence: wire.sequence, event: Some(wire.event) },
                Err(error) => {
                    tracing::debug!(%error, "skipping a thread event this client cannot decode");
                    head.sequence.map_or(ThreadStreamItem::Ignored, |sequence| ThreadStreamItem::Event { sequence, event: None })
                }
            }
        }
        _ => head.sequence.map_or(ThreadStreamItem::Ignored, |sequence| ThreadStreamItem::Event { sequence, event: None }),
    }
}

/// One page of older history (`OrchestrationV2ThreadHistoryPage`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub snapshot_sequence: u64,
    pub items: Vec<VisibleTurnItem>,
    pub next_cursor: Option<String>,
    pub has_more_history: bool,
    #[serde(default)]
    pub subagents: Option<Vec<Arc<OrchestrationV2Subagent>>>,
}

/// Decodes a history page, expanding its id table.
pub fn decode_history_page(text: &str) -> Result<HistoryPage, serde_json::Error> {
    let head: Head<'_> = serde_json::from_str(text)?;
    decode_with_ids(text, head.ids.is_some(), Container::TopLevel)
}
