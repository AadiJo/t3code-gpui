//! Common fields of every turn item variant, read without matching on the type at each call site.
//! Items of a type this client does not know (`Unknown`) answer from their raw JSON.

use t3code_contracts::{NodeId, OrchestrationV2TurnItem, OrchestrationV2TurnItemStatus, RunId, ThreadId, TurnItemId};

/// The `deferredWork` marker of a collapsed work group: what to page for its hidden rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredWork<'a> {
    pub cursor: &'a str,
    pub item_count: u64,
    pub last_ordinal: u64,
}

macro_rules! for_each_variant {
    ($item:expr, $bound:ident => $body:expr, $unknown:ident => $fallback:expr) => {
        match $item {
            OrchestrationV2TurnItem::Notification($bound) => $body,
            OrchestrationV2TurnItem::UserMessage($bound) => $body,
            OrchestrationV2TurnItem::AssistantMessage($bound) => $body,
            OrchestrationV2TurnItem::Reasoning($bound) => $body,
            OrchestrationV2TurnItem::ProposedPlan($bound) => $body,
            OrchestrationV2TurnItem::TodoList($bound) => $body,
            OrchestrationV2TurnItem::UserInputRequest($bound) => $body,
            OrchestrationV2TurnItem::FileChange($bound) => $body,
            OrchestrationV2TurnItem::CommandExecution($bound) => $body,
            OrchestrationV2TurnItem::FileSearch($bound) => $body,
            OrchestrationV2TurnItem::WebSearch($bound) => $body,
            OrchestrationV2TurnItem::ApprovalRequest($bound) => $body,
            OrchestrationV2TurnItem::Checkpoint($bound) => $body,
            OrchestrationV2TurnItem::RunInterruptRequest($bound) => $body,
            OrchestrationV2TurnItem::RunInterruptResult($bound) => $body,
            OrchestrationV2TurnItem::SystemNotice($bound) => $body,
            OrchestrationV2TurnItem::Error($bound) => $body,
            OrchestrationV2TurnItem::Compaction($bound) => $body,
            OrchestrationV2TurnItem::Handoff($bound) => $body,
            OrchestrationV2TurnItem::Fork($bound) => $body,
            OrchestrationV2TurnItem::ThreadCreated($bound) => $body,
            OrchestrationV2TurnItem::SecretRequest($bound) => $body,
            OrchestrationV2TurnItem::LinkRequest($bound) => $body,
            OrchestrationV2TurnItem::Subagent($bound) => $body,
            OrchestrationV2TurnItem::DynamicTool($bound) => $body,
            OrchestrationV2TurnItem::Unknown($unknown) => $fallback,
        }
    };
}

/// Read access to the base fields every turn item carries (`OrchestrationV2TurnItemBase`).
pub trait TurnItemExt {
    fn item_id(&self) -> TurnItemId;
    fn item_thread_id(&self) -> Option<ThreadId>;
    fn item_run_id(&self) -> Option<&RunId>;
    fn item_node_id(&self) -> Option<&NodeId>;
    fn item_ordinal(&self) -> u64;
    /// `None` for an item type this client does not know.
    fn item_status(&self) -> Option<OrchestrationV2TurnItemStatus>;
    /// The wire `type` tag.
    fn type_name(&self) -> &str;
    fn deferred_work(&self) -> Option<DeferredWork<'_>>;
    /// Sets the deferred work cursor, or drops the marker when `cursor` is `None`.
    fn set_deferred_work_cursor(&mut self, cursor: Option<String>);
    /// Copies `other`'s deferred work marker onto this item when both are the same type.
    fn keep_deferred_work_from(&mut self, other: &Self);
    /// Whether this item's work is still going (`isOrchestrationV2WorkActive`).
    fn is_work_active(&self) -> bool {
        self.item_status().is_some_and(|status| {
            matches!(
                status,
                OrchestrationV2TurnItemStatus::Pending
                    | OrchestrationV2TurnItemStatus::Running
                    | OrchestrationV2TurnItemStatus::Waiting
            )
        })
    }
}

fn unknown_str<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(serde_json::Value::as_str)
}

impl TurnItemExt for OrchestrationV2TurnItem {
    fn item_id(&self) -> TurnItemId {
        for_each_variant!(self, item => item.id.clone(), value => TurnItemId::new(unknown_str(value, "id").unwrap_or_default()))
    }

