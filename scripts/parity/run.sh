#!/usr/bin/env bash
# Renders every parity scene with the port and compares it with the fork.
# Usage: scripts/parity/run.sh [--owner] <out-dir> [reference-png-dir]
# --owner renders at the owner's Contrast 95 and Glass opacity 40 and judges
# against each scene's "owner" values (a fork capture at those settings); without
# it the scenes use contrast 100 and glass 45 against the "default" values.
# Needs CARGO_TARGET_DIR (and the dev sysroot env) set as for any build.
# PARITY_CLIENT_SETTINGS=<client-settings.json> also renders the owner scenes
# (thread details, palette, inbox at interface 15, 1100x780 and 1710x1074; the
# owner-sb-* sidebar views at 1100x780);
# without it they are skipped and reported missing.
# Writes <out-dir>/port/*.png, <out-dir>/report/report.md and results.json,
# and side-by-side images when the reference PNGs are given.
# Scenes live in specs/<area>.py (registered in gen_spec.py's AREAS); expected
# values in expected/<scene>.json. After adding or changing a scene, record it
# from its reference PNG: python3 compare.py record <ref-dir> <scene>
# (PARITY_MODE=owner for the owner capture).
set -euo pipefail
if [[ "${1:-}" == "--owner" ]]; then
  shift
  export T3CODE_CONTRAST=95 T3CODE_GLASS_OPACITY=40 PARITY_MODE=owner
fi
out=${1:?usage: run.sh [--owner] <out-dir> [reference-png-dir]}
ref=${2:-}
here=$(cd "$(dirname "$0")" && pwd)
cargo build -q --features headless -p t3code-desktop --bin parity-shot
"${CARGO_TARGET_DIR:-target}/debug/parity-shot" "$out/port"
python3 "$here/compare.py" compare "$out/port" "$out/report" ${ref:+"$ref"}
