//! Messages shown before the server echoes them (`optimisticUserMessages.ts`). A local message
//! leaves in the same update that brings the server's copy of its `messageId`.

use std::collections::HashSet;

use t3code_contracts::{ChatAttachment, MessageId, ThreadId};
use t3code_shared::thread_projection::ThreadProjection;

use super::models::EpochMs;

/// A message the composer sent and the server has not echoed yet.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimisticMessage {
    pub message_id: MessageId,
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
    pub created_at: EpochMs,
}

/// The local messages of the thread a view shows.
#[derive(Debug, Clone, Default)]
pub struct OptimisticMessages {
    thread_id: Option<ThreadId>,
    messages: Vec<OptimisticMessage>,
}

impl OptimisticMessages {
    /// Adds a sent message; ignored if the view moved to another thread.
    pub fn push(&mut self, thread_id: &ThreadId, message: OptimisticMessage) {
        if self.thread_id.as_ref() != Some(thread_id) {
            self.thread_id = Some(thread_id.clone());
            self.messages.clear();
        }
        self.messages.push(message);
    }

    /// Drops a message whose send failed.
    pub fn remove(&mut self, message_id: &MessageId) {
        self.messages.retain(|message| &message.message_id != message_id);
    }

    /// Drops messages the projection now holds, and everything when the view shows another
    /// thread. Returns whether anything changed.
    pub fn reconcile(&mut self, thread_id: &ThreadId, projection: Option<&ThreadProjection>) -> bool {
        if self.thread_id.as_ref() != Some(thread_id) {
            let changed = !self.messages.is_empty();
            self.thread_id = Some(thread_id.clone());
            self.messages.clear();
            return changed;
        }
        let Some(projection) = projection else { return false };
        let acknowledged: HashSet<&MessageId> = projection.messages.iter().map(|message| &message.id).collect();
        let before = self.messages.len();
        self.messages.retain(|message| !acknowledged.contains(&message.message_id));
        before != self.messages.len()
    }

    /// The messages still waiting for their echo.
    pub fn messages(&self) -> &[OptimisticMessage] {
        &self.messages
    }
}
