//! The pull requests linked to a thread, as the composer strip's badge and the details
//! card's row show them. Ports `resolveThreadCurrentPullRequestLink` and
//! `resolveThreadPullRequestBadge` (`source-control-core/shared/threadPullRequests.ts`)
//! and the labels of `prStatusIndicator` (`ThreadStatusIndicators.tsx`).

use gpui_kit::SharedString;
use t3code_contracts::{PullRequestMergeMethod, PullRequestState, ThreadPullRequestLink, ThreadPullRequestLinkSource};

use crate::components::branch_toolbar::StripPullRequestBadge;
use crate::components::chat::thread_details_pr_row::PullRequestRowData;
use crate::i18n::t;
use crate::source_control_presentation::{ChangeRequestTermKind, source_control_definition};

/// `visibleThreadPullRequests`: every link but a dismissed stack member.
fn visible(links: &[ThreadPullRequestLink]) -> impl Iterator<Item = &ThreadPullRequestLink> {
    links.iter().filter(|link| link.source != ThreadPullRequestLinkSource::StackDismissed)
}

/// A link without a snapshot counts as open, as in the fork.
fn state_of(link: &ThreadPullRequestLink) -> PullRequestState {
    link.snapshot.as_ref().map_or(PullRequestState::Open, |s| s.state)
}

/// `threadPullRequestUpdatedAtMs`: the snapshot's update time, else when it was linked.
/// ISO-8601 UTC strings order as their instants.
fn updated_at(link: &ThreadPullRequestLink) -> &str {
    link.snapshot.as_ref().and_then(|s| s.updated_at.as_deref()).unwrap_or(&link.linked_at)
}

/// The visible links all belong to one native stack.
fn one_stack<'a>(links: impl IntoIterator<Item = &'a ThreadPullRequestLink>) -> bool {
    let mut ids = links.into_iter().map(|link| link.stack.as_ref().map(|s| s.id.as_str()));
    let Some(Some(first)) = ids.next() else { return false };
    ids.all(|id| id == Some(first))
}

/// `resolveThreadCurrentPullRequestLink`: the one open link; of several open ones the
/// top of their stack, else the most recently linked; with none open, the latest updated.
pub fn current_link(links: &[ThreadPullRequestLink]) -> Option<&ThreadPullRequestLink> {
    let open: Vec<_> = visible(links).filter(|link| state_of(link) == PullRequestState::Open).collect();
    match open.as_slice() {
        [single] => Some(single),
        [] => visible(links).max_by(|a, b| updated_at(a).cmp(updated_at(b))),
        several if one_stack(several.iter().copied()) => {
            // The stack's layers run base first; the top is the last open member listed.
            let layers = &several[0].stack.as_ref()?.layers;
            layers.iter().rev().find_map(|layer| several.iter().copied().find(|link| link.number == layer.number))
        }
        several => several.iter().copied().max_by(|a, b| a.linked_at.cmp(&b.linked_at)),
    }
}

/// The host's change request noun from a link's URL (`findByChangeRequestUrl`).
fn term_for_url(url: &str) -> ChangeRequestTermKind {
    let kind = if url.contains("/-/merge_requests/") {
        Some("gitlab")
    } else if url.contains("/pull-requests/") {
        Some("bitbucket")
    } else if url.contains("/pullrequest/") {
        Some("azure-devops")
    } else {
        None
    };
    source_control_definition(kind).terminology.term_kind()
}

/// `prStatusLabels(...).tooltipLead`, e.g. "PR #418 - Open".
fn status_lead(state: PullRequestState, is_draft: bool, term: ChangeRequestTermKind, number: u64) -> SharedString {
    let term = term.as_str();
    match (state, is_draft) {
        (PullRequestState::Open, true) => t!("{term, select, mr {MR} cr {change request} other {PR}} #{number} - Draft", term = term, number = number),
        (PullRequestState::Closed, _) => t!("{term, select, mr {MR} cr {change request} other {PR}} #{number} - Closed", term = term, number = number),
        (PullRequestState::Merged, _) => t!("{term, select, mr {MR} cr {change request} other {PR}} #{number} - Merged", term = term, number = number),
        _ => t!("{term, select, mr {MR} cr {change request} other {PR}} #{number} - Open", term = term, number = number),
    }
}

/// The composer strip's badge (`ThreadPullRequestBadgeControl`): the current link, and how
/// many others are linked (a stack or several links open the Pull requests tab instead).
pub fn strip_badge(links: &[ThreadPullRequestLink]) -> Option<StripPullRequestBadge> {
    let link = current_link(links)?;
    let count = visible(links).count() as u64;
    let snapshot = link.snapshot.as_ref();
    let (state, is_draft) = (state_of(link), snapshot.is_some_and(|s| s.is_draft));
    let title: SharedString = snapshot.map(|s| s.title.clone()).unwrap_or_default().into();
    let lead = status_lead(state, is_draft, term_for_url(&link.url), link.number);
    Some(StripPullRequestBadge {
        number: link.number,
        url: link.url.clone().into(),
        label: t!("{tooltipLead}: {title}", tooltipLead = lead, title = title.clone()),
        title,
        state,
        is_draft,
        others: count.saturating_sub(1),
    })
}

/// The details card's row for the current link; `detail` arrives from `pullRequests.detail`.
pub fn row_data(
    links: &[ThreadPullRequestLink],
    project_merge_method: Option<PullRequestMergeMethod>,
    last_merge_method: PullRequestMergeMethod,
) -> Option<PullRequestRowData> {
    let link = current_link(links)?;
    let snapshot = link.snapshot.as_ref();
    Some(PullRequestRowData {
        number: link.number,
        title: snapshot.map(|s| s.title.clone()).unwrap_or_default().into(),
        url: link.url.clone().into(),
        state: state_of(link),
        is_draft: snapshot.is_some_and(|s| s.is_draft),
        detail: None,
        watching: link.watch.is_some(),
        project_merge_method,
        last_merge_method,
    })
}
