//! Pure sidebar rules. Ports `Sidebar.logic.ts`, the sorts it re-exports from
//! `client-runtime/state/threadSort.ts`, and the list partition in
//! `Sidebar.tsx:622-722`. Nothing here touches GPUI state.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::SharedString;

use super::sidebar::model::{
    ErrorClass, ProjectRef, ProjectSortOrder, RunStatus, RuntimeStatus, SettledOverride,
    SidebarSnapshot, SidebarThread, SubagentRequest, ThreadKey, TimestampMs,
};
use super::sidebar::sidebar_project_grouping::{ProjectGroup, build_project_groups};

/// `SidebarThreadStatus`, in precedence order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadStatus {
    Approval,
    Input,
    Request,
    Preparing,
    Working,
    Waiting,
    Failed,
    Limited,
    Ready,
}

/// `resolveSidebarThreadStatus` (`Sidebar.logic.ts:930-960`).
pub fn resolve_thread_status(thread: &SidebarThread) -> ThreadStatus {
    if thread.has_pending_approvals
        || thread.pending_subagent_request == Some(SubagentRequest::Approval)
    {
        return ThreadStatus::Approval;
    }
    if thread.has_pending_user_input
        || thread.pending_subagent_request == Some(SubagentRequest::UserInput)
    {
        return ThreadStatus::Input;
    }
    if thread.pending_guest_message_count > 0 {
        return ThreadStatus::Request;
    }
    let Some(runtime) = thread.runtime else {
        return ThreadStatus::Ready;
    };
    match runtime.status {
        RuntimeStatus::Run(RunStatus::Running) if thread.blocked_on_thread_wait => {
            ThreadStatus::Waiting
        }
        RuntimeStatus::Run(RunStatus::Preparing) => ThreadStatus::Preparing,
        RuntimeStatus::Run(
            RunStatus::Queued | RunStatus::Starting | RunStatus::Running | RunStatus::Waiting,
        ) => ThreadStatus::Working,
        RuntimeStatus::Idle => ThreadStatus::Waiting,
        RuntimeStatus::Run(RunStatus::Failed) => match runtime.last_error_class {
            Some(ErrorClass::UsageLimit) => ThreadStatus::Limited,
            _ => ThreadStatus::Failed,
        },
        RuntimeStatus::Run(_) => ThreadStatus::Ready,
    }
}

/// A completion the user has not seen (`hasUnseenCompletion`, fork: an unread
/// completion notification counts too).
pub fn has_unseen_completion(thread: &SidebarThread) -> bool {
    if thread.has_unread_completion_notification {
        return true;
    }
    let Some(run) = thread.latest_run else {
        return false;
    };
    let Some(completed_at) = run.completed_at else {
        return false;
    };
    if matches!(run.status, RunStatus::Interrupted | RunStatus::Cancelled) {
        return false;
    }
    // A thread never visited reads as seen.
    thread
        .last_visited_at
        .is_some_and(|visited| completed_at > visited)
}

/// A failed run newer than the last visit, or never visited
/// (`threadSettled.ts:469-481`).
pub fn has_unseen_run_failure(thread: &SidebarThread) -> bool {
    let Some(run) = thread.latest_run.filter(|run| run.status == RunStatus::Failed) else {
        return false;
    };
    match (thread.last_visited_at, run.completed_at.or(run.started_at)) {
        (None, _) => true,
        (Some(visited), Some(failed_at)) => failed_at > visited,
        (Some(_), None) => false,
    }
}

/// Row status: a failure the user already opened settles to ready.
pub fn resolve_row_status(thread: &SidebarThread) -> ThreadStatus {
    let status = resolve_thread_status(thread);
    let failed_run = thread
        .latest_run
        .is_some_and(|run| run.status == RunStatus::Failed);
    if status != ThreadStatus::Failed || !failed_run {
        return status;
    }
    if has_unseen_run_failure(thread) {
        status
    } else {
        ThreadStatus::Ready
    }
}

/// A snoozed thread that woke after the user last looked at it.
pub fn is_woke(thread: &SidebarThread) -> bool {
    let Some(woke_at) = thread.woke_at else {
        return false;
    };
    thread.settled_override != Some(SettledOverride::Settled)
        && thread.last_visited_at.is_none_or(|visited| visited < woke_at)
}

