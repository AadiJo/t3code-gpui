"""Compares the web's and the port's timeline dumps for the same projections.

Usage: python3 -I compare.py <ts-dir> <rust-dir> <out.md>
Each dir holds one JSON per projection file: a list of threads with entries, rows (default
expansion) and expandedRows (every fold and group open). Rows align by (kind, id); matched
rows compare label, detail and status, and grouped work entries compare by id the same way.
"""
import collections
import difflib
import json
import os
import sys

ts_dir, rust_dir, out_path = sys.argv[1:4]
FIELDS = ("label", "detail", "status")

mismatch = collections.defaultdict(list)  # category -> examples
totals = collections.Counter()


def norm(value):
    if isinstance(value, str):
        return value.strip()
    return value


def compare_fields(category, where, a, b):
    ok = True
    for field in FIELDS:
        if field not in a and field not in b:
            continue
        if norm(a.get(field)) != norm(b.get(field)):
            ok = False
            mismatch[f"{category} {field}"].append((where, a.get(field), b.get(field)))
    return ok


def compare_seq(name, layer, ts_items, rust_items):
    keys_a = [(r["kind"], r["id"]) for r in ts_items]
    keys_b = [(r["kind"], r["id"]) for r in rust_items]
    matcher = difflib.SequenceMatcher(a=keys_a, b=keys_b, autojunk=False)
    for tag, i1, i2, j1, j2 in matcher.get_opcodes():
        if tag == "equal":
            for a, b in zip(ts_items[i1:i2], rust_items[j1:j2]):
                totals[f"{layer} matched"] += 1
                kind = a["kind"]
                if layer == "entries" and kind == "work":
                    ok = compare_fields(f"{layer} work", f"{name} {a['id']}", a, b)
                    ok &= compare_fields(f"{layer} work display", f"{name} {a['id']}", {"label": a.get("display")}, {"label": b.get("display")})
                else:
                    ok = compare_fields(f"{layer} {kind}", f"{name} {a['id']}", a, b)
                if "entries" in a or "entries" in b:
                    ok &= compare_group(name, layer, a.get("entries", []), b.get("entries", []))
                totals[f"{layer} equal" if ok else f"{layer} differ"] += 1
        else:
            for a in ts_items[i1:i2]:
                totals[f"{layer} web only"] += 1
                mismatch[f"{layer} web-only {a['kind']}"].append((f"{name} {a['id']}", a.get("label"), None))
            for b in rust_items[j1:j2]:
                totals[f"{layer} port only"] += 1
                mismatch[f"{layer} port-only {b['kind']}"].append((f"{name} {b['id']}", None, b.get("label")))


def compare_group(name, layer, ts_entries, rust_entries):
    by_id = {e["id"]: e for e in rust_entries}
    ok = True
    for a in ts_entries:
        b = by_id.pop(a["id"], None)
        if b is None:
            ok = False
            mismatch[f"{layer} grouped web-only {a['itemType']}"].append((f"{name} {a['id']}", a["label"], None))
            continue
        ok &= compare_fields(f"{layer} grouped {a['itemType']}", f"{name} {a['id']}", a, b)
    for b in by_id.values():
        ok = False
        mismatch[f"{layer} grouped port-only {b['itemType']}"].append((f"{name} {b['id']}", None, b["label"]))
    return ok


for file in sorted(os.listdir(ts_dir)):
    if not file.endswith(".json"):
        continue
    ts = json.load(open(os.path.join(ts_dir, file)))
    rust = json.load(open(os.path.join(rust_dir, file)))
    for a, b in zip(ts, rust):
        name = f"{file[:-5]}[{a['index']}]"
        if "decodeError" in b:
            mismatch["decode"].append((name, None, b["decodeError"]))
            continue
        totals["checkpoints compared"] += len(a["checkpoints"])
        if a["checkpoints"] != b["checkpoints"]:
            mismatch["checkpoint summaries"].append((name, a["checkpoints"], b["checkpoints"]))
        compare_seq(name, "entries", a["entries"], b["entries"])
        compare_seq(name, "rows", a["rows"], b["rows"])
        compare_seq(name, "expanded", a["expandedRows"], b["expandedRows"])

with open(out_path, "w") as out:
    out.write("# Timeline comparison: web vs port\n\n")
    out.write("| measure | count |\n| --- | --- |\n")
    for key in sorted(totals):
        out.write(f"| {key} | {totals[key]} |\n")
    out.write("\n## Differences by category\n\n| category | count | example (where / web / port) |\n| --- | --- | --- |\n")
    for category, items in sorted(mismatch.items(), key=lambda kv: -len(kv[1])):
        where, web, port = items[0]
        cell = f"`{where}` / `{str(web)[:90]}` / `{str(port)[:90]}`".replace("|", "\\|").replace("\n", " ")
        out.write(f"| {category} | {len(items)} | {cell} |\n")
    out.write("\n## All differences\n\n")
    for category, items in sorted(mismatch.items()):
        out.write(f"### {category} ({len(items)})\n\n")
        for where, web, port in items[:40]:
            out.write(f"- `{where}`\n  - web: `{str(web)[:200]}`\n  - port: `{str(port)[:200]}`\n".replace("\n  - ", "\n  - "))
        out.write("\n")
print(dict(totals))
print(len(mismatch), "categories")
