#!/usr/bin/env bash
# Compares the port's timeline with the web's over real thread projections.
# Usage: T3CODE_REF=<TypeScript checkout> scripts/timeline/run.sh <out-dir>
# Needs CARGO_TARGET_DIR and the dev sysroot env, as any build. Steps:
#  1. Once per out-dir: replays the server's recorded provider fixtures (Claude, Codex, Cursor,
#     OpenCode, Kiro, Grok, Pi, fx, Muse, ACP) in vitest with a plugin that writes each final
#     projection to <out>/projections (the reference checkout is not modified).
#  2. Adds a synthetic projection with every T3 Code MCP tool.
#  3. Derives the web's entries and rows (`deriveTimelineEntriesFromVisibleTurnItems`,
#     `deriveMessagesTimelineRows`, `workEntryDisplayLabel`) and the port's (`live-check timeline`).
#  4. Writes <out>/report.md (rows matched by kind and id, then label, detail and status) and
#     <out>/commands-diff.md (the command labeller over the test corpus).
set -euo pipefail
out=$(mkdir -p "${1:?usage: run.sh <out-dir>}" && cd "$1" && pwd)
ref=${T3CODE_REF:?set T3CODE_REF to the TypeScript checkout}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
harness=$out/harness
mkdir -p "$harness" "$out/projections" "$out/ts" "$out/rust"
cp "$here"/rows.test.ts "$here"/gen.test.ts "$here"/rows.config.mts "$here"/corpus.mjs "$harness/"
ln -sfn "$ref/apps/web/node_modules" "$harness/node_modules"
if [[ -z "$(ls "$out/projections" | grep -v synthetic || true)" ]]; then
  # A global commit hook can reject the fixtures' `initial` commit; hooks stay off for this run.
  ( cd "$ref/apps/server" && GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null \
      T3_DUMP_DIR="$out/projections" ./node_modules/.bin/vp test run --config "$here/dump.config.mts" \
      src/orchestration-v2/testkit/*ReplayFixtures.integration.test.ts )
fi
python3 -I "$here/synthetic.py" "$out/projections" "$repo/packages/client-runtime/src/work_log/t3_tools.rs"
python3 -I "$here/corpus.py" "$out/projections" "$ref" "$harness/corpus-src.json"
node "$harness/corpus.mjs" >/dev/null
( cd "$ref/apps/web" && T3CODE_REF="$ref" T3_HARNESS_DIR="$harness" T3_PROJECTIONS="$out/projections" \
    T3_ROWS_OUT="$out/ts" T3_CORPUS_DIR="$harness" ./node_modules/.bin/vp test run --config "$harness/rows.config.mts" rows )
cargo build -q --features headless -p t3code-desktop --bin live-check
"${CARGO_TARGET_DIR:-target}/debug/live-check" timeline "$out/projections" "$out/rust"
"${CARGO_TARGET_DIR:-target}/debug/live-check" commands "$harness/corpus.json" "$out/commands-rust.json"
python3 -I "$here/compare.py" "$out/ts" "$out/rust" "$out/report.md"
python3 -I - "$out" <<'PY'
import json, sys
out = sys.argv[1]
web, port = json.load(open(f"{out}/commands-ts.json")), json.load(open(f"{out}/commands-rust.json"))
differ = [(a, b) for a, b in zip(web, port) if a != b]
with open(f"{out}/commands-diff.md", "w") as report:
    report.write(f"# Command labeller: web vs port\n\n{len(web)} commands, {len(differ)} differ\n\n")
    for a, b in differ:
        report.write(f"- `{json.dumps(a['command'])[:300]}`\n")
        for key in ("display", "program", "language"):
            if a[key] != b[key]:
                report.write(f"  - {key}: web `{json.dumps(a[key])[:200]}` port `{json.dumps(b[key])[:200]}`\n")
print(f"commands: {len(web)} compared, {len(differ)} differ")
PY
