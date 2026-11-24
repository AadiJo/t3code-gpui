//! Fixture threads for scenes and measurements: one thread that holds every
//! row kind and markdown element, a generated thread of thousands of rows
//! with long code blocks, and a long reply to stream.

use std::sync::Arc;

use gpui_kit::SharedString;

use super::timeline_entries::*;

/// "Now" for fixtures: 2026-11-23 15:41 UTC.
pub const FIXTURE_NOW: i64 = 1_795_448_460_000;

fn at(seconds_ago: i64) -> Timestamp {
    Timestamp(FIXTURE_NOW - seconds_ago * 1_000)
}

fn s(text: &str) -> SharedString {
    SharedString::from(text.to_owned())
}

struct Builder {
    entries: Vec<TimelineEntry>,
    next: usize,
}

impl Builder {
    fn new() -> Self {
        Self { entries: Vec::new(), next: 0 }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{}", self.next)
    }

    fn user(&mut self, ago: i64, text: &str, intent: InputIntent, attachments: Vec<Attachment>) -> &mut Self {
        let id = self.id("user");
        self.entries.push(TimelineEntry {
            id: EntryId::new(id.clone()),
            created_at: at(ago),
            kind: EntryKind::Message(ChatMessage {
                id: MessageId::new(id),
                role: MessageRole::User,
                text: s(text),
                created_at: at(ago),
                updated_at: at(ago),
                streaming: false,
                run_id: None,
                attachments: attachments.into(),
                input_intent: intent,
                sent_by_app: false,
                guest: None,
                from_agent: None,
            }),
        });
        self
    }

    fn assistant(&mut self, ago: i64, run: &str, text: &str, streaming: bool) -> MessageId {
        let id = self.id("assistant");
        self.entries.push(TimelineEntry {
            id: EntryId::new(id.clone()),
            created_at: at(ago + 4),
            kind: EntryKind::Message(ChatMessage {
                id: MessageId::new(id.clone()),
                role: MessageRole::Assistant,
                text: s(text),
                created_at: at(ago + 4),
                updated_at: at(ago),
                streaming,
                run_id: Some(RunId::new(run)),
                attachments: Arc::from([]),
                input_intent: InputIntent::Prompt,
                sent_by_app: false,
                guest: None,
                from_agent: None,
            }),
        });
        MessageId::new(id)
    }

    fn work(&mut self, ago: i64, run: &str, status: ToolStatus, label: &str, item: WorkItem) -> &mut Self {
        self.work_in_attempt(ago, run, status, label, item, None)
    }

    fn work_in_attempt(
        &mut self,
        ago: i64,
        run: &str,
        status: ToolStatus,
        label: &str,
        item: WorkItem,
        attempt: Option<&str>,
    ) -> &mut Self {
        let id = self.id("work");
        self.entries.push(TimelineEntry {
            id: EntryId::new(id.clone()),
            created_at: at(ago),
            kind: EntryKind::Work(WorkLogEntry {
                id: EntryId::new(id),
                run_id: Some(RunId::new(run)),
                created_at: at(ago),
                status,
                label: s(label),
                item,
                superseded_attempt: attempt.map(AttemptId::new),
                source: None,
            }),
        });
        self
    }

    fn entry(&mut self, ago: i64, kind: EntryKind) -> &mut Self {
        let id = self.id("entry");
        self.entries.push(TimelineEntry { id: EntryId::new(id), created_at: at(ago), kind });
        self
    }
}

fn command(cmd: &str, output: &str, exit: i32) -> WorkItem {
    WorkItem::Command { command: s(cmd), output: Some(s(output)), exit_code: Some(exit) }
}

fn read(paths: &[&str], output: &str) -> WorkItem {
    WorkItem::FileRead { paths: paths.iter().map(|p| s(p)).collect(), output: Some(s(output)) }
}

fn edit(changes: &[(&str, u32, u32)]) -> WorkItem {
    WorkItem::FileChange {
        changes: changes
            .iter()
            .map(|(path, a, d)| FileChange { path: s(path), additions: *a, deletions: *d, operation: s("update") })
            .collect(),
        raw_diff: None,
    }
}

fn subagent(id: &str, run: &str, turn: &str, title: &str, status: SubagentStatus, model: &str, elapsed: i64) -> EntryKind {
    EntryKind::Event(TimelineEvent::Subagent(Subagent {
        id: s(id),
        run_id: Some(RunId::new(run)),
        provider_turn_id: Some(s(turn)),
        title: s(title),
        detail: None,
        status,
        model: s(model),
        elapsed_ms: Some(elapsed),
    }))
}

