//! T3 Code's own MCP tools as the timeline presents them (`packages/shared/src/t3McpToolPresentation.ts`):
//! which tool a provider's tool name refers to, its labels and icon, and the action its
//! calls summarize as. The table is the TypeScript's, row for row.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// The icon a T3 tool row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T3ToolIcon {
    T3Code,
    Browser,
    Device,
    PullRequest,
}

/// How a group of calls to one tool summarizes (`T3McpToolSummaryAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum T3SummaryAction {
    Capabilities,
    Delegate,
    TaskStatus,
    TaskCancel,
    ScheduleRun,
    ScheduleCreate,
    ScheduleList,
    ScheduleUpdate,
    ScheduleDelete,
    ThreadCreate,
    ThreadList,
    ThreadRead,
    ThreadSend,
    ThreadWait,
    ThreadInterrupt,
    WorkflowStop,
    BackgroundWait,
    ThreadConfiguration,
    ThreadConfigure,
    ThreadFork,
    ThreadMerge,
    ThreadSearch,
    ThreadTransfers,
    ThreadOrganize,
    ThreadUpdate,
    QueueList,
    QueueRead,
    QueueEdit,
    QueueCancel,
    QueueReorder,
    QueueSteer,
    QuestionList,
    QuestionRead,
    QuestionRespond,
    SecretRequest,
    WorktreeHandoff,
    WorktreeList,
    WorktreeStatus,
    ProjectList,
    ProjectRead,
    ProjectCreate,
    ProjectUpdate,
    ProjectDelete,
    ProjectClone,
    EnvironmentRead,
    EnvironmentLinks,
    EnvironmentLink,
    EnvironmentUnlink,
    EnvironmentUpdate,
    ThemeRead,
    ThemeSet,
    AttachmentPrepare,
    AttachmentDiscard,
    AttachmentSend,
    LinkPr,
    UnlinkPr,
    ListPrs,
    WatchPr,
    UnwatchPr,
    WritePrGuides,
    ReadPrGuide,
    ListIssues,
    ReadIssue,
    CreateIssue,
    CommentIssue,
    SetIssueState,
    LinkIssue,
    UnlinkIssue,
    ListThreadIssues,
    ReadPr,
    ListRepoPrs,
    CreatePr,
    UpdatePr,
    RespondPr,
    MergePr,
    ReviewPr,
    CiLogs,
    CiRerun,
    Browser,
    Device,
    HtmlPreview,
    HtmlRender,
    SendToUser,

}

