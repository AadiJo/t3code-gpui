#!/usr/bin/env bash
# Regenerates the t3code-contracts crate from a T3 Code checkout:
#
#   T3CODE_CHECKOUT=/path/to/t3code scripts/contracts/regen.sh
#
# 1. dump.mts exports every wire schema to packages/contracts/schema/rpc-schema.json.
# 2. fixtures.mts encodes sample values with the real codec into schema/wire-fixtures.json.
# 3. gen-rust.mjs writes packages/contracts/src/generated/ from the schema.
# Without T3CODE_CHECKOUT only step 3 runs, from the committed schema.
# The checkout needs its dependencies installed; node must be 24 or newer.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
schema="$root/packages/contracts/schema"
mkdir -p "$schema"
if [ -n "${T3CODE_CHECKOUT:-}" ]; then
  cd "$T3CODE_CHECKOUT/packages/contracts"
  node "$here/dump.mts" "$schema/rpc-schema.json"
  node "$here/fixtures.mts" "$schema/rpc-schema.json" "$schema/wire-fixtures.json" "${FIXTURE_SAMPLES:-8}"
else
  echo "T3CODE_CHECKOUT not set: generating Rust from the committed schema only" >&2
fi
node "$here/gen-rust.mjs" "$schema/rpc-schema.json" "$root/packages/contracts/src/generated"
