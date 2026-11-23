#!/usr/bin/env bash
# Renders every parity scene with the port and compares it with the fork.
# Usage: scripts/parity/run.sh <out-dir> [reference-png-dir]
# Needs CARGO_TARGET_DIR (and the dev sysroot env) set as for any build.
# Writes <out-dir>/port/*.png, <out-dir>/report/report.md and results.json,
# and side-by-side images when the reference PNGs are given.
# After editing gen_spec.py, regenerate spec.json and re-record expected.json
# from the reference PNGs: python3 gen_spec.py spec.json && python3 compare.py record <ref-dir>
set -euo pipefail
out=${1:?usage: run.sh <out-dir> [reference-png-dir]}
ref=${2:-}
here=$(cd "$(dirname "$0")" && pwd)
cargo build -q --features headless -p t3code-desktop --bin parity-shot
"${CARGO_TARGET_DIR:-target}/debug/parity-shot" "$out/port"
python3 "$here/compare.py" compare "$out/port" "$out/report" ${ref:+"$ref"}