impl T3SummaryAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Delegate => "delegate",
            Self::TaskStatus => "task-status",
            Self::TaskCancel => "task-cancel",
            Self::ScheduleRun => "schedule-run",
            Self::ScheduleCreate => "schedule-create",
            Self::ScheduleList => "schedule-list",
            Self::ScheduleUpdate => "schedule-update",
            Self::ScheduleDelete => "schedule-delete",
            Self::ThreadCreate => "thread-create",
            Self::ThreadList => "thread-list",
            Self::ThreadRead => "thread-read",
            Self::ThreadSend => "thread-send",
            Self::ThreadWait => "thread-wait",
            Self::ThreadInterrupt => "thread-interrupt",
            Self::WorkflowStop => "workflow-stop",
            Self::BackgroundWait => "background-wait",
            Self::ThreadConfiguration => "thread-configuration",
            Self::ThreadConfigure => "thread-configure",
            Self::ThreadFork => "thread-fork",
            Self::ThreadMerge => "thread-merge",
            Self::ThreadSearch => "thread-search",
            Self::ThreadTransfers => "thread-transfers",
            Self::ThreadOrganize => "thread-organize",
            Self::ThreadUpdate => "thread-update",
            Self::QueueList => "queue-list",
            Self::QueueRead => "queue-read",
            Self::QueueEdit => "queue-edit",
            Self::QueueCancel => "queue-cancel",
            Self::QueueReorder => "queue-reorder",
            Self::QueueSteer => "queue-steer",
            Self::QuestionList => "question-list",
            Self::QuestionRead => "question-read",
            Self::QuestionRespond => "question-respond",
            Self::SecretRequest => "secret-request",
            Self::WorktreeHandoff => "worktree-handoff",
            Self::WorktreeList => "worktree-list",
            Self::WorktreeStatus => "worktree-status",
            Self::ProjectList => "project-list",
            Self::ProjectRead => "project-read",
            Self::ProjectCreate => "project-create",
            Self::ProjectUpdate => "project-update",
            Self::ProjectDelete => "project-delete",
            Self::ProjectClone => "project-clone",
            Self::EnvironmentRead => "environment-read",
            Self::EnvironmentLinks => "environment-links",
            Self::EnvironmentLink => "environment-link",
            Self::EnvironmentUnlink => "environment-unlink",
            Self::EnvironmentUpdate => "environment-update",
            Self::ThemeRead => "theme-read",
            Self::ThemeSet => "theme-set",
            Self::AttachmentPrepare => "attachment-prepare",
            Self::AttachmentDiscard => "attachment-discard",
            Self::AttachmentSend => "attachment-send",
            Self::LinkPr => "link-pr",
            Self::UnlinkPr => "unlink-pr",
            Self::ListPrs => "list-prs",
            Self::WatchPr => "watch-pr",
            Self::UnwatchPr => "unwatch-pr",
            Self::WritePrGuides => "write-pr-guides",
            Self::ReadPrGuide => "read-pr-guide",
            Self::ListIssues => "list-issues",
            Self::ReadIssue => "read-issue",
            Self::CreateIssue => "create-issue",
            Self::CommentIssue => "comment-issue",
            Self::SetIssueState => "set-issue-state",
            Self::LinkIssue => "link-issue",
            Self::UnlinkIssue => "unlink-issue",
            Self::ListThreadIssues => "list-thread-issues",
            Self::ReadPr => "read-pr",
            Self::ListRepoPrs => "list-repo-prs",
            Self::CreatePr => "create-pr",
            Self::UpdatePr => "update-pr",
            Self::RespondPr => "respond-pr",
            Self::MergePr => "merge-pr",
            Self::ReviewPr => "review-pr",
            Self::CiLogs => "ci-logs",
            Self::CiRerun => "ci-rerun",
            Self::Browser => "browser",
            Self::Device => "device",
            Self::HtmlPreview => "html-preview",
            Self::HtmlRender => "html-render",
            Self::SendToUser => "send-to-user",

        }
    }
}

