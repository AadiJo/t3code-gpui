"""Collects command strings for the labeller comparison: every command execution in the
projections plus every string literal in the reference's command-label tests.

Usage: python3 -I corpus.py <projections-dir> <reference-checkout> <out corpus-src.json>
corpus.mjs then evaluates the literals into corpus.json.
"""
import json
import os
import re
import sys

projections, reference, out = sys.argv[1:4]
work_log = f"{reference}/packages/client-runtime/src/work-log"
literal = re.compile(r'"(?:\\.|[^"\\\n])*"|\'(?:\\.|[^\'\\\n])*\'|`(?:\\.|[^`\\$])*`')
sources = []
for name in ["commandLabel.test.ts", "embeddedScripts.test.ts"]:
    sources += literal.findall(open(f"{work_log}/{name}").read())
commands = set()
for name in os.listdir(projections):
    for projection in json.load(open(f"{projections}/{name}")):
        commands.update(item["input"] for item in projection["turnItems"] if item.get("type") == "command_execution")
json.dump({"sources": sources, "commands": sorted(commands)}, open(out, "w"))
