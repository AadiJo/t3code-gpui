#!/usr/bin/env bash
# Renders every parity scene with the port and compares it with the fork.
# Usage: scripts/parity/run.sh [--owner] <out-dir> [reference-png-dir]
# --owner renders at the owner's Contrast 95 and Glass opacity 40 and judges
# against expected-owner.json (a fork capture at those settings); without it
# the scenes use contrast 100 and glass 45 against expected.json.
# Needs CARGO_TARGET_DIR (and the dev sysroot env) set as for any build.
# Writes <out-dir>/port/*.png, <out-dir>/report/report.md and results.json,
# and side-by-side images when the reference PNGs are given.
# After editing gen_spec.py, regenerate spec.json and re-record the expected
# files from the reference PNGs: python3 gen_spec.py spec.json &&
# python3 compare.py record <ref-dir> (PARITY_EXPECTED=expected-owner.json for the owner capture)
set -euo pipefail
if [[ "${1:-}" == "--owner" ]]; then
  shift
  export T3CODE_CONTRAST=95 T3CODE_GLASS_OPACITY=40 PARITY_EXPECTED=expected-owner.json
fi
out=${1:?usage: run.sh [--owner] <out-dir> [reference-png-dir]}
ref=${2:-}
here=$(cd "$(dirname "$0")" && pwd)
cargo build -q --features headless -p t3code-desktop --bin parity-shot
"${CARGO_TARGET_DIR:-target}/debug/parity-shot" "$out/port"
python3 "$here/compare.py" compare "$out/port" "$out/report" ${ref:+"$ref"}