/// One tool: `labels` are action, running, completed and detail (`["Read", "Reading", "Read", "a thread"]`).
#[derive(Debug, PartialEq, Eq)]
pub struct T3ToolDefinition {
    pub name: &'static str,
    pub display_name: &'static str,
    pub labels: [&'static str; 4],
    pub icon: T3ToolIcon,
    pub summary_action: T3SummaryAction,
}

static T3_TOOLS: &[T3ToolDefinition] = &[
    T3ToolDefinition { name: "ci_logs", display_name: "Read CI logs", labels: ["Read", "Reading", "Read", "CI logs"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::CiLogs },
    T3ToolDefinition { name: "ci_rerun", display_name: "Re-run CI", labels: ["Re-run", "Re-running", "Re-ran", "CI"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::CiRerun },
    T3ToolDefinition { name: "comment_on_issue", display_name: "Comment on an issue", labels: ["Comment on", "Commenting on", "Commented on", "an issue"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::CommentIssue },
    T3ToolDefinition { name: "create_issue", display_name: "File an issue", labels: ["File", "Filing", "Filed", "an issue"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::CreateIssue },
    T3ToolDefinition { name: "create_threads", display_name: "Create T3 threads", labels: ["Create", "Creating", "Created", "T3 threads"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadCreate },
    T3ToolDefinition { name: "delegate_task", display_name: "Delegate a child task", labels: ["Delegate", "Delegating", "Delegated", "a child task"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::Delegate },
    T3ToolDefinition { name: "delete_scheduled_task", display_name: "Delete a scheduled task", labels: ["Delete", "Deleting", "Requested deletion of", "a scheduled task"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ScheduleDelete },
    T3ToolDefinition { name: "device_close", display_name: "Close a device", labels: ["Close", "Closing", "Closed", "a device"], icon: T3ToolIcon::Device, summary_action: T3SummaryAction::Device },
    T3ToolDefinition { name: "device_list", display_name: "List simulators and emulators", labels: ["List", "Listing", "Listed", "simulators and emulators"], icon: T3ToolIcon::Device, summary_action: T3SummaryAction::Device },
    T3ToolDefinition { name: "device_open", display_name: "Open a device in the Device panel", labels: ["Open", "Opening", "Opened", "a device in the Device panel"], icon: T3ToolIcon::Device, summary_action: T3SummaryAction::Device },
    T3ToolDefinition { name: "device_screenshot", display_name: "Take a screenshot of the device", labels: ["Take a screenshot of", "Taking a screenshot of", "Took a screenshot of", "the device"], icon: T3ToolIcon::Device, summary_action: T3SummaryAction::Device },
    T3ToolDefinition { name: "html_preview", display_name: "Preview an HTML page", labels: ["Preview", "Previewing", "Previewed", "an HTML page"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::HtmlPreview },
    T3ToolDefinition { name: "html_render", display_name: "Render an HTML page", labels: ["Render", "Rendering", "Rendered", "an HTML page"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::HtmlRender },
    T3ToolDefinition { name: "link_issue", display_name: "Link an issue", labels: ["Link", "Linking", "Linked", "an issue"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::LinkIssue },
    T3ToolDefinition { name: "link_pull_request", display_name: "Link a pull request", labels: ["Link", "Linking", "Linked", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::LinkPr },
    T3ToolDefinition { name: "link_pull_requests", display_name: "Link pull requests", labels: ["Link", "Linking", "Linked", "pull requests"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::LinkPr },
    T3ToolDefinition { name: "list_issues", display_name: "List issues", labels: ["List", "Listing", "Listed", "issues"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ListIssues },
    T3ToolDefinition { name: "list_scheduled_tasks", display_name: "List scheduled tasks", labels: ["List", "Listing", "Listed", "scheduled tasks"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ScheduleList },
    T3ToolDefinition { name: "list_thread_issues", display_name: "Check linked issues", labels: ["Check", "Checking", "Checked", "linked issues"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ListThreadIssues },
    T3ToolDefinition { name: "list_thread_pull_requests", display_name: "Check linked pull requests", labels: ["Check", "Checking", "Checked", "linked pull requests"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::ListPrs },
    T3ToolDefinition { name: "orchestrator_capabilities", display_name: "Get orchestration capabilities", labels: ["Get", "Getting", "Got", "orchestration capabilities"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::Capabilities },
    T3ToolDefinition { name: "preview_act", display_name: "Use the preview browser", labels: ["Use", "Using", "Used", "the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_click", display_name: "Click in the preview browser", labels: ["Click", "Clicking", "Clicked", "in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_dialog", display_name: "Respond to a preview browser dialog", labels: ["Respond", "Responding", "Responded", "to a preview browser dialog"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_drag", display_name: "Drag in the preview browser", labels: ["Drag", "Dragging", "Dragged", "in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_evaluate", display_name: "Evaluate script in the preview browser", labels: ["Evaluate", "Evaluating", "Evaluated", "script in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_hover", display_name: "Hover in the preview browser", labels: ["Hover", "Hovering", "Hovered", "in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_navigate", display_name: "Navigate the preview browser", labels: ["Navigate", "Navigating", "Navigated", "the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_open", display_name: "Open a page in the preview browser", labels: ["Open", "Opening", "Opened", "a page in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_press", display_name: "Press a key in the preview browser", labels: ["Press", "Pressing", "Pressed", "a key in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_recording_start", display_name: "Start recording the preview browser", labels: ["Start", "Starting", "Started", "recording the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_recording_stop", display_name: "Stop recording the preview browser", labels: ["Stop", "Stopping", "Stopped", "recording the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_resize", display_name: "Resize the preview browser", labels: ["Resize", "Resizing", "Resized", "the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_scroll", display_name: "Scroll the preview browser", labels: ["Scroll", "Scrolling", "Scrolled", "the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_select", display_name: "Choose an option in the preview browser", labels: ["Choose", "Choosing", "Chose", "an option in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_set_appearance", display_name: "Set preview browser appearance", labels: ["Set", "Setting", "Set", "preview browser appearance"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_snapshot", display_name: "Snapshot the preview page", labels: ["Take a snapshot of", "Taking a snapshot of", "Took a snapshot of", "the preview page"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_status", display_name: "Get preview browser status", labels: ["Get", "Getting", "Got", "preview browser status"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_type", display_name: "Type in the preview browser", labels: ["Type", "Typing", "Typed", "in the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_upload", display_name: "Upload files to the preview browser", labels: ["Upload", "Uploading", "Uploaded", "files to the preview browser"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "preview_wait_for", display_name: "Wait for the preview page", labels: ["Wait", "Waiting", "Waited", "for the preview page"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "pull_request_create", display_name: "Open a pull request", labels: ["Open", "Opening", "Opened", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::CreatePr },
    T3ToolDefinition { name: "pull_request_list", display_name: "List pull requests", labels: ["List", "Listing", "Listed", "pull requests"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::ListRepoPrs },
    T3ToolDefinition { name: "pull_request_merge", display_name: "Merge a pull request", labels: ["Merge", "Merging", "Merged", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::MergePr },
    T3ToolDefinition { name: "pull_request_read", display_name: "Read a pull request", labels: ["Read", "Reading", "Read", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::ReadPr },
    T3ToolDefinition { name: "pull_request_respond", display_name: "Respond on a pull request", labels: ["Respond", "Responding", "Responded", "on a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::RespondPr },
    T3ToolDefinition { name: "pull_request_review", display_name: "Review a pull request", labels: ["Review", "Reviewing", "Reviewed", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::ReviewPr },
    T3ToolDefinition { name: "pull_request_update", display_name: "Update a pull request", labels: ["Update", "Updating", "Updated", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::UpdatePr },
    T3ToolDefinition { name: "read_issue", display_name: "Read an issue", labels: ["Read", "Reading", "Read", "an issue"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ReadIssue },
    T3ToolDefinition { name: "read_pull_request_guide", display_name: "Read a review guide", labels: ["Read", "Reading", "Read", "a review guide"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::ReadPrGuide },
    T3ToolDefinition { name: "request_secret", display_name: "Ask for a secret", labels: ["Ask for", "Asking for", "Asked for", "a secret"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::SecretRequest },
    T3ToolDefinition { name: "run_scheduled_task_now", display_name: "Run a scheduled task", labels: ["Run", "Running", "Requested a run of", "a scheduled task"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ScheduleRun },
    T3ToolDefinition { name: "schedule_task", display_name: "Schedule a task", labels: ["Schedule", "Scheduling", "Scheduled", "a task"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ScheduleCreate },
    T3ToolDefinition { name: "send_to_user", display_name: "Send a message", labels: ["Send", "Sending", "Sent", "a message"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::SendToUser },
    T3ToolDefinition { name: "set_issue_state", display_name: "Change an issue's state", labels: ["Change", "Changing", "Changed", "an issue's state"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::SetIssueState },
    T3ToolDefinition { name: "t3_attachment_discard", display_name: "Discard a pending attachment", labels: ["Discard", "Discarding", "Discarded", "a pending attachment"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::AttachmentDiscard },
    T3ToolDefinition { name: "t3_attachment_prepare_upload", display_name: "Prepare an attachment upload", labels: ["Prepare", "Preparing", "Prepared", "an attachment upload"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::AttachmentPrepare },
    T3ToolDefinition { name: "t3_background_wait", display_name: "Update background waits", labels: ["Update", "Updating", "Updated", "background waits"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::BackgroundWait },
    T3ToolDefinition { name: "t3_environment_link", display_name: "Ask to link an environment", labels: ["Ask to link", "Asking to link", "Asked to link", "an environment"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::EnvironmentLink },
    T3ToolDefinition { name: "t3_environment_links", display_name: "List linked environments", labels: ["List", "Listing", "Listed", "linked environments"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::EnvironmentLinks },
    T3ToolDefinition { name: "t3_environment_preferences_update", display_name: "Update environment preferences", labels: ["Update", "Updating", "Updated", "environment preferences"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::EnvironmentUpdate },
    T3ToolDefinition { name: "t3_environment_read", display_name: "Read environment preferences", labels: ["Read", "Reading", "Read", "environment preferences"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::EnvironmentRead },
    T3ToolDefinition { name: "t3_environment_unlink", display_name: "Forget a linked environment", labels: ["Forget", "Forgetting", "Forgot", "a linked environment"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::EnvironmentUnlink },
    T3ToolDefinition { name: "t3_pending_request", display_name: "Handle pending questions", labels: ["Handle", "Handling", "Handled", "pending questions"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QuestionRespond },
    T3ToolDefinition { name: "t3_pending_request_list", display_name: "List pending questions", labels: ["List", "Listing", "Listed", "pending questions"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QuestionList },
    T3ToolDefinition { name: "t3_pending_request_read", display_name: "Read pending questions", labels: ["Read", "Reading", "Read", "pending questions"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QuestionRead },
    T3ToolDefinition { name: "t3_pending_request_respond", display_name: "Answer pending questions", labels: ["Answer", "Answering", "Answered", "pending questions"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QuestionRespond },
    T3ToolDefinition { name: "t3_preview_close", display_name: "Close a preview tab", labels: ["Close", "Closing", "Closed", "a preview tab"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "t3_preview_list", display_name: "List preview tabs", labels: ["List", "Listing", "Listed", "preview tabs"], icon: T3ToolIcon::Browser, summary_action: T3SummaryAction::Browser },
    T3ToolDefinition { name: "t3_project_clone", display_name: "Clone a repository", labels: ["Clone", "Cloning", "Cloned", "a repository"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ProjectClone },
    T3ToolDefinition { name: "t3_project_create", display_name: "Register a project", labels: ["Register", "Registering", "Registered", "a project"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ProjectCreate },
    T3ToolDefinition { name: "t3_project_delete", display_name: "Delete a project", labels: ["Delete", "Deleting", "Deleted", "a project"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ProjectDelete },
    T3ToolDefinition { name: "t3_project_list", display_name: "List projects", labels: ["List", "Listing", "Listed", "projects"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ProjectList },
    T3ToolDefinition { name: "t3_project_read", display_name: "Read a project", labels: ["Read", "Reading", "Read", "a project"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ProjectRead },
    T3ToolDefinition { name: "t3_project_update", display_name: "Update a project", labels: ["Update", "Updating", "Updated", "a project"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ProjectUpdate },
    T3ToolDefinition { name: "t3_queue", display_name: "Manage queued messages", labels: ["Manage", "Managing", "Managed", "queued messages"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueEdit },
    T3ToolDefinition { name: "t3_queue_cancel", display_name: "Cancel a queued run", labels: ["Cancel", "Canceling", "Requested cancellation of", "a queued run"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueCancel },
    T3ToolDefinition { name: "t3_queue_edit", display_name: "Edit a queued message", labels: ["Edit", "Editing", "Edited", "a queued message"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueEdit },
    T3ToolDefinition { name: "t3_queue_list", display_name: "List queued messages", labels: ["List", "Listing", "Listed", "queued messages"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueList },
    T3ToolDefinition { name: "t3_queue_promote_to_steer", display_name: "Steer with a queued message", labels: ["Steer with", "Steering with", "Requested steering with", "a queued message"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueSteer },
    T3ToolDefinition { name: "t3_queue_read", display_name: "Read a queued message", labels: ["Read", "Reading", "Read", "a queued message"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueRead },
    T3ToolDefinition { name: "t3_queue_reorder", display_name: "Reorder a queued run", labels: ["Reorder", "Reordering", "Reordered", "a queued run"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::QueueReorder },
    T3ToolDefinition { name: "t3_theme_read", display_name: "Read themes", labels: ["Read", "Reading", "Read", "themes"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThemeRead },
    T3ToolDefinition { name: "t3_theme_set", display_name: "Set the theme", labels: ["Set", "Setting", "Set", "the theme"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThemeSet },
    T3ToolDefinition { name: "t3_thread_configuration", display_name: "Read thread configuration", labels: ["Read", "Reading", "Read", "thread configuration"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadConfiguration },
    T3ToolDefinition { name: "t3_thread_configure", display_name: "Use thread model settings", labels: ["Use", "Using", "Used", "thread model settings"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadConfigure },
    T3ToolDefinition { name: "t3_thread_fork", display_name: "Fork this thread", labels: ["Fork", "Forking", "Requested a fork of", "this thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadFork },
    T3ToolDefinition { name: "t3_thread_interrupt", display_name: "Interrupt a T3 thread", labels: ["Interrupt", "Interrupting", "Requested an interrupt of", "a T3 thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadInterrupt },
    T3ToolDefinition { name: "t3_thread_launch", display_name: "Launch a project thread", labels: ["Launch", "Launching", "Launched", "a project thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadCreate },
    T3ToolDefinition { name: "t3_thread_list", display_name: "List T3 threads", labels: ["List", "Listing", "Listed", "T3 threads"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadList },
    T3ToolDefinition { name: "t3_thread_merge_back", display_name: "Merge thread context", labels: ["Merge", "Merging", "Requested a merge of", "thread context"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadMerge },
    T3ToolDefinition { name: "t3_thread_organize", display_name: "Organize a thread", labels: ["Organize", "Organizing", "Organized", "a thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadOrganize },
    T3ToolDefinition { name: "t3_thread_read", display_name: "Read a T3 thread", labels: ["Read", "Reading", "Read", "a T3 thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadRead },
    T3ToolDefinition { name: "t3_thread_search", display_name: "Search thread content", labels: ["Search", "Searching", "Searched", "thread content"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadSearch },
    T3ToolDefinition { name: "t3_thread_send", display_name: "Send to a T3 thread", labels: ["Send", "Sending", "Sent", "to a T3 thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadSend },
    T3ToolDefinition { name: "t3_thread_send_attachments", display_name: "Send attachments", labels: ["Send", "Sending", "Sent", "attachments"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::AttachmentSend },
    T3ToolDefinition { name: "t3_thread_start", display_name: "Start a T3 thread", labels: ["Start", "Starting", "Started", "a T3 thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadCreate },
    T3ToolDefinition { name: "t3_thread_transfer", display_name: "Use thread transfers", labels: ["Use", "Using", "Used", "thread transfers"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadFork },
    T3ToolDefinition { name: "t3_thread_transfers", display_name: "Read thread transfers", labels: ["Read", "Reading", "Read", "thread transfers"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadTransfers },
    T3ToolDefinition { name: "t3_thread_update", display_name: "Update T3 thread metadata", labels: ["Update", "Updating", "Updated", "T3 thread metadata"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadUpdate },
    T3ToolDefinition { name: "t3_thread_wait", display_name: "Wait for a T3 thread", labels: ["Wait", "Waiting", "Waited", "for a T3 thread"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ThreadWait },
    T3ToolDefinition { name: "t3_workflow_stop", display_name: "Stop a workflow", labels: ["Stop", "Stopping", "Requested a stop for", "a workflow"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::WorkflowStop },
    T3ToolDefinition { name: "t3_worktree_handoff", display_name: "Hand off thread to a git worktree", labels: ["Hand off", "Handing off", "Handed off", "thread to a git worktree"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::WorktreeHandoff },
    T3ToolDefinition { name: "t3_worktree_list", display_name: "List workspace branches", labels: ["List", "Listing", "Listed", "workspace branches"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::WorktreeList },
    T3ToolDefinition { name: "t3_worktree_status", display_name: "Get thread worktree status", labels: ["Get", "Getting", "Got", "thread worktree status"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::WorktreeStatus },
    T3ToolDefinition { name: "task_cancel", display_name: "Cancel a delegated task", labels: ["Cancel", "Canceling", "Requested cancellation of", "a delegated task"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::TaskCancel },
    T3ToolDefinition { name: "task_status", display_name: "Get delegated task status", labels: ["Get", "Getting", "Got", "delegated task status"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::TaskStatus },
    T3ToolDefinition { name: "unlink_issue", display_name: "Unlink an issue", labels: ["Unlink", "Unlinking", "Unlinked", "an issue"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::UnlinkIssue },
    T3ToolDefinition { name: "unlink_pull_request", display_name: "Unlink a pull request", labels: ["Unlink", "Unlinking", "Unlinked", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::UnlinkPr },
    T3ToolDefinition { name: "unlink_pull_requests", display_name: "Unlink pull requests by state", labels: ["Unlink", "Unlinking", "Unlinked", "pull requests by state"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::UnlinkPr },
    T3ToolDefinition { name: "unwatch_pull_request", display_name: "Stop watching a pull request", labels: ["Stop watching", "Unwatching", "Stopped watching", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::UnwatchPr },
    T3ToolDefinition { name: "update_scheduled_task", display_name: "Update a scheduled task", labels: ["Update", "Updating", "Updated", "a scheduled task"], icon: T3ToolIcon::T3Code, summary_action: T3SummaryAction::ScheduleUpdate },
    T3ToolDefinition { name: "watch_pull_request", display_name: "Watch a pull request", labels: ["Watch", "Watching", "Watching", "a pull request"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::WatchPr },
    T3ToolDefinition { name: "write_pull_request_guides", display_name: "Write review guides", labels: ["Write", "Writing", "Started writing", "review guides"], icon: T3ToolIcon::PullRequest, summary_action: T3SummaryAction::WritePrGuides },
];

fn definition(name: &str) -> Option<&'static T3ToolDefinition> {
    T3_TOOLS.binary_search_by(|tool| tool.name.cmp(name)).ok().map(|index| &T3_TOOLS[index])
}

const SERVER_ALIASES: &[&str] = &["t3-code", "t3_code", "t3code", "t3-code-direct", "t3_code_direct"];

macro_rules! regex {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).expect("valid regex"));
    };
}

regex!(COMPLETE_SUFFIX, r"(?i)\s+(?:complete|completed)\s*$");
regex!(MCP_NAME, r"(?i)^mcp__(.+?)__(.+)$");
regex!(NAMESPACED, r"(?i)^(?:t3[-_]code[-_]direct|t3-code|t3_code|t3code)(?:[.:/]|\s*·\s*)(.+)$");
regex!(PREFIXED, r"(?i)^(?:mcp[-_]{1,2})?t3[-_ ]?code(?:__|[-_.:/ ])(.+)$");
regex!(OPENCODE_THREAD_PREFIX, r"(?i)^t3-code-");

/// The bare tool name a provider's label refers to (`parseT3McpToolName`). ACP prefixes vary,
/// so it matches loosely but requires a known tool.
fn parse_tool_name(value: &str) -> Option<String> {
    let label = COMPLETE_SUFFIX.replace(value, "");
    let label = label.trim();
    if let Some(captures) = MCP_NAME.captures(label) {
        return SERVER_ALIASES.contains(&captures[1].to_lowercase().as_str()).then(|| captures[2].to_owned());
    }
    if let Some(captures) = NAMESPACED.captures(label) {
        return Some(captures[1].to_owned());
    }
    let candidate = PREFIXED.captures(label).map_or(label, |c| c.get(1).map_or(label, |m| m.as_str()));
    if definition(candidate).is_some() {
        return Some(candidate.to_owned());
    }
    // OpenCode 2 thread prefixes can contain underscores; take the longest known suffix.
    if !OPENCODE_THREAD_PREFIX.is_match(label) {
        return None;
    }
    T3_TOOLS
        .iter()
        .filter(|tool| label.ends_with(&format!("_{}", tool.name)))
        .max_by_key(|tool| tool.name.len())
        .map(|tool| tool.name.to_owned())
}

/// The id of the T3 tool a name refers to (`resolveT3McpToolId`).
pub fn t3_tool_id(tool_name: Option<&str>) -> Option<&'static str> {
    parse_tool_name(tool_name?).and_then(|name| definition(&name)).map(|tool| tool.name)
}

/// An action tool presents each call as the single-purpose tool its action replaced.
fn action_tool(name: &str, action: &str) -> Option<String> {
    match name {
        "preview_act" => Some(format!("preview_{action}")),
        "t3_queue" => Some(format!("t3_queue_{action}")),
        "t3_pending_request" => Some(format!("t3_pending_request_{action}")),
        "t3_thread_transfer" => match action {
            "fork" => Some("t3_thread_fork".to_owned()),
            "merge_back" => Some("t3_thread_merge_back".to_owned()),
            "status" => Some("t3_thread_transfers".to_owned()),
            _ => None,
        },
        _ => None,
    }
}

/// The definition a call resolves to, following action tools (`resolveT3McpToolDefinition`).
pub fn t3_tool_definition(tool_name: Option<&str>, input: Option<&Value>) -> Option<&'static T3ToolDefinition> {
    let name = parse_tool_name(tool_name?)?;
    let base = definition(&name)?;
    let action = input.and_then(|input| input.get("action")).and_then(Value::as_str);
    Some(action.and_then(|action| action_tool(&name, action)).and_then(|tool| definition(&tool)).unwrap_or(base))
}

/// The summary action of a call; some tools summarize by which fields the call sets
/// (`resolveT3McpSummaryAction`).
pub fn t3_summary_action(tool_name: Option<&str>, input: Option<&Value>) -> Option<T3SummaryAction> {
    let definition = t3_tool_definition(tool_name, input)?;
    let fields = input.and_then(Value::as_object);
    let by_fields = fields.and_then(|fields| match parse_tool_name(tool_name?)?.as_str() {
        "t3_thread_configure" => {
            Some(if fields.contains_key("modelSelection") { T3SummaryAction::ThreadConfigure } else { T3SummaryAction::ThreadConfiguration })
        }
        "schedule_task" => {
            Some(if fields.contains_key("scheduledTaskId") { T3SummaryAction::ScheduleUpdate } else { T3SummaryAction::ScheduleCreate })
        }
        _ => None,
    });
    Some(by_fields.unwrap_or(definition.summary_action))
}

/// What a T3 tool call returned (`readResult` in `t3ToolSummary.ts`): the first JSON object
/// found through text blocks and `structuredContent`, and whether any layer reported an error.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct T3ToolResult {
    pub data: Option<serde_json::Map<String, Value>>,
    pub failed: bool,
}

/// Reads a call's output or input the way `readResult` does: JSON strings are parsed, text
/// blocks are searched, `structuredContent`/`content` are followed four levels deep.
pub fn read_t3_tool_result(value: &Value) -> T3ToolResult {
    read_result(value, 0)
}

fn read_result(value: &Value, depth: usize) -> T3ToolResult {
    if depth > 4 {
        return T3ToolResult::default();
    }
    match value {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed) => read_result(&parsed, depth + 1),
            Err(_) => T3ToolResult::default(),
        },
        Value::Array(blocks) => {
            let mut result = T3ToolResult::default();
            for block in blocks {
                let record = block.as_object();
                let text = record.and_then(|r| r.get("text")).or_else(|| record.and_then(|r| r.get("content")).and_then(|c| c.get("text")));
                let inner = text.and_then(|t| t.as_object().and_then(|o| o.get("text"))).or(text);
                let block_result = inner.map_or_else(T3ToolResult::default, |inner| read_result(inner, depth + 1));
                if result.data.is_none() {
                    result.data = block_result.data;
                }
                result.failed |= block_result.failed;
            }
            result
        }
        Value::Object(record) => {
            let tag_failed = record.get("_tag").and_then(Value::as_str).is_some_and(|tag| tag.ends_with("Error") || tag.ends_with("Failure"));
            let failed = record.get("isError") == Some(&Value::Bool(true))
                || record.get("is_error") == Some(&Value::Bool(true))
                || tag_failed
                || record.get("error").is_some_and(|error| !error.is_null());
            match record.get("structuredContent").or_else(|| record.get("content")) {
                Some(content) => {
                    let inner = read_result(content, depth + 1);
                    T3ToolResult { data: inner.data, failed: failed || inner.failed }
                }
                None => T3ToolResult { data: Some(record.clone()), failed },
            }
        }
        _ => T3ToolResult::default(),
    }
}

/// A call's input as its summary reads it (`readInput`): historical Cursor items keep the
/// MCP args envelope, which is unwrapped.
pub fn read_t3_tool_input(value: &Value) -> Option<serde_json::Map<String, Value>> {
    let input = read_result(value, 0).data?;
    if input.get("toolName").is_some_and(Value::is_string) {
        return input.get("args").and_then(Value::as_object).cloned();
    }
    Some(input)
}

/// MCP errors can come back as data even when the provider completed the call
/// (`t3ToolResultIndicatesFailure`).
pub fn t3_tool_result_indicates_failure(output: &Value) -> bool {
    read_result(output, 0).failed
}
