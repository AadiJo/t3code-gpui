//! Thread timelines for the fixture store. Every id carries the thread id as
//! a prefix, so two threads never share a message id (the timeline caches
//! parsed markdown by message id).

use std::sync::Arc;

use gpui_kit::SharedString;

use crate::components::chat::timeline_entries::*;

/// The timeline clock: the sidebar fixtures' "now" (2026-11-23 15:00 UTC).
pub const NOW_MS: i64 = crate::components::sidebar::fixtures::FIXTURE_NOW.0;

/// Builds one thread's entries with unique, prefixed ids.
pub struct TimelineBuilder {
    prefix: SharedString,
    pub entries: Vec<TimelineEntry>,
    next: usize,
}

impl TimelineBuilder {
    pub fn new(prefix: impl Into<SharedString>) -> Self {
        Self { prefix: prefix.into(), entries: Vec::new(), next: 0 }
    }

    /// Continues numbering after `entries`.
    pub fn resume(prefix: impl Into<SharedString>, entries: Vec<TimelineEntry>, next: usize) -> Self {
        Self { prefix: prefix.into(), entries, next }
    }

    pub fn next_index(&self) -> usize {
        self.next
    }

    fn id(&mut self, kind: &str) -> SharedString {
        self.next += 1;
        format!("{}:{kind}-{}", self.prefix, self.next).into()
    }

    pub fn user(&mut self, at: Timestamp, text: impl Into<SharedString>, intent: InputIntent) -> MessageId {
        let id = self.id("user");
        self.entries.push(TimelineEntry {
            id: EntryId(id.clone()),
            created_at: at,
            kind: EntryKind::Message(ChatMessage {
                id: MessageId(id.clone()),
                role: MessageRole::User,
                text: text.into(),
                created_at: at,
                updated_at: at,
                streaming: false,
                run_id: None,
                attachments: Arc::from([]),
                input_intent: intent,
                sent_by_app: false,
                guest: None,
            }),
        });
        MessageId(id)
    }

    pub fn assistant(&mut self, at: Timestamp, run: &RunId, text: impl Into<SharedString>, streaming: bool) -> MessageId {
        let id = self.id("assistant");
        self.entries.push(TimelineEntry {
            id: EntryId(id.clone()),
            created_at: at,
            kind: EntryKind::Message(ChatMessage {
                id: MessageId(id.clone()),
                role: MessageRole::Assistant,
                text: text.into(),
                created_at: at,
                updated_at: at,
                streaming,
                run_id: Some(run.clone()),
                attachments: Arc::from([]),
                input_intent: InputIntent::Prompt,
                sent_by_app: false,
                guest: None,
            }),
        });
        MessageId(id)
    }

    pub fn work(&mut self, at: Timestamp, run: &RunId, status: ToolStatus, label: &str, item: WorkItem) -> EntryId {
        let id = EntryId(self.id("work"));
        self.entries.push(TimelineEntry {
            id: id.clone(),
            created_at: at,
            kind: EntryKind::Work(WorkLogEntry {
                id: id.clone(),
                run_id: Some(run.clone()),
                created_at: at,
                status,
                label: label.to_string().into(),
                item,
                superseded_attempt: None,
            }),
        });
        id
    }

    pub fn run_id(&mut self) -> RunId {
        RunId(self.id("run"))
    }

    pub fn request_id(&mut self) -> RequestId {
        RequestId(self.id("request"))
    }
}

fn ts(ms_ago: i64) -> Timestamp {
    Timestamp(NOW_MS - ms_ago)
}

/// Rust source of `lines` lines, for code blocks in replies.
pub fn rust_code(lines: usize, seed: usize) -> String {
    let mut code = String::from("```rust\n");
    for n in 0..lines {
        let line = match n % 6 {
            0 => format!("/// Applies batch {seed}.{n} of the replay window.\n"),
            1 => format!("pub fn apply_{seed}_{n}(store: &mut Store, batch: &Batch) -> Result<usize> {{\n"),
            2 => format!("    let cursor = store.cursor(\"env-{n}\")?.unwrap_or_default();\n"),
            3 => "    let fresh = batch.items.iter().filter(|item| item.seq > cursor.seq);\n".to_string(),
            4 => format!("    store.persist(fresh.count() + {n})\n"),
            _ => "}\n".to_string(),
        };
        code.push_str(&line);
    }
    code.push_str("```\n");
    code
}

/// A finished thread of `turns` turns. Each turn is a prompt, a folded work
/// group of two tool calls and a reply, every fourth reply with a long code
/// block: three timeline rows per turn.
pub fn finished_thread(prefix: &str, title: &str, turns: usize) -> TimelineInput {
    let mut b = TimelineBuilder::new(SharedString::from(prefix.to_string()));
    for turn in 0..turns {
        let ago = ((turns - turn) as i64) * 90_000;
        let run = b.run_id();
        let prompt = if turn == 0 {
            format!("{title}. Start by reading the code that owns this.")
        } else {
            format!("Turn {turn}: keep going and show me what changed.")
        };
        b.user(ts(ago), prompt, InputIntent::Prompt);
        b.work(
            ts(ago - 5_000),
            &run,
            ToolStatus::Completed,
            "Read",
            WorkItem::FileRead { paths: Arc::from([SharedString::from("src/replay.rs")]), output: Some("fn replay() {}".into()) },
        );
        b.work(
            ts(ago - 9_000),
            &run,
            ToolStatus::Completed,
            "Command",
            WorkItem::Command { command: "cargo test replay".into(), output: Some("test result: ok".into()), exit_code: Some(0) },
        );
        let reply = if turn % 4 == 0 {
            format!(
                "Here is the handler after turn {turn}.\n\n{}\nIt skips items at or below the cursor, then **persists** the new position.",
                rust_code(24 + (turn % 3) * 24, turn)
            )
        } else {
            format!(
                "Turn {turn} is done. The loop now batches writes:\n\n- fewer fsyncs\n- one transaction per batch\n\nSee `src/replay.rs:{}` for the change.",
                turn + 10
            )
        };
        b.assistant(ts(ago - 30_000), &run, reply, false);
    }
    TimelineInput { entries: Arc::new(b.entries), ..Default::default() }
}

