"""Extracts the reference world the Mac captures were taken in from the reference server's
`statev2.sqlite` into `apps/web/src/fixtures/reference_world.json`, which
`FixtureStore::reference()` decodes and projects through the live timeline mapper.

  python3 -I mac_world.py <statev2.sqlite> <workspaces-dir> <t3code-checkout> [shutdown-iso]
  python3 -I mac_world.py server <server-dump.json> <path=replacement> ...

<t3code-checkout> is the "t3code" project's workspace as the captures saw it (with the four
uncommitted files the seed copied in); its files and working-tree patch feed the Files and Diff
views and the details card's Changes row.

Every path under <workspaces-dir> becomes `/work/...`; the script refuses to write a file that
still contains an absolute home path. The server was stopped after the captures, which cancelled
the runs still going (the approval, the question, the working thread). Records cancelled at or
after `shutdown-iso` (default 2026-11-24T04:30:00Z) are put back into the state the captures saw,
and the "Server shut down" notices are dropped.

`server` writes `apps/web/src/fixtures/reference_server.json` from a dump of what the settings
pages read from the reference server (`subscribeServerConfig`'s snapshot, source control, issue
trackers, scheduled tasks, storage and diagnostics replies, and the t3code checkout's
`projects.searchEntries` replies for the go-to-file queries, as the capture's `dump-server.mjs`
saves them). Each `path=replacement` rewrites a machine path (longest first); the server's host
name and remote-open targets become invented ones; it refuses to write a file that still names
a home directory, the user, the host, a tailnet or a private address.
"""
import base64
import json
import subprocess
import re
import sqlite3
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / "apps/web/src/fixtures/reference_world.json"
SERVER_OUT = OUT.with_name("reference_server.json")
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


def leaks(text, words):
    """Machine-identifying strings left in `text`."""
    found = set(re.findall(r"/(?:home|Users)/(?!scenario\b)[^\"/]+", text))
    found |= {h for h in re.findall(r"[\w.-]+\.ts\.net\b", text) if not h.endswith("example-tailnet.ts.net")}
    found |= {ip for ip in re.findall(r"\b\d{1,3}(?:\.\d{1,3}){3}\b", text) if ip not in ("127.0.0.1", "0.0.0.0", "100.64.0.1")}
    found |= {m for m in re.findall(r"[\w.+-]+@[A-Za-z][\w-]*\.[A-Za-z]{2,}\b", text) if not m.endswith(("example.com", "example.invalid"))}
    found |= {w for w in words if w and w.lower() in text.lower()}
    return sorted(found)


def server(dump_path, *replacements):
    import getpass
    import socket

    dump = json.loads(Path(dump_path).read_text())
    host = dump["config"]["value"]["config"].get("hostname") or socket.gethostname()
    # The pull request and issue listings keep how they answered: `{"ok": value}` or
    # `{"err": typed error}` (the reference server has no GitHub credential).
    replies = {"pullRequests", "issues"}

    def reply(value):
        if "value" in value:
            return {"ok": value["value"]}
        fail = next((c for c in value.get("error") or [] if c.get("_tag") == "Fail"), None)
        return {"err": fail["error"] if fail else {"message": str(value.get("error"))}}

    def entry(key, value):
        if key in replies:
            return reply(value)
        if key == "searchEntries":  # one reply per query
            return {query: answer.get("value") for query, answer in value.items() if "value" in answer}
        return value.get("value")

    text = json.dumps({key: entry(key, value) for key, value in dump.items()}, ensure_ascii=False)
    pairs = sorted((r.split("=", 1) for r in replacements), key=lambda p: -len(p[0]))
    for path, replacement in pairs:
        text = text.replace(path.rstrip("/"), replacement.rstrip("/"))
    # What is left in stack traces and probes: home directories, tailnet hosts and addresses.
    user = getpass.getuser()
    text = re.sub(r"[\w.-]+\.tail[0-9a-f]+\.ts\.net", "scenario.example-tailnet.ts.net", text)
    text = re.sub(r"\b100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.\d{1,3}\.\d{1,3}\b", "100.64.0.1", text)
    text = re.sub(rf"/(home|Users)/{re.escape(user)}\b", "/home/scenario", text)
    text = re.sub(re.escape(host), "scenario-host", text, flags=re.I)
    out = json.loads(text)
    config = out["config"]["config"]
    config["hostname"] = "scenario-host"
    # The server's zone stays: the fork names it in the task editor ("CT"), and the port must too.
    config["remoteOpenTargets"] = [
        {**t, "host": {"tailscale": "scenario.example-tailnet.ts.net", "mdns": "scenario-host.local"}.get(t.get("kind"), "scenario-host"), "username": "scenario"}
        for t in config.get("remoteOpenTargets", [])
    ]
    text = json.dumps(out, ensure_ascii=False, separators=(",", ":"))
    info = Path("/etc/machine-info")
    pretty = re.findall(r'PRETTY_HOSTNAME="?([^"\n]+)', info.read_text()) if info.exists() else []
    found = leaks(text, {getpass.getuser(), host, socket.gethostname(), *pretty})
    if found:
        sys.exit(f"refusing to write: machine names remain: {found}")
    SERVER_OUT.write_text(text + "\n")
    print(f"wrote {SERVER_OUT} ({len(text) // 1024} KiB, {len(config['providers'])} providers)")


if __name__ == "__main__":
    if sys.argv[1:2] == ["server"]:
        server(*sys.argv[2:])
    else:
        main(*sys.argv[1:])
