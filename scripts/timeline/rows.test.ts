// Derives the web's timeline entries and rows for every dumped projection and writes one
// JSON per projection: entries (id, kind, label, detail, status) and rows with each
// grouped entry's display label, as the fork's components compute them.
import * as fs from "node:fs";
import * as Schema from "effect/Schema";
import * as DateTime from "effect/DateTime";
import { test } from "vite-plus/test";
import { OrchestrationV2ThreadProjection } from "@t3tools/contracts";

const REF = `${process.env.T3CODE_REF}/apps/web/src`;
const IN = process.env.T3_PROJECTIONS ?? "";
const OUT = process.env.T3_ROWS_OUT ?? "";

test("dump timeline rows", async () => {
  const session = await import(`${REF}/session-logic.ts`);
  const logic = await import(`${REF}/components/chat/MessagesTimeline.logic.ts`);
  const checkpoints = await import(`${process.env.T3CODE_REF}/packages/client-runtime/src/state/threadCheckpoints.ts`);
  const decode = Schema.decodeUnknownSync(Schema.toCodecJson(OrchestrationV2ThreadProjection));
  fs.mkdirSync(OUT, { recursive: true });
  for (const file of fs.readdirSync(IN).filter((name) => name.endsWith(".json")).sort()) {
    const encoded = JSON.parse(fs.readFileSync(`${IN}/${file}`, "utf8")) as unknown[];
    const threads = encoded.map((raw, index) => {
      const p = decode(raw);
      const summaries = checkpoints.deriveThreadCheckpointSummaries(p);
      const entries = session.deriveTimelineEntriesFromVisibleTurnItems({
        visibleTurnItems: p.visibleTurnItems,
        optimisticMessages: [],
        attempts: p.attempts,
        nodes: p.nodes,
        plans: p.plans,
      });
      const pending = new Set(
        p.runtimeRequests.filter((r: any) => r.status === "pending").map((r: any) => r.id),
      );
      const runs = p.runs.filter((r: any) => r.status !== "queued");
      const latest = runs.reduce((a: any, b: any) => (a && a.ordinal > b.ordinal ? a : b), null);
      const active = ["preparing", "starting", "running", "waiting"].includes(latest?.status);
      const derive = (expandedRunIds?: Set<any>, expandedAttemptIds?: Set<any>, expandedWorkGroupIds?: Set<string>) => logic.deriveMessagesTimelineRows({
        timelineEntries: entries,
        expandedRunIds,
        expandedAttemptIds,
        expandedWorkGroupIds,
        pendingApprovalRequestIds: pending,
        latestRun: latest
          ? {
              runId: latest.id,
              status: latest.status,
              startedAt: latest.startedAt ? DateTime.formatIso(latest.startedAt) : null,
              completedAt: latest.completedAt ? DateTime.formatIso(latest.completedAt) : null,
              turnEnded: latest.completedAt != null,
            }
          : null,
        isWorking: active,
        turnDiffSummaries: summaries,
        supportsConversationRollback: false,
      });
      const rows = derive();
      // Everything open: every run fold, attempt fold and tool group, iterated until stable.
      const runIds = new Set(p.runs.map((r: any) => r.id));
      const attemptIds = new Set(p.attempts.map((a: any) => a.id));
      const groups = new Set<string>();
      let expandedRows = derive(runIds, attemptIds, groups);
      for (let i = 0; i < 5; i++) {
        const before = groups.size;
        for (const r of expandedRows) if (r.kind === "work-toggle" || r.kind === "work-live") groups.add(r.groupId);
        if (groups.size === before) break;
        expandedRows = derive(runIds, attemptIds, groups);
      }
      const workEntry = (e: any) => ({
        id: e.id,
        label: logic.workEntryDisplayLabel(e, undefined),
        rawLabel: e.label,
        detail: e.detail ?? null,
        status: e.toolLifecycleStatus ?? null,
        tone: e.tone,
        itemType: e.itemType ?? null,
      });
      return {
        index,
        threadId: p.thread.id,
        entries: entries.map((e: any) => ({
          id: e.id,
          kind: e.kind,
          ...(e.kind === "work"
            ? { label: e.entry.label, detail: e.entry.detail ?? null, status: e.entry.toolLifecycleStatus ?? null, display: logic.workEntryDisplayLabel(e.entry, undefined) }
            : e.kind === "message"
              ? { label: e.message.role, detail: e.message.text.slice(0, 120) }
              : e.kind === "event"
                ? { label: e.projectedItem.item.type, status: e.projectedItem.item.type === "subagent" ? e.projectedItem.item.status : null }
                : e.kind === "sent-message"
                  ? { detail: e.text }
                  : {}),
        })),
        checkpoints: summaries.map((c: any) => ({ assistantMessageId: c.assistantMessageId, turnCount: c.checkpointTurnCount, files: c.files.map((f: any) => `${f.kind}:${f.path}+${f.additions}-${f.deletions}`) })),
        rows: rows.map(rowOut),
        expandedRows: expandedRows.map(rowOut),
      };
      function rowOut(r: any) {
          switch (r.kind) {
            case "work":
              // The heading TimelineWorkEntryRow draws: displayLabel, else the entry's own label.
              return { id: r.id, kind: r.kind, label: r.displayLabel ?? (r.groupedEntries.length === 1 ? logic.workEntryDisplayLabel(r.groupedEntries[0], undefined) : null), entries: r.groupedEntries.map(workEntry) };
            case "work-live":
              return { id: r.id, kind: r.kind, label: logic.liveWorkEntryLabel(r.entry, undefined, r.active), entries: r.groupedEntries.map(workEntry) };
            case "work-toggle":
              return { id: r.id, kind: r.kind, label: r.summary, detail: String(r.hiddenCount), status: r.hasFailure ? "failed" : null, summaryKind: r.summaryKind };
            case "message":
              return { id: r.id, kind: r.kind, label: r.message.role, detail: r.message.text.slice(0, 120), showMeta: r.showAssistantMeta, status: r.assistantTurnDiffSummary && r.assistantTurnDiffSummary.files.length > 0 ? `diff:${r.assistantTurnDiffSummary.files.map((f: any) => `${f.path}+${f.additions}-${f.deletions}`).join(",")}` : null };
            case "context-compaction":
              return { id: r.id, kind: r.kind, label: r.label, detail: r.title ?? null, status: r.failed ? "failed" : r.active ? "active" : null };
            case "turn-fold":
            case "attempt-fold":
              return { id: r.id, kind: r.kind, label: r.label };
            case "event":
              return { id: r.id, kind: r.kind, label: r.projectedItem.item.type, status: r.projectedItem.item.type === "subagent" ? r.projectedItem.item.status : null, detail: (r.subagents ?? []).length ? String(r.subagents.length) : null };
            case "sent-message":
              return { id: r.id, kind: r.kind, detail: r.text };
            case "thinking":
              return { id: r.id, kind: r.kind, label: r.label ?? null };
            default:
              return { id: r.id, kind: r.kind };
          }
      }
    });
    fs.writeFileSync(`${OUT}/${file}`, JSON.stringify(threads, null, 1));
  }
}, 600_000);

// The command labeller over the corpus: display text, program name and highlight language.
test("dump command labels", async () => {
  const label = await import(`${process.env.T3CODE_REF}/packages/client-runtime/src/work-log/commandLabel.ts`);
  const here = process.env.T3_CORPUS_DIR + "/";
  const corpus = JSON.parse(fs.readFileSync(here + "corpus.json", "utf8")) as string[];
  const out = corpus.map((command) => ({
    command,
    display: label.commandDisplayText(command),
    program: label.commandProgramName(command),
    language: label.commandHighlightLanguage(command),
  }));
  fs.writeFileSync(`${OUT}/../commands-ts.json`, JSON.stringify(out, null, 1));
});