    fn item_thread_id(&self) -> Option<ThreadId> {
        for_each_variant!(self, item => Some(item.thread_id.clone()), value => unknown_str(value, "threadId").map(ThreadId::from))
    }

    fn item_run_id(&self) -> Option<&RunId> {
        for_each_variant!(self, item => item.run_id.as_ref(), _value => None)
    }

    fn item_node_id(&self) -> Option<&NodeId> {
        for_each_variant!(self, item => item.node_id.as_ref(), _value => None)
    }

    fn item_ordinal(&self) -> u64 {
        for_each_variant!(self, item => item.ordinal, value => value.get("ordinal").and_then(serde_json::Value::as_u64).unwrap_or(0))
    }

    fn item_status(&self) -> Option<OrchestrationV2TurnItemStatus> {
        for_each_variant!(self, item => Some(item.status), _value => None)
    }

    fn type_name(&self) -> &str {
        match self {
            Self::Notification(_) => "notification",
            Self::UserMessage(_) => "user_message",
            Self::AssistantMessage(_) => "assistant_message",
            Self::Reasoning(_) => "reasoning",
            Self::ProposedPlan(_) => "proposed_plan",
            Self::TodoList(_) => "todo_list",
            Self::UserInputRequest(_) => "user_input_request",
            Self::FileChange(_) => "file_change",
            Self::CommandExecution(_) => "command_execution",
            Self::FileSearch(_) => "file_search",
            Self::WebSearch(_) => "web_search",
            Self::ApprovalRequest(_) => "approval_request",
            Self::Checkpoint(_) => "checkpoint",
            Self::RunInterruptRequest(_) => "run_interrupt_request",
            Self::RunInterruptResult(_) => "run_interrupt_result",
            Self::SystemNotice(_) => "system_notice",
            Self::Error(_) => "error",
            Self::Compaction(_) => "compaction",
            Self::Handoff(_) => "handoff",
            Self::Fork(_) => "fork",
            Self::ThreadCreated(_) => "thread_created",
            Self::SecretRequest(_) => "secret_request",
            Self::LinkRequest(_) => "link_request",
            Self::Subagent(_) => "subagent",
            Self::DynamicTool(_) => "dynamic_tool",
            Self::Unknown(value) => unknown_str(value, "type").unwrap_or("unknown"),
        }
    }

    fn deferred_work(&self) -> Option<DeferredWork<'_>> {
        for_each_variant!(
            self,
            item => item.deferred_work.as_ref().map(|work| DeferredWork {
                cursor: &work.cursor,
                item_count: work.item_count,
                last_ordinal: work.last_ordinal,
            }),
            _value => None
        )
    }

    fn set_deferred_work_cursor(&mut self, cursor: Option<String>) {
        for_each_variant!(
            self,
            item => match cursor {
                None => item.deferred_work = None,
                Some(cursor) => if let Some(work) = item.deferred_work.as_mut() {
                    work.cursor = cursor;
                },
            },
            _value => {}
        )
    }

    fn keep_deferred_work_from(&mut self, other: &Self) {
        macro_rules! copy {
            ($($variant:ident),*) => {
                match (self, other) {
                    $((Self::$variant(item), Self::$variant(previous)) => {
                        if let Some(work) = &previous.deferred_work {
                            item.deferred_work = Some(work.clone());
                        }
                    })*
                    _ => {}
                }
            };
        }
        copy!(
            Notification, UserMessage, AssistantMessage, Reasoning, ProposedPlan, TodoList, UserInputRequest,
            FileChange, CommandExecution, FileSearch, WebSearch, ApprovalRequest, Checkpoint, RunInterruptRequest,
            RunInterruptResult, SystemNotice, Error, Compaction, Handoff, Fork, ThreadCreated, SecretRequest,
            LinkRequest, Subagent, DynamicTool
        );
    }
}

/// Steers join a running turn; every other user message, and every notification, opens one
/// (`startsTurn` in `threads.ts`).
pub fn starts_turn(item: &OrchestrationV2TurnItem) -> bool {
    use t3code_contracts::OrchestrationV2UserMessageInputIntent as Intent;
    match item {
        OrchestrationV2TurnItem::Notification(_) => true,
        OrchestrationV2TurnItem::UserMessage(message) => {
            !matches!(message.input_intent, Intent::Steer | Intent::PromotedQueuedToSteer)
        }
        _ => false,
    }
}