/// Every markdown element the chat renders.
pub const MARKDOWN_SHOWCASE: &str = r#"# Sync engine audit

The **reconnect path** drops the *pending cursor* when the socket closes mid-batch, and ~~retries~~ never resumes. See `packages/client-runtime/src/sync/cursor.rs:142` and [the RFC](https://example.com/rfc/sync) for context. The fix touches /workspace/apps/server/src/sync/session.rs. Inline math: $O(n \log n)$.

## What changed

- Persist the cursor before acknowledging a batch
  - Write-ahead in the same transaction
    - Fsync only on batch boundaries
- Replay from the persisted cursor on reconnect
- [x] Unit coverage for the replay window
- [ ] Soak test on a flaky network

### Rollout order

1. Ship the server change behind `sync.persistCursor`
2. Flip the flag for internal users
   1. Watch reconnect latency
   2. Compare duplicate deliveries
      1. By region
3. Remove the flag

9. Ninth step keeps its number
10. Tenth widens the gutter

#### Numbers

| Metric | Before | After | Change |
| :--- | ---: | ---: | :---: |
| Reconnect p50 | 840 ms | 120 ms | −86% |
| Duplicate deliveries | 3.1% | 0.0% | fixed |
| Batch size | 256 | 256 | — |

> Reconnects happened every 40 seconds on the test network, which is why this showed up at all.

> [!NOTE]
> The cursor is per environment, not per thread.

> [!TIP]
> Run `pnpm test sync` to replay the captured traffic.

> [!IMPORTANT]
> Servers older than 0.1.9 ignore the persisted cursor.

> [!WARNING]
> Clearing the cache resets every cursor.

> [!CAUTION]
> Do not ship the flag on without the migration.

---

##### Code

```rust title=cursor.rs
/// Persists the cursor before the batch is acknowledged.
pub fn persist(store: &mut Store, cursor: Cursor) -> Result<(), SyncError> {
    let tx = store.begin()?;
    tx.write(CURSOR_KEY, &cursor.encode())?;
    tx.commit().map_err(SyncError::from) // fsync happens here
}
```

```ts
export async function replay(from: Cursor): Promise<number> {
  const batch = await fetchBatch({ after: from, limit: 256 });
  return batch.items.length;
}
```

```bash
pnpm --filter @t3tools/server test -- --grep "cursor" && echo "ok"
```

```json
{ "persistCursor": true, "batchSize": 256, "regions": ["iad", "fra"] }
```

```text
plain output keeps the code colour
```

###### Notes

![Architecture sketch](fixtures/fixture-photo.svg)

<details>
<summary>Why not a vector clock?</summary>

A vector clock would need every client to keep state per peer. The server already orders events.

</details>

$$
T_{reconnect} = T_{handshake} + T_{replay}
$$

The replay window is bounded by the batch size.[^1]

[^1]: 256 events at most, about 40 KB.
"#;

fn long_rust_code(lines: usize, seed: usize) -> String {
    let mut code = String::from("```rust\n");
    for line in 0..lines {
        let n = line + seed;
        match line % 6 {
            0 => code.push_str(&format!("/// Handles batch {n} of the replay window.\n")),
            1 => code.push_str(&format!("pub fn handle_{n}(store: &mut Store, batch: &Batch) -> Result<usize, SyncError> {{\n")),
            2 => code.push_str(&format!("    let cursor = store.cursor(\"env-{n}\")?.unwrap_or_default();\n")),
            3 => code.push_str(&format!("    let applied = batch.items.iter().filter(|item| item.seq > cursor.seq).count(); // {n}\n")),
            4 => code.push_str("    store.persist(cursor.advance(applied as u64))?;\n"),
            _ => code.push_str("    Ok(applied)\n}\n"),
        }
    }
    code.push_str("```\n");
    code
}

/// The scene thread: every row kind and markdown element.
pub fn fixture_thread() -> TimelineInput {
    let mut b = Builder::new();
    let image = Attachment { name: s("sketch.svg"), kind: AttachmentKind::Image, source: s("fixtures/fixture-photo.svg") };
    let file = Attachment { name: s("trace.log"), kind: AttachmentKind::File, source: s("trace.log") };

    // Turn A: finished, folded behind "Worked for …", with the markdown showcase and a diff card.
    b.user(3_600, "Audit the sync engine's reconnect path. Something drops events after a network blip.", InputIntent::Prompt, vec![image, file]);
    b.work(3_590, "run-a", ToolStatus::Completed, "Thought", WorkItem::Reasoning { text: s("Start with the socket close handler, then the cursor store.") });
    b.work(3_585, "run-a", ToolStatus::Completed, "Read", read(&["packages/client-runtime/src/sync/cursor.rs"], "pub struct Cursor { seq: u64 }"));
    b.work(3_580, "run-a", ToolStatus::Completed, "Command", command("rg -n persist packages", "cursor.rs:142: persist()", 0));
    b.work(3_560, "run-a", ToolStatus::Completed, "Edit", edit(&[("packages/client-runtime/src/sync/cursor.rs", 24, 6)]));
    let showcase = b.assistant(3_470, "run-a", MARKDOWN_SHOWCASE, false);

    // Turn B: failed, so it stays unfolded and shows its work rows.
    b.user(3_000, "Can you also check the server side?", InputIntent::Prompt, vec![]);
    b.user(2_995, "And keep the migration small — one file if you can.", InputIntent::Prompt, vec![]);
    b.entry(2_990, EntryKind::Event(TimelineEvent::Divider(Divider {
        kind: DividerKind::Forked,
        run_id: None,
        label: s("Forked from Sync engine audit"),
        detail: Some(s("at turn 1")),
        action: true,
    })));
    b.work(2_980, "run-b", ToolStatus::Completed, "Read", read(&["apps/server/src/sync/session.rs"], "impl Session {\n    pub fn close(&mut self) { /* … */ }\n}"));
    b.work(2_978, "run-b", ToolStatus::Completed, "Read", read(&["apps/server/src/sync/batch.rs"], "pub struct Batch;"));
    b.work(2_976, "run-b", ToolStatus::Completed, "Search", WorkItem::FileSearch {
        pattern: s("fn persist"),
        matches: Arc::from([SearchMatch { path: s("apps/server/src/sync/cursor.rs"), line: 88, column: 5, preview: s("pub fn persist(&self) -> Result<()>") }]),
    });
    b.work(2_974, "run-b", ToolStatus::Completed, "Viewed image", WorkItem::ViewedImage { path: s("fixtures/fixture-photo.svg") });
    b.work(2_960, "run-b", ToolStatus::Completed, "Command", command("pnpm --filter @t3tools/server test sync", "✓ 42 tests passed (1.8s)", 0));
    b.work(2_950, "run-b", ToolStatus::Completed, "Todo", WorkItem::TodoList {
        steps: Arc::from([
            (TodoState::Completed, s("Persist the cursor")),
            (TodoState::Running, s("Replay on reconnect")),
            (TodoState::Pending, s("Soak test")),
        ]),
    });
    b.work(2_945, "run-b", ToolStatus::Completed, "Edit", edit(&[("apps/server/src/sync/session.rs", 12, 3)]));
    b.work_in_attempt(2_940, "run-b", ToolStatus::Stopped, "Command", command("pnpm build", "", 0), Some("attempt-b1"));
    b.work_in_attempt(2_938, "run-b", ToolStatus::Completed, "Read", read(&["apps/server/package.json"], "{}"), Some("attempt-b1"));
    b.user(2_935, "Skip the build, just run the sync tests.", InputIntent::Steer, vec![]);
    b.work(2_930, "run-b", ToolStatus::Completed, "Question", WorkItem::QuestionAnswer {
        pairs: Arc::from([(s("Which regions should the soak test cover?"), s("iad and fra"))]),
    });
    b.entry(2_920, subagent("sub-1", "run-b", "turn-7", "Profile reconnect latency", SubagentStatus::Completed, "Claude", 84_000));
    b.entry(2_919, subagent("sub-2", "run-b", "turn-7", "Write the migration", SubagentStatus::Running, "GPT", 41_000));
    b.entry(2_918, subagent("sub-3", "run-b", "turn-7", "Check older servers", SubagentStatus::Failed, "Gemini", 12_000));
    b.work(2_910, "run-b", ToolStatus::Completed, "Context compacted", WorkItem::Compaction {
        status: CompactionStatus::Completed,
        tokens_before: Some(182_340),
        tokens_after: Some(24_100),
    });
    b.work(2_905, "run-b", ToolStatus::Completed, "Notice", WorkItem::SystemNotice { info: false, text: s("Provider restarted after a crash") });
    b.entry(2_900, EntryKind::ProposedPlan(ProposedPlan {
        run_id: Some(RunId::new("run-b")),
        title: s("Plan: persist the sync cursor"),
        markdown: s("1. Add `cursor` column to `sessions`\n2. Write it in the batch transaction\n3. Replay from it on reconnect\n\n```sql\nALTER TABLE sessions ADD COLUMN cursor BIGINT;\n```\n\n- Keep the old path for one release\n- Remove the flag after the soak test"),
    }));
    b.entry(2_890, EntryKind::SentMessage { run_id: Some(RunId::new("run-b")), text: s("Heads up: the soak test needs **30 minutes**.") });
    b.entry(2_885, EntryKind::HtmlRender { run_id: Some(RunId::new("run-b")), title: s("Reconnect latency chart") });
    b.entry(2_880, EntryKind::McpApp { run_id: Some(RunId::new("run-b")), title: s("Grafana · sync dashboard") });
    b.entry(2_875, subagent("sub-4", "run-b", "turn-8", "Summarize findings", SubagentStatus::Idle, "Claude", 9_000));
    let reply_b = b.assistant(2_860, "run-b", "The server persists the cursor in `session.rs` now. Tests pass; the soak test is next.", false);
    b.work(2_850, "run-b", ToolStatus::Completed, "Command", command("git diff --stat", " 2 files changed, 36 insertions(+), 9 deletions(-)", 0));
    b.work(2_848, "run-b", ToolStatus::Failed, "Command", command("pnpm lint", "error: unused import `Cursor`", 1));
    b.work(2_840, "run-b", ToolStatus::Failed, "Usage limit", WorkItem::Error {
        message: s("You've hit your usage limit for this model."),
        usage_limit_retry: Some(s("4:00 PM (UTC)")),
        retryable: true,
    });
    b.entry(2_830, EntryKind::Event(TimelineEvent::Divider(Divider {
        kind: DividerKind::RunInterrupted,
        run_id: Some(RunId::new("run-b")),
        label: s("Run interrupted"),
        detail: Some(s("provider exited")),
        action: false,
    })));
    b.entry(2_820, EntryKind::Event(TimelineEvent::Divider(Divider {
        kind: DividerKind::NewSession,
        run_id: None,
        label: s("New session"),
        detail: None,
        action: false,
    })));

    // Turn C: running.
    b.user(90, "Start the dev server and run the soak test.", InputIntent::Prompt, vec![]);
    b.assistant(80, "run-c", "Starting the dev server first.", true);
    b.work(78, "run-c", ToolStatus::Completed, "Thought", WorkItem::Reasoning { text: s("**Plan the soak run.** The dev server must be up before the soak client connects.") });
    b.work(75, "run-c", ToolStatus::InProgress, "Command", command("pnpm dev --port 4310", "ready in 812 ms", 0));
    b.work(74, "run-c", ToolStatus::Completed, "Read", read(&["scripts/soak.ts"], "export async function soak() {}"));
    b.work(60, "run-c", ToolStatus::InProgress, "Approval", WorkItem::Approval {
        request_id: RequestId::new("req-1"),
        kind: ApprovalKind::Command,
        detail: s("pnpm soak --regions iad,fra --duration 30m"),
        reason: Some(s("Runs a 30 minute network soak against the dev server")),
    });
    b.work(55, "run-c", ToolStatus::Completed, "Todo", WorkItem::TodoList {
        steps: Arc::from([(TodoState::Completed, s("Start dev server")), (TodoState::Running, s("Soak test"))]),
    });
    b.work(40, "run-c", ToolStatus::Failed, "Command", command("curl -sf localhost:4310/health", "curl: (7) Failed to connect", 7));

    TimelineInput {
        entries: Arc::new(b.entries),
        pending_approval_ids: vec![RequestId::new("req-1")],
        latest_run: Some(LatestRun {
            run_id: RunId::new("run-c"),
            status: RunStatus::Running,
            started_at: Some(at(88)),
            completed_at: None,
            turn_ended: false,
        }),
        is_working: true,
        active_turn_started_at: Some(at(83)),
        turn_diff_summaries: Arc::from([
            TurnDiffSummary {
                assistant_message_id: showcase,
                files: Arc::from([
                    FileChange { path: s("packages/client-runtime/src/sync/cursor.rs"), additions: 24, deletions: 6, operation: s("update") },
                    FileChange { path: s("packages/client-runtime/src/sync/replay.rs"), additions: 51, deletions: 0, operation: s("create") },
                    FileChange { path: s("README.md"), additions: 3, deletions: 1, operation: s("update") },
                ]),
            },
            TurnDiffSummary {
                assistant_message_id: reply_b,
                files: Arc::from([FileChange { path: s("apps/server/src/sync/session.rs"), additions: 12, deletions: 3, operation: s("update") }]),
            },
        ]),
        supports_rollback: true,
        worktree_setup: Some(WorktreeSetup {
            started_at: at(3_598),
            running: false,
            branch: s("sync-cursor"),
            steps: Arc::from([(s("Create worktree"), ToolStatus::Completed), (s("Install dependencies"), ToolStatus::Completed)]),
        }),
        queued: Arc::from([
            QueuedMessage { id: s("q-1"), text: s("Then open a PR with the soak numbers."), created_at: at(20), image_count: 0, guest: false, editing: false },
            QueuedMessage { id: s("q-2"), text: s("And ping the infra channel."), created_at: at(10), image_count: 0, guest: false, editing: true },
        ]),
        pending_question: Some(PendingQuestion {
            request_id: RequestId::new("q-req"),
            header: s("Soak test"),
            question: s("How long should the soak test run?"),
            options: Arc::from([
                (s("30 minutes"), s("Enough to see three reconnect cycles")),
                (s("2 hours"), s("Catches slow leaks")),
            ]),
            multi_select: false,
            index: 0,
            count: 2,
        }),
        item_sources: Default::default(),
    }
}

/// A finished thread of `turns` turns: each is a prompt, a folded work
/// group and a reply, every other one with a long code block. Three rows per turn.
pub fn perf_thread(turns: usize) -> TimelineInput {
    let mut b = Builder::new();
    for turn in 0..turns {
        let ago = ((turns - turn) * 120) as i64;
        let run = format!("run-{turn}");
        b.user(ago, &format!("Turn {turn}: tighten the replay loop and show me the handler."), InputIntent::Prompt, vec![]);
        b.work(ago - 10, &run, ToolStatus::Completed, "Read", read(&["src/replay.rs"], "fn replay() {}"));
        b.work(ago - 20, &run, ToolStatus::Completed, "Command", command("cargo test replay", "ok", 0));
        let body = if turn % 2 == 0 {
            format!("Here is the handler for turn {turn}.\n\n{}\nIt skips items at or below the cursor, then **persists** the new position.", long_rust_code(60 + (turn % 5) * 30, turn))
        } else {
            format!("Turn {turn} is done. The loop now batches writes:\n\n- fewer fsyncs\n- one transaction per batch\n\nSee `src/replay.rs:{turn}` for the change.")
        };
        b.assistant(ago - 60, &run, &body, false);
    }
    TimelineInput { entries: Arc::new(b.entries), ..Default::default() }
}

/// A long reply with prose, lists and code, for streaming measurements.
pub fn streaming_reply(chars: usize) -> String {
    let mut text = String::new();
    let mut section = 0;
    while text.len() < chars {
        section += 1;
        text.push_str(&format!("## Step {section}\n\nThe replay loop reads the persisted cursor, skips anything at or below it, and applies the rest in one transaction. That keeps duplicate deliveries at zero.\n\n- batch {section}: fewer fsyncs\n- batch {section}: one transaction\n\n"));
        text.push_str(&long_rust_code(18, section));
        text.push('\n');
    }
    text.truncate(chars);
    text
}

/// A thread whose last turn is streaming `reply`.
pub fn streaming_thread(history: TimelineInput, reply: &str) -> TimelineInput {
    let mut entries = (*history.entries).clone();
    let prompt_id = format!("stream-user-{}", entries.len());
    entries.push(TimelineEntry {
        id: EntryId::new(prompt_id.clone()),
        created_at: at(30),
        kind: EntryKind::Message(ChatMessage {
            id: MessageId::new(prompt_id),
            role: MessageRole::User,
            text: s("Walk me through the replay loop step by step."),
            created_at: at(30),
            updated_at: at(30),
            streaming: false,
            run_id: None,
            attachments: Arc::from([]),
            input_intent: InputIntent::Prompt,
            sent_by_app: false,
            guest: None,
            from_agent: None,
        }),
    });
    entries.push(TimelineEntry {
        id: EntryId::new("stream-reply"),
        created_at: at(20),
        kind: EntryKind::Message(ChatMessage {
            id: MessageId::new("stream-reply"),
            role: MessageRole::Assistant,
            text: s(reply),
            created_at: at(20),
            updated_at: at(0),
            streaming: true,
            run_id: Some(RunId::new("run-stream")),
            attachments: Arc::from([]),
            input_intent: InputIntent::Prompt,
            sent_by_app: false,
            guest: None,
            from_agent: None,
        }),
    });
    TimelineInput {
        entries: Arc::new(entries),
        latest_run: Some(LatestRun {
            run_id: RunId::new("run-stream"),
            status: RunStatus::Running,
            started_at: Some(at(25)),
            completed_at: None,
            turn_ended: false,
        }),
        is_working: true,
        active_turn_started_at: Some(at(25)),
        ..history
    }
}