/// `shouldRecedeSidebarThread`: quiet rows dim their text.
pub fn should_recede(
    status: ThreadStatus,
    is_unread: bool,
    is_woke: bool,
    is_active: bool,
    is_selected: bool,
) -> bool {
    if is_active || is_selected || matches!(status, ThreadStatus::Input | ThreadStatus::Request) {
        return false;
    }
    match status {
        ThreadStatus::Preparing | ThreadStatus::Working | ThreadStatus::Waiting => true,
        ThreadStatus::Ready | ThreadStatus::Approval => !is_unread && !is_woke,
        _ => false,
    }
}

/// The status a row shows in its status slot: the nine sidebar indicators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopStatus {
    /// Working; `preparing` reads "Setting up", `goal` reads "Goal".
    Working { preparing: bool, goal: bool },
    Waiting,
    Approval,
    Input,
    Request,
    Limited,
    Failed,
    Woke,
    Done,
}

/// The status slot's content (`SidebarThreadRow.tsx:790-850`), if any.
pub fn resolve_top_status(
    thread: &SidebarThread,
    status: ThreadStatus,
    is_unread: bool,
    is_woke: bool,
) -> Option<TopStatus> {
    Some(match status {
        ThreadStatus::Preparing => TopStatus::Working {
            preparing: true,
            goal: false,
        },
        ThreadStatus::Working => TopStatus::Working {
            preparing: false,
            goal: thread.goal_active,
        },
        ThreadStatus::Waiting => TopStatus::Waiting,
        ThreadStatus::Approval => TopStatus::Approval,
        ThreadStatus::Input => TopStatus::Input,
        ThreadStatus::Request => TopStatus::Request,
        ThreadStatus::Limited => TopStatus::Limited,
        ThreadStatus::Failed => TopStatus::Failed,
        ThreadStatus::Ready if is_woke => TopStatus::Woke,
        ThreadStatus::Ready if is_unread => TopStatus::Done,
        ThreadStatus::Ready => return None,
    })
}

/// The dot on a closed folder: the most urgent hidden state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderDot {
    Attention,
    Failed,
    Unread,
    Working,
}

/// `resolveSidebarFolderDot`.
pub fn resolve_folder_dot<'a>(
    threads: impl IntoIterator<Item = &'a SidebarThread>,
) -> Option<FolderDot> {
    let mut best: Option<FolderDot> = None;
    let rank = |dot: FolderDot| match dot {
        FolderDot::Attention => 0,
        FolderDot::Failed => 1,
        FolderDot::Unread => 2,
        FolderDot::Working => 3,
    };
    for thread in threads {
        let status = resolve_row_status(thread);
        let dot = match status {
            ThreadStatus::Input
            | ThreadStatus::Approval
            | ThreadStatus::Request
            | ThreadStatus::Limited => Some(FolderDot::Attention),
            ThreadStatus::Failed => Some(FolderDot::Failed),
            ThreadStatus::Ready if has_unseen_completion(thread) => Some(FolderDot::Unread),
            ThreadStatus::Working | ThreadStatus::Preparing => Some(FolderDot::Working),
            _ => None,
        };
        if let Some(dot) = dot
            && best.is_none_or(|b| rank(dot) < rank(b))
        {
            best = Some(dot);
        }
    }
    best
}

/// `SidebarSection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SidebarSection {
    Pinned,
    Active,
    Working,
    Snoozed,
    Settled,
}

/// Snoozed until a future time and not already woken (`threadSettled.ts:158-167`).
pub fn effectively_snoozed(thread: &SidebarThread, now: TimestampMs) -> bool {
    thread.snoozed_until.is_some_and(|until| until > now) && thread.woke_at.is_none()
}

/// Working for the inbox shelf: running, with nothing waiting on the user
/// (`threadInbox.ts:17-31`).
pub fn is_inbox_working(thread: &SidebarThread) -> bool {
    matches!(
        resolve_thread_status(thread),
        ThreadStatus::Working | ThreadStatus::Preparing
    )
}

/// `resolveSidebarThreadSection` plus the working shelf.
pub fn resolve_section(
    thread: &SidebarThread,
    now: TimestampMs,
    working_shelf: bool,
) -> SidebarSection {
    if effectively_snoozed(thread, now) {
        SidebarSection::Snoozed
    } else if thread.settled_override == Some(SettledOverride::Settled) {
        SidebarSection::Settled
    } else if thread.pinned_at.is_some() {
        SidebarSection::Pinned
    } else if working_shelf && is_inbox_working(thread) {
        SidebarSection::Working
    } else {
        SidebarSection::Active
    }
}

