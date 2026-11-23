//! Issue sentences and state tones. Ports `issueEventSentence` from
//! client-runtime `issuePresentation.ts` and `issuePresentation.tsx`.

use gpui_kit::{Hsla, SharedString};
use t3code_contracts::{IssueEvent, IssueEventKind, IssueState, IssueStateCategory};

use crate::AppTheme;
use crate::components::icons::IconName;
use crate::components::pull_request::pull_request_presentation::{merged_tone, open_tone};
use crate::i18n::t;

/// "completed" → "Completed"; a reason the tracker spells with a dash reads as words.
fn close_reason_label(reason: &str) -> String {
    let words = reason.replace(['-', '_'], " ");
    let words = words.trim();
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What an event says after its actor's name.
pub fn issue_event_sentence(event: &IssueEvent) -> SharedString {
    let detail = event.detail.as_deref().filter(|d| !d.is_empty());
    match (event.kind, detail) {
        (IssueEventKind::Closed, Some(d)) => t!("closed this as {reason}", reason = close_reason_label(d).to_lowercase().as_str()),
        (IssueEventKind::Closed, None) => t!("closed this"),
        (IssueEventKind::Reopened, _) => t!("reopened this"),
        (IssueEventKind::Labeled, Some(d)) => t!("added the {label} label", label = d),
        (IssueEventKind::Labeled, None) => t!("added a label"),
        (IssueEventKind::Unlabeled, Some(d)) => t!("removed the {label} label", label = d),
        (IssueEventKind::Unlabeled, None) => t!("removed a label"),
        (IssueEventKind::Assigned, Some(d)) => t!("assigned {person}", person = d),
        (IssueEventKind::Assigned, None) => t!("assigned someone"),
        (IssueEventKind::Unassigned, Some(d)) => t!("unassigned {person}", person = d),
        (IssueEventKind::Unassigned, None) => t!("unassigned someone"),
        (IssueEventKind::Renamed, Some(d)) => t!("renamed this to “{title}”", title = d),
        (IssueEventKind::Renamed, None) => t!("renamed this"),
        (IssueEventKind::Referenced, Some(d)) => t!("referenced this from {source}", source = d),
        (IssueEventKind::Referenced, None) => t!("referenced this"),
        (IssueEventKind::StateChanged, Some(d)) => t!("moved this to {state}", state = d),
        (IssueEventKind::StateChanged, None) => t!("changed the state"),
    }
}

/// An issue state's icon and tone: open green, closed violet like a merged pull request.
pub fn issue_state_presentation(theme: &AppTheme, state: &IssueState) -> (IconName, Hsla) {
    match state.category {
        IssueStateCategory::Open => (IconName::CircleDot, open_tone(theme)),
        IssueStateCategory::Closed => (IconName::CircleCheck, merged_tone(theme)),
    }
}
