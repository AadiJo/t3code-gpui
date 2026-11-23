//! Helpers over orchestration types (`orchestrationV2.ts`).

use crate::{
    OrchestrationV2Actor, OrchestrationV2AppThread, OrchestrationV2AppThreadLineage,
    OrchestrationV2AppThreadLineageRelationshipToParent as Relationship, OrchestrationV2CreationSource,
    OrchestrationV2ExecutionNodeStatus, OrchestrationV2IdleCompaction,
};

/// The prompt that only asks the provider to compact.
pub const IDLE_COMPACTION_TEXT: &str = "/compact";

impl OrchestrationV2ExecutionNodeStatus {
    /// Whether work in this state is still going (`isOrchestrationV2WorkActive`).
    pub const fn is_work_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::Waiting)
    }
}

impl OrchestrationV2AppThreadLineage {
    /// Whether a thread with this lineage appears in thread lists (`isListedThreadLineage`):
    /// subagent threads and side chats do not.
    pub fn is_listed(&self) -> bool {
        self.relationship_to_parent != Some(Relationship::Subagent) && self.side_chat != Some(true)
    }
}

impl OrchestrationV2AppThread {
    /// Whether a provider spawned this thread as its own subagent (`isProviderNativeSubagentThread`).
    pub fn is_provider_native_subagent(&self) -> bool {
        self.lineage.relationship_to_parent == Some(Relationship::Subagent)
            && self.creation_source == OrchestrationV2CreationSource::Provider
    }
}

/// The parts of a message that decide whether it is a compaction command.
#[derive(Debug, Clone, Copy)]
pub struct CompactionMessage<'a> {
    pub role: &'a str,
    pub text: &'a str,
    pub attachment_count: usize,
    pub created_by: Option<OrchestrationV2Actor>,
    pub creation_source: Option<OrchestrationV2CreationSource>,
    pub idle_compaction: Option<OrchestrationV2IdleCompaction>,
}

impl CompactionMessage<'_> {
    /// A user prompt that only asks the provider to compact (`isCompactionCommandMessage`).
    pub fn is_compaction_command(&self) -> bool {
        self.role == "user" && self.text.trim().eq_ignore_ascii_case(IDLE_COMPACTION_TEXT) && self.attachment_count == 0
    }

    /// Why an idle compaction ran, if this message is one (`idleCompactionOf`). Servers before
    /// `idleCompaction` marked their own as agent-created.
    pub fn idle_compaction(&self) -> Option<OrchestrationV2IdleCompaction> {
        if !self.is_compaction_command() {
            return None;
        }
        self.idle_compaction.or_else(|| {
            (self.created_by == Some(OrchestrationV2Actor::Agent)
                && self.creation_source == Some(OrchestrationV2CreationSource::Server))
            .then_some(OrchestrationV2IdleCompaction::BeforeCacheExpires)
        })
    }
}