/// Latest of the user message and run times, else `updatedAt`.
pub fn thread_activity_timestamp(thread: &SidebarThread) -> TimestampMs {
    let run = thread.latest_run;
    [
        thread.latest_user_message_at,
        run.and_then(|r| r.requested_at),
        run.and_then(|r| r.started_at),
        run.and_then(|r| r.completed_at),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(thread.updated_at)
}

/// `settledAt`, else the activity time.
pub fn settled_timestamp(thread: &SidebarThread) -> TimestampMs {
    thread
        .settled_at
        .unwrap_or_else(|| thread_activity_timestamp(thread))
}

fn identity_order(left: &SidebarThread, right: &SidebarThread) -> Ordering {
    left.key
        .thread_id
        .cmp(&right.key.thread_id)
        .then_with(|| left.key.environment_id.cmp(&right.key.environment_id))
}

/// Pinned: by `pinOrderKey`, keyless after them newest first.
pub fn sort_pinned(threads: &mut [Arc<SidebarThread>]) {
    threads.sort_by(|l, r| match (&l.pin_order_key, &r.pin_order_key) {
        (Some(a), Some(b)) => a.cmp(b).then_with(|| identity_order(l, r)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => r.created_at.cmp(&l.created_at).then_with(|| identity_order(l, r)),
    });
}

/// Active: keyless first by `max(createdAt, unsettledAt)` desc, then by `activeOrderKey`.
pub fn sort_active(threads: &mut [Arc<SidebarThread>]) {
    let anchor = |t: &SidebarThread| t.created_at.max(t.unsettled_at.unwrap_or_default());
    threads.sort_by(|l, r| {
        match (&l.active_order_key, &r.active_order_key) {
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
            (None, None) => anchor(r).cmp(&anchor(l)),
        }
        .then_with(|| identity_order(l, r))
    });
}

/// When the thread last came back to the user (`threadInbox.ts:57-74`).
pub fn return_timestamp(thread: &SidebarThread) -> TimestampMs {
    let run = thread.latest_run;
    [
        Some(thread.created_at),
        thread.unsettled_at,
        run.and_then(|r| r.requested_at),
        run.and_then(|r| r.completed_at),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or_default()
}

/// Working shelf and folder rows: by last send (`threadInbox.ts:81-96`).
pub fn send_timestamp(thread: &SidebarThread) -> TimestampMs {
    let sent = thread
        .latest_user_authored_message_at
        .or_else(|| thread.latest_run.and_then(|r| r.requested_at))
        .unwrap_or_default();
    thread.created_at.max(sent)
}

fn sort_desc_by(threads: &mut [Arc<SidebarThread>], key: impl Fn(&SidebarThread) -> TimestampMs) {
    threads.sort_by(|l, r| key(r).cmp(&key(l)).then_with(|| identity_order(l, r)));
}

/// Snoozed: soonest wake first.
pub fn sort_snoozed(threads: &mut [Arc<SidebarThread>]) {
    threads.sort_by(|l, r| {
        l.snoozed_until
            .cmp(&r.snoozed_until)
            .then_with(|| identity_order(l, r))
    });
}

/// Settled: newest settle first, id ascending on ties.
pub fn sort_settled(threads: &mut [Arc<SidebarThread>]) {
    threads.sort_by(|l, r| {
        settled_timestamp(r)
            .cmp(&settled_timestamp(l))
            .then_with(|| l.key.thread_id.cmp(&r.key.thread_id))
    });
}

/// Project groups ordered by their newest thread (`sortLogicalProjectsForSidebar`).
pub fn sort_project_groups(
    groups: &mut [ProjectGroup],
    threads: &[Arc<SidebarThread>],
    order: ProjectSortOrder,
) {
    if order == ProjectSortOrder::Manual {
        return;
    }
    let mut group_by_ref: HashMap<&ProjectRef, usize> = HashMap::new();
    for (ix, group) in groups.iter().enumerate() {
        for member in &group.member_refs {
            group_by_ref.insert(member, ix);
        }
    }
    let mut latest: Vec<Option<TimestampMs>> = vec![None; groups.len()];
    for thread in threads.iter().filter(|t| !t.archived) {
        let project_ref = thread.project_ref();
        let Some(&ix) = group_by_ref.get(&project_ref) else {
            continue;
        };
        let stamp = match order {
            ProjectSortOrder::CreatedAt => thread.created_at,
            _ => thread
                .latest_user_authored_message_at
                .or(thread.latest_user_message_at)
                .unwrap_or(thread.created_at),
        };
        latest[ix] = Some(latest[ix].map_or(stamp, |l| l.max(stamp)));
    }
    let mut keyed: Vec<(TimestampMs, ProjectGroup)> = groups
        .iter()
        .cloned()
        .zip(latest)
        .map(|(group, stamp)| {
            let fallback = match order {
                ProjectSortOrder::CreatedAt => group.created_at,
                _ => group.updated_at,
            };
            (stamp.unwrap_or(fallback), group)
        })
        .collect();
    keyed.sort_by(|(ls, l), (rs, r)| {
        rs.cmp(ls)
            .then_with(|| l.label.cmp(&r.label))
            .then_with(|| l.key.cmp(&r.key))
    });
    for (slot, (_, group)) in groups.iter_mut().zip(keyed) {
        *slot = group;
    }
}

/// `resolveAdjacentThreadId`.
pub fn adjacent_thread<T: PartialEq + Clone>(
    ids: &[T],
    current: Option<&T>,
    forward: bool,
) -> Option<T> {
    let Some(current) = current else {
        return if forward { ids.first() } else { ids.last() }.cloned();
    };
    let ix = ids.iter().position(|id| id == current)?;
    if forward {
        ids.get(ix + 1).cloned()
    } else {
        ix.checked_sub(1).and_then(|ix| ids.get(ix)).cloned()
    }
}

/// Folds a string for search: lowercase, whitespace collapsed.
fn fold(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sidebar search over title, branch and PR number. Every query token must
/// match; title-prefix hits rank first, then newest activity.
pub fn search_threads(threads: &[Arc<SidebarThread>], query: &str) -> Vec<Arc<SidebarThread>> {
    let query = fold(query);
    if query.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = query.split(' ').collect();
    let mut hits: Vec<(bool, TimestampMs, Arc<SidebarThread>)> = threads
        .iter()
        .filter(|t| !t.archived)
        .filter_map(|thread| {
            let title = fold(&thread.title);
            let mut haystack = title.clone();
            if let Some(branch) = &thread.branch {
                haystack.push(' ');
                haystack.push_str(&branch.to_lowercase());
            }
            if let Some(pr) = &thread.pull_request {
                haystack.push_str(&format!(" #{}", pr.number));
            }
            tokens.iter().all(|token| haystack.contains(token)).then(|| {
                (
                    title.starts_with(&query),
                    thread_activity_timestamp(thread),
                    thread.clone(),
                )
            })
        })
        .collect();
    hits.sort_by(|(lp, lt, l), (rp, rt, r)| {
        rp.cmp(lp)
            .then_with(|| rt.cmp(lt))
            .then_with(|| identity_order(l, r))
    });
    hits.into_iter().map(|(_, _, thread)| thread).collect()
}

/// `compactSidebarTimeLabel`: "now", "5m", "3h", "2d".
pub fn compact_relative_time(at: TimestampMs, now: TimestampMs) -> SharedString {
    let elapsed = (now.0 - at.0).max(0);
    if elapsed < TimestampMs::MINUTE {
        "now".into()
    } else if elapsed < TimestampMs::HOUR {
        format!("{}m", elapsed / TimestampMs::MINUTE).into()
    } else if elapsed < TimestampMs::DAY {
        format!("{}h", elapsed / TimestampMs::HOUR).into()
    } else {
        format!("{}d", elapsed / TimestampMs::DAY).into()
    }
}

/// `formatWorkingDurationLabel`: "42s", "7m", "1h 5m".
pub fn working_duration_label(elapsed_ms: i64) -> SharedString {
    let seconds = elapsed_ms.max(0) / 1000;
    if seconds < 60 {
        return format!("{seconds}s").into();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m").into();
    }
    format!("{}h {}m", minutes / 60, minutes % 60).into()
}

/// When the working duration started counting.
pub fn working_started_at(thread: &SidebarThread, status: ThreadStatus) -> Option<TimestampMs> {
    let run = thread.latest_run?;
    match status {
        ThreadStatus::Waiting => run.completed_at,
        _ => run.started_at.or(run.requested_at),
    }
}

/// `splitForMiddleTruncate`: the head truncates, the tail stays whole.
pub fn split_middle_truncate(value: &str) -> Option<(SharedString, SharedString)> {
    const DEFAULT_TAIL: usize = 10;
    const MAX_SEGMENT_TAIL: usize = 16;
    const MIN_WORD_TAIL: usize = 4;
    let is_break = |c: char| matches!(c, '-' | '_' | '.' | '/');
    let chars: Vec<char> = value.chars().collect();
    let mut keep = DEFAULT_TAIL;
    if let Some(slash) = chars.iter().rposition(|&c| c == '/')
        && slash > 0
        && slash < chars.len() - 1
    {
        let segment = chars.len() - slash - 1;
        keep = if segment <= MAX_SEGMENT_TAIL {
            segment
        } else {
            DEFAULT_TAIL
        };
    }
    if chars.len() <= keep + 4 {
        return None;
    }
    let mut cut = chars.len() - keep;
    if !is_break(chars[cut - 1]) {
        let starts_word = |ix: usize| is_break(chars[ix - 1]);
        let mut snapped = (cut + 1..=chars.len().saturating_sub(MIN_WORD_TAIL)).find(|&ix| starts_word(ix));
        if snapped.is_none() {
            let floor = chars.len().saturating_sub(MAX_SEGMENT_TAIL).max(5);
            snapped = (floor..cut).rev().filter(|&ix| starts_word(ix)).last();
        }
        if let Some(ix) = snapped {
            cut = ix;
        }
    }
    let head: String = chars[..cut].iter().collect();
    let tail: String = chars[cut..].iter().collect();
    Some((head.into(), tail.into()))
}

/// Which row layout a thread uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowVariant {
    /// 78px card (pinned, active, working, search results).
    Card,
    /// 36px row (snoozed, settled).
    Slim,
    /// 36px row inside a project folder.
    Folder,
}

/// A shelf at the bottom of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shelf {
    Working,
    Snoozed,
    Settled,
}

/// One item of the flattened, virtualized sidebar list.
#[derive(Clone, Debug)]
pub enum SidebarListEntry {
    Thread {
        thread: Arc<SidebarThread>,
        variant: RowVariant,
        section: SidebarSection,
    },
    ShelfHeader {
        shelf: Shelf,
        count: usize,
        expanded: bool,
    },
    FolderHeader {
        group: usize,
        collapsed: bool,
        hidden_count: usize,
        dot: Option<FolderDot>,
        first: bool,
    },
    ShowMore {
        count: usize,
    },
    /// `mt-auto`: pushes the shelves to the bottom when the list is short.
    ShelfSpacer,
    /// "No projects yet" and friends.
    Empty(EmptyState),
    /// No search results.
    SearchEmpty,
}

/// The centred empty-list messages (`Sidebar.tsx:3336-3373`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmptyState {
    NoProjects,
    NoThreads,
}

/// Sidebar UI state that changes the list: shelves, folders, search.
#[derive(Clone, Debug)]
pub struct SidebarListOptions {
    pub working_expanded: bool,
    pub snoozed_expanded: bool,
    pub settled_expanded: bool,
    pub settled_visible: usize,
    pub collapsed_folders: HashSet<SharedString>,
    pub search: SharedString,
}

impl Default for SidebarListOptions {
    fn default() -> Self {
        Self {
            working_expanded: true,
            snoozed_expanded: true,
            settled_expanded: false,
            settled_visible: 0,
            collapsed_folders: HashSet::new(),
            search: SharedString::default(),
        }
    }
}

/// The derived list plus what it was built from.
#[derive(Clone, Debug, Default)]
pub struct SidebarList {
    pub entries: Vec<SidebarListEntry>,
    pub groups: Vec<ProjectGroup>,
    /// Project display name by project ref, for card rows.
    pub project_labels: HashMap<ProjectRef, SharedString>,
    /// Project icon override and favicon by project ref, from its group.
    pub project_icons: HashMap<ProjectRef, crate::components::project_favicon::ProjectIconSource>,
    /// Thread keys in visual order, for keyboard and shortcut traversal.
    pub thread_order: Vec<ThreadKey>,
}

/// The card's meta line (branch, terminals, PR, issue, diff) has content.
pub fn card_has_meta(thread: &SidebarThread) -> bool {
    thread.branch.is_some()
        || thread.pull_request.is_some()
        || thread.issue_key.is_some()
        || thread.diff.is_some()
        || thread.running_terminal_count > 0
}

/// Fixed pixel height of an entry, without the 1px list gap.
pub fn entry_height(entry: &SidebarListEntry) -> f32 {
    use super::sidebar::style as s;
    match entry {
        SidebarListEntry::Thread {
            thread,
            variant: RowVariant::Card,
            ..
        } => {
            let inner = if card_has_meta(thread) {
                s::CARD_ROW_HEIGHT
            } else {
                s::CARD_ROW_HEIGHT_NO_META
            };
            f32::from(inner + s::CARD_ROW_PADDING_Y * 2.)
        }
        SidebarListEntry::Thread { .. } | SidebarListEntry::ShowMore { .. } => {
            f32::from(s::SLIM_ROW_HEIGHT)
        }
        SidebarListEntry::ShelfHeader { .. } => f32::from(s::SECTION_HEADER_HEIGHT),
        SidebarListEntry::FolderHeader { first, .. } => {
            let gap = if *first { 0. } else { f32::from(s::FOLDER_HEADER_GAP) };
            f32::from(s::SECTION_HEADER_HEIGHT) + gap
        }
        SidebarListEntry::ShelfSpacer => 0.,
        SidebarListEntry::Empty(EmptyState::NoProjects) => 98.,
        SidebarListEntry::Empty(EmptyState::NoThreads) => 64.,
        SidebarListEntry::SearchEmpty => 64.,
    }
}

/// Builds the flat list: pinned, active (or project folders), then the
/// Working, Snoozed and Settled shelves (`Sidebar.tsx:3534-3717`).
pub fn build_sidebar_list(snapshot: &SidebarSnapshot, options: &SidebarListOptions) -> SidebarList {
    let settings = snapshot.settings;
    let mut groups = build_project_groups(&snapshot.projects, settings.grouping_mode);
    sort_project_groups(&mut groups, &snapshot.threads, settings.project_sort_order);
    groups.retain(|g| !g.archived);

    let mut group_of: HashMap<ProjectRef, usize> = HashMap::new();
    let mut project_labels = HashMap::new();
    let mut project_icons = HashMap::new();
    for (ix, group) in groups.iter().enumerate() {
        for member in &group.member_refs {
            group_of.insert(member.clone(), ix);
            project_labels.insert(member.clone(), group.label.clone());
            if group.icon.is_some() || group.favicon.is_some() {
                let source = crate::components::project_favicon::ProjectIconSource { icon: group.icon.clone(), favicon: group.favicon.clone() };
                project_icons.insert(member.clone(), source);
            }
        }
    }

    let mut list = SidebarList {
        groups: Vec::new(),
        project_labels,
        project_icons,
        ..Default::default()
    };
    let push_thread = |list: &mut SidebarList, thread: &Arc<SidebarThread>, variant, section| {
        list.thread_order.push(thread.key.clone());
        list.entries.push(SidebarListEntry::Thread {
            thread: thread.clone(),
            variant,
            section,
        });
    };

    if !options.search.trim().is_empty() {
        let mut results = search_threads(&snapshot.threads, &options.search);
        // Then threads listed only for their messages, in the server's order (`searchSidebarThreads`).
        if let Some(matches) = snapshot.search_content_matches.as_ref().filter(|m| m.query.trim() == options.search.trim()) {
            for key in matches.threads.iter() {
                if results.iter().any(|t| &t.key == key) {
                    continue;
                }
                if let Some(thread) = snapshot.threads.iter().find(|t| &t.key == key && !t.archived) {
                    results.push(thread.clone());
                }
            }
        }
        if results.is_empty() {
            list.entries.push(SidebarListEntry::SearchEmpty);
        }
        for thread in &results {
            let section = resolve_section(thread, snapshot.now, settings.working_shelf);
            push_thread(&mut list, thread, RowVariant::Card, section);
        }
        list.groups = groups;
        return list;
    }

    let mut by_section: HashMap<SidebarSection, Vec<Arc<SidebarThread>>> = HashMap::new();
    for thread in snapshot.threads.iter().filter(|t| !t.archived) {
        let section = resolve_section(thread, snapshot.now, settings.working_shelf);
        by_section.entry(section).or_default().push(thread.clone());
    }
    let mut take = |section| by_section.remove(&section).unwrap_or_default();
    let mut pinned = take(SidebarSection::Pinned);
    let mut active = take(SidebarSection::Active);
    let mut working = take(SidebarSection::Working);
    let mut snoozed = take(SidebarSection::Snoozed);
    let mut settled = take(SidebarSection::Settled);
    sort_pinned(&mut pinned);
    sort_snoozed(&mut snoozed);
    sort_settled(&mut settled);
    // Older settled threads load in pages; the shelf lists them only as far as they are loaded.
    let settled_count = settled.len() + snapshot.older_settled.unloaded_count(None);
    settled.retain(|thread| snapshot.older_settled.is_listed(thread));
    sort_desc_by(&mut working, send_timestamp);

    for thread in &pinned {
        push_thread(&mut list, thread, RowVariant::Card, SidebarSection::Pinned);
    }

    if settings.project_folders {
        sort_desc_by(&mut active, send_timestamp);
        let mut folders: Vec<Vec<Arc<SidebarThread>>> = vec![Vec::new(); groups.len()];
        for thread in active {
            if let Some(&ix) = group_of.get(&thread.project_ref()) {
                folders[ix].push(thread);
            }
        }
        for (ix, threads) in folders.iter().enumerate() {
            let collapsed = options.collapsed_folders.contains(&groups[ix].key);
            list.entries.push(SidebarListEntry::FolderHeader {
                group: ix,
                collapsed,
                hidden_count: if collapsed { threads.len() } else { 0 },
                dot: collapsed
                    .then(|| resolve_folder_dot(threads.iter().map(|t| t.as_ref())))
                    .flatten(),
                first: ix == 0,
            });
            if !collapsed {
                for thread in threads {
                    push_thread(&mut list, thread, RowVariant::Folder, SidebarSection::Active);
                }
            }
        }
    } else {
        if settings.working_shelf {
            sort_desc_by(&mut active, return_timestamp);
        } else {
            sort_active(&mut active);
        }
        for thread in &active {
            push_thread(&mut list, thread, RowVariant::Card, SidebarSection::Active);
        }
    }

    let nothing_listed = list.entries.is_empty()
        && working.is_empty()
        && snoozed.is_empty()
        && settled_count == 0;
    if nothing_listed {
        list.entries.push(SidebarListEntry::Empty(if snapshot.projects.is_empty() {
            EmptyState::NoProjects
        } else {
            EmptyState::NoThreads
        }));
    }

    let mut spacer_placed = false;
    let mut shelf = |list: &mut SidebarList,
                     shelf: Shelf,
                     threads: &[Arc<SidebarThread>],
                     expanded: bool,
                     variant: RowVariant,
                     section: SidebarSection,
                     limit: usize| {
        // The Settled header stays, at 0, whenever the list has rows (`Sidebar.tsx:1984`).
        if threads.is_empty() && (shelf != Shelf::Settled || nothing_listed) {
            return;
        }
        if !spacer_placed {
            list.entries.push(SidebarListEntry::ShelfSpacer);
            spacer_placed = true;
        }
        list.entries.push(SidebarListEntry::ShelfHeader {
            shelf,
            count: threads.len(),
            expanded,
        });
        if expanded {
            for thread in threads.iter().take(limit) {
                push_thread(list, thread, variant, section);
            }
        }
    };
    shelf(
        &mut list,
        Shelf::Working,
        &working,
        options.working_expanded,
        RowVariant::Card,
        SidebarSection::Working,
        usize::MAX,
    );
    shelf(
        &mut list,
        Shelf::Snoozed,
        &snoozed,
        options.snoozed_expanded,
        RowVariant::Slim,
        SidebarSection::Snoozed,
        usize::MAX,
    );
    let settled_limit = settings.settled_page_size + options.settled_visible;
    shelf(
        &mut list,
        Shelf::Settled,
        &settled,
        options.settled_expanded,
        RowVariant::Slim,
        SidebarSection::Settled,
        settled_limit,
    );
    if let Some(SidebarListEntry::ShelfHeader { count, .. }) =
        list.entries.iter_mut().rev().find(|entry| matches!(entry, SidebarListEntry::ShelfHeader { shelf: Shelf::Settled, .. }))
    {
        *count = settled_count;
    }
    let hidden_settled = settled_count.saturating_sub(settled.len().min(settled_limit));
    if options.settled_expanded && hidden_settled > 0 {
        list.entries.push(SidebarListEntry::ShowMore {
            count: hidden_settled.min(settings.settled_page_size),
        });
    }
    list.groups = groups;
    list
}
