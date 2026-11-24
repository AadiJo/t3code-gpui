"""Extracts the reference world the Mac captures were taken in from the reference server's
`statev2.sqlite` into `apps/web/src/fixtures/reference_world.json`, which
`FixtureStore::reference()` decodes and projects through the live timeline mapper.

  python3 -I mac_world.py <statev2.sqlite> <workspaces-dir> <t3code-checkout> [shutdown-iso]

<t3code-checkout> is the "t3code" project's workspace as the captures saw it (with the four
uncommitted files the seed copied in); its files and working-tree patch feed the Files and Diff
views and the details card's Changes row.

Every path under <workspaces-dir> becomes `/work/...`; the script refuses to write a file that
still contains an absolute home path. The server was stopped after the captures, which cancelled
the runs still going (the approval, the question, the working thread). Records cancelled at or
after `shutdown-iso` (default 2026-11-24T04:30:00Z) are put back into the state the captures saw,
and the "Server shut down" notices are dropped.
"""
import base64
import json
import subprocess
import re
import sqlite3
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / "apps/web/src/fixtures/reference_world.json"
WAITING_ITEMS = {"approval_request", "user_input_request"}


def rows(db, table, thread_id, order="rowid"):
    return [json.loads(r[0]) for r in db.execute(f"select payload_json from {table} where thread_id=? order by {order}", (thread_id,))]


def revive(record, cutoff, status):
    """Puts a record cancelled by the shutdown back to `status`."""
    done = record.get("completedAt") or record.get("resolvedAt")
    if record.get("status") != "cancelled" or not done or done < cutoff:
        return record
    record["status"] = status
    for key in ("completedAt", "resolvedAt"):
        if key in record:
            record[key] = None
    if "updatedAt" in record and record.get("startedAt"):
        record["updatedAt"] = record["startedAt"]
    return record


def thread_projection(db, thread_id, cutoff):
    thread = json.loads(db.execute("select payload_json from orchestration_v2_projection_threads where thread_id=?", (thread_id,)).fetchone()[0])
    requests = rows(db, "orchestration_v2_projection_runtime_requests", thread_id)
    waiting = any(r.get("status") in ("pending", "cancelled") for r in requests)
    items = []
    for item in rows(db, "orchestration_v2_projection_turn_items", thread_id, "run_id, ordinal"):
        if item["type"] == "system_notice" and item.get("startedAt", "") >= cutoff:
            continue
        items.append(revive(item, cutoff, "waiting" if item["type"] in WAITING_ITEMS else "running"))
    positions = {r[0]: r[1] for r in db.execute("select turn_item_id, ordinal from orchestration_v2_turn_item_positions where thread_id=?", (thread_id,))}
    run_status = "waiting" if waiting else "running"
    projection = {
        "thread": thread,
        "runs": [revive(r, cutoff, run_status) for r in rows(db, "orchestration_v2_projection_runs", thread_id, "ordinal")],
        "attempts": [revive(r, cutoff, "running") for r in rows(db, "orchestration_v2_projection_run_attempts", thread_id)],
        "nodes": [revive(r, cutoff, "waiting" if r.get("runtimeRequestId") else "running") for r in rows(db, "orchestration_v2_projection_nodes", thread_id)],
        "subagents": [],
        "providerSessions": rows(db, "orchestration_v2_projection_provider_sessions", thread_id),
        "providerThreads": rows(db, "orchestration_v2_projection_provider_threads", thread_id),
        "providerTurns": [revive(r, cutoff, "running") for r in rows(db, "orchestration_v2_projection_provider_turns", thread_id)],
        "runtimeRequests": [revive(r, cutoff, "pending") for r in requests],
        "messages": rows(db, "orchestration_v2_projection_messages", thread_id, "created_at"),
        "plans": rows(db, "orchestration_v2_projection_plans", thread_id),
        "turnItems": items,
        "checkpointScopes": rows(db, "orchestration_v2_projection_checkpoint_scopes", thread_id),
        "checkpoints": rows(db, "orchestration_v2_projection_checkpoints", thread_id),
        "contextHandoffs": [],
        "contextTransfers": [],
        # Rows of the rendered timeline, as (position, turn item id); the fixture builds
        # `visibleTurnItems` from these and `turnItems`, so each item is stored once.
        "positions": sorted([positions.get(item["id"], item["ordinal"]), item["id"]] for item in items),
        "updatedAt": thread["updatedAt"],
    }
    return projection


def checkout(root):
    """The checkout's files (binary ones base64) and its working-tree patch, untracked files included."""
    root = Path(root)
    git = ["git", "--no-optional-locks", "-C", str(root)]
    files = {}
    for path in sorted(p for p in root.rglob("*") if p.is_file() and ".git" not in p.relative_to(root).parts):
        rel = path.relative_to(root).as_posix()
        data = path.read_bytes()
        try:
            files[rel] = {"text": data.decode("utf-8")}
        except UnicodeDecodeError:
            files[rel] = {"base64": base64.b64encode(data).decode()}
    patch = subprocess.run(git + ["diff"], capture_output=True, text=True, check=True).stdout
    untracked = subprocess.run(git + ["ls-files", "--others", "--exclude-standard"], capture_output=True, text=True, check=True).stdout.split()
    for rel in untracked:
        if "text" in files[rel]:
            body = files[rel]["text"]
            lines = body.splitlines()
            patch += f"diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n@@ -0,0 +1,{len(lines)} @@\n"
            patch += "".join(f"+{line}\n" for line in lines)
        else:
            patch += f"diff --git a/{rel} b/{rel}\nnew file mode 100644\nBinary files /dev/null and b/{rel} differ\n"
    return {"files": files, "patch": patch}


def main(db_path, workspaces, t3code_checkout, cutoff="2026-11-24T04:30:00Z"):
    db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    projects = [
        {"id": pid, "title": title, "workspaceRoot": root, "createdAt": created, "updatedAt": updated}
        for pid, title, root, created, updated in db.execute(
            "select project_id, title, workspace_root, created_at, updated_at from projection_projects where deleted_at is null order by created_at"
        )
    ]
    threads = [
        {"projectId": pid, "projection": thread_projection(db, tid, cutoff)}
        for tid, pid in db.execute("select thread_id, project_id from orchestration_v2_projection_threads where deleted_at is null order by created_at")
    ]
    text = json.dumps({"projects": projects, "threads": threads, "workspace": checkout(t3code_checkout)}, ensure_ascii=False, separators=(",", ":"))
    text = text.replace(workspaces.rstrip("/"), "/work")
    leaks = re.findall(r"/(?:home|Users)/[^\"/]+", text)
    if leaks:
        sys.exit(f"refusing to write: absolute home paths remain: {sorted(set(leaks))}")
    OUT.write_text(text + "\n")
    print(f"wrote {OUT} ({len(text) // 1024} KiB, {len(projects)} projects, {len(threads)} threads)")


if __name__ == "__main__":
    main(*sys.argv[1:])
