//! What the viewer may do to an issue. Ports `issueWriteAccess` from
//! `issuePresentation.ts`; the event sentences live in the web crate.

use t3code_contracts::{IssueDetail, IssueState};

/// Why every write is blocked, when one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueWriteBlock {
    /// The client lacks the source-control write scope.
    NoWriteScope,
    /// The issue is outside every project's own scope.
    OutsideProjectScope,
}

impl IssueWriteBlock {
    /// The English message, also the translation source.
    pub fn message(self) -> &'static str {
        match self {
            Self::NoWriteScope => "Writing to this tracker needs the source-control write scope.",
            Self::OutsideProjectScope => "This issue is outside every project's own scope, so it is read-only here.",
        }
    }
}

/// The writes open to the viewer on one issue.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueWriteAccess {
    pub blocked: Option<IssueWriteBlock>,
    pub comment: bool,
    pub edit: bool,
    pub labels: bool,
    pub assignees: bool,
    pub react: bool,
    /// The states the issue can move to; empty when the viewer may not move it.
    pub states: Vec<IssueState>,
}

/// The client's write scope, the issue being inside a project's scope, what the tracker can do
/// at all, and what it grants this account.
pub fn issue_write_access(detail: &IssueDetail, can_write: bool) -> IssueWriteAccess {
    let blocked = if !can_write {
        Some(IssueWriteBlock::NoWriteScope)
    } else if !detail.writable {
        Some(IssueWriteBlock::OutsideProjectScope)
    } else {
        None
    };
    let may = |capability: bool, permission: bool| blocked.is_none() && capability && permission;
    let caps = &detail.capabilities;
    let perms = &detail.viewer_permissions;
    IssueWriteAccess {
        blocked,
        comment: may(caps.comment, perms.comment),
        edit: may(caps.edit, perms.edit),
        labels: may(caps.labels, perms.labels),
        assignees: may(caps.assignees, perms.assignees),
        react: blocked.is_none() && caps.reactions,
        states: if may(true, perms.set_state) {
            detail.states.iter().filter(|s| s.name != detail.state.name).cloned().collect()
        } else {
            Vec::new()
        },
    }
}