/// Adds a pending approval to the end of `input` (an inline approval row).
pub fn with_pending_approval(input: TimelineInput, prefix: &str) -> TimelineInput {
    let mut b = TimelineBuilder::resume(SharedString::from(prefix.to_string()), (*input.entries).clone(), input.entries.len() + 1);
    let run = b.run_id();
    let request = b.request_id();
    b.work(
        ts(20_000),
        &run,
        ToolStatus::InProgress,
        "Run command",
        WorkItem::Approval {
            request_id: request.clone(),
            kind: ApprovalKind::Command,
            detail: "pnpm vitest run apps/web/src/router.test.ts".into(),
            reason: Some("The agent wants to run the router tests.".into()),
        },
    );
    TimelineInput {
        entries: Arc::new(b.entries),
        pending_approval_ids: vec![request],
        latest_run: Some(LatestRun { run_id: run, status: RunStatus::Waiting, started_at: Some(ts(60_000)), completed_at: None, turn_ended: false }),
        ..input
    }
}

/// The questions a fixture thread waits on: two, so the drawer pages between them.
pub fn pending_questions(prefix: &str, first_index: usize) -> Vec<PendingQuestion> {
    let mut b = TimelineBuilder::resume(SharedString::from(prefix.to_string()), Vec::new(), first_index);
    let request_id = b.request_id();
    let question = |index: usize, header: &str, question: &str, options: [(&str, &str); 3]| PendingQuestion {
        request_id: request_id.clone(),
        header: header.to_string().into(),
        question: question.to_string().into(),
        options: options.iter().map(|(label, description)| (SharedString::from(*label), SharedString::from(*description))).collect(),
        multi_select: false,
        index,
        count: 2,
    };
    vec![
        question(0, "Rollout", "Should the new replay loop ship behind a flag?", [
            ("Behind a flag", "Off by default; turn on per environment."),
            ("Ship it", "On for everyone in the next release."),
            ("Hold", "Wait for another round of review."),
        ]),
        question(1, "Tests", "Which tests should run before it merges?", [
            ("Unit only", "The replay loop's own tests."),
            ("Unit and end to end", "Also the crash-between-writes scenario."),
            ("Everything", "The whole suite, including the slow ones."),
        ]),
    ]
}

/// Adds a pending question (the composer's top drawer) to `input`.
pub fn with_pending_question(input: TimelineInput, questions: &[PendingQuestion], prefix: &str) -> TimelineInput {
    let mut b = TimelineBuilder::resume(SharedString::from(prefix.to_string()), Vec::new(), input.entries.len() + 1);
    let run = b.run_id();
    TimelineInput {
        pending_question: questions.first().cloned(),
        latest_run: Some(LatestRun { run_id: run, status: RunStatus::Waiting, started_at: Some(ts(60_000)), completed_at: None, turn_ended: false }),
        ..input
    }
}

/// Gives every other reply of a short finished thread a changed-files card,
/// so its turns have diffs, and allows "Edit from here".
pub fn with_diff_summaries(input: TimelineInput) -> TimelineInput {
    let replies = input.entries.iter().filter_map(|entry| match &entry.kind {
        EntryKind::Message(m) if m.role == MessageRole::Assistant => Some(m.id.clone()),
        _ => None,
    });
    let summaries: Vec<TurnDiffSummary> = replies
        .enumerate()
        .filter(|(index, _)| index % 2 == 0)
        .map(|(index, id)| {
            let path = |n: usize| SharedString::from(super::workspace::CHANGED_PATHS[n % super::workspace::CHANGED_PATHS.len()]);
            let file = |n: usize, additions, deletions| FileChange { path: path(n), additions, deletions, operation: "update".into() };
            TurnDiffSummary {
                assistant_message_id: id,
                files: Arc::from([file(index, 12 + index as u32, 3), file(index + 1, 4, 1)]),
            }
        })
        .collect();
    TimelineInput { turn_diff_summaries: summaries.into(), supports_rollback: true, ..input }
}

/// The reply a fixture turn streams back, with prose, a list and a code block.
pub fn canned_reply(prompt: &str) -> String {
    let topic: String = prompt.chars().take(60).collect();
    format!(
        "On it. You asked: *{topic}*\n\nI read the replay loop first. The cursor is persisted after each batch, so a crash mid-batch replays at most one batch.\n\n- the loop skips items at or below the cursor\n- writes go out in one transaction per batch\n- the cursor moves only after the commit\n\n{}\nThat keeps duplicate deliveries at zero. Next I would add a test that kills the process between the write and the cursor update.",
        rust_code(12, prompt.len())
    )
}
