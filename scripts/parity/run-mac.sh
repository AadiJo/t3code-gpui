#!/usr/bin/env bash
# Renders every Mac reference view with a CI build of the port on a Mac over SSH, under the owner
# profile, and judges the images here in the `mac` mode.
# Usage: scripts/parity/run-mac.sh <ci-run-id> <ssh-host> [ref-dir]
#   <ci-run-id>  a run of personal-builds.yml with the t3code-gpui-tools-mac-arm64 artifact
#   <ssh-host>   the Mac; the tools run from a fresh /tmp/t3code-gpui-parity-<random> there
#   [ref-dir]    the Mac capture (PNG + element JSON) to match text against and draw
#                side-by-sides with; without it text is measured in place
# Env: PARITY_OUT (default ./parity-mac-<run>), PARITY_REPO (default AadiJo/t3code-gpui),
#      PARITY_SHOT=0 to skip the `shot` scenes (rendered for inspection, not judged),
#      PARITY_LOCAL_STORAGE to name the install's localStorage directory on the host
#      (default: the T3 Code desktop app's, under ~/Library/Application Support/t3code-v2).
# On the host it reads the install's client-settings.json and localStorage by path (localStorage
# from a copy in the run directory), writes only inside the run directory, and removes it at the
# end, also on failure. Nothing is installed or left running.
set -euo pipefail
run=${1:?usage: run-mac.sh <ci-run-id> <ssh-host> [ref-dir]}
host=${2:?usage: run-mac.sh <ci-run-id> <ssh-host> [ref-dir]}
ref=${3:-}
here=$(cd "$(dirname "$0")" && pwd)
repo=${PARITY_REPO:-AadiJo/t3code-gpui}
out=$(mkdir -p "${PARITY_OUT:-parity-mac-$run}" && cd "${PARITY_OUT:-parity-mac-$run}" && pwd)
dl=$(mktemp -d)
remote=/tmp/t3code-gpui-parity-$(openssl rand -hex 6)

cleanup() {
  rm -rf "$dl"
  # Only processes started from this run's directory, which nothing else can name.
  ssh "$host" "pkill -f '^$remote/' 2>/dev/null; rm -rf '$remote'; if [ -e '$remote' ]; then echo 'host: $remote still there'; else echo 'host: removed $remote'; fi; pgrep -f '$remote' >/dev/null && echo 'host: processes still running' || echo 'host: nothing running'" || echo "host cleanup failed: remove $remote on $host by hand"
}
trap cleanup EXIT

echo "== downloading tools from run $run"
gh run download "$run" --repo "$repo" -n t3code-gpui-tools-mac-arm64 -D "$dl"
gh run view "$run" --repo "$repo" --json headSha,headBranch -q '"commit \(.headSha) on \(.headBranch)"' | tee "$out/build.txt"

echo "== copying to $host:$remote"
ssh "$host" "mkdir '$remote'"
scp -q "$dl/t3code-gpui-tools-mac-arm64.tar.gz" "$host:$remote/"

echo "== rendering on $host"
ssh "$host" "bash -s -- $(printf '%q ' "$remote" "${PARITY_SHOT:-1}" "${PARITY_LOCAL_STORAGE:-}")" <<'REMOTE' 2>&1 | tee "$out/render.log"
set -u
remote=$1 shots=${2:-1} storage=${3:-}
cd "$remote"
tar xzf t3code-gpui-tools-mac-arm64.tar.gz && rm t3code-gpui-tools-mac-arm64.tar.gz
tools=$remote/t3code-gpui-tools-mac-arm64/bin
settings=$HOME/.t3/userdata/client-settings.json
layout=${storage:-$HOME/Library/Application Support/t3code-v2/Local Storage/leveldb}
mkdir -p home tmp data out/mac out/shot
# The tools see only the run directory; the install is named by path and read, never written.
export HOME=$remote/home TMPDIR=$remote/tmp T3CODE_DATA_DIR=$remote/data
export PARITY_CLIENT_SETTINGS=$settings PARITY_LAYOUT=$layout T3CODE_CLIENT_SETTINGS=$settings
for scene in $("$tools/parity-shot" --list-mac); do
  "$tools/parity-shot" --mac out/mac "$scene" 2>&1 | grep -v '^owner profile'
done
echo "mac scenes rendered: $(ls out/mac | grep -c '\.png$')"
if [ "$shots" = 1 ]; then
  for scene in $("$tools/shot" --list); do
    "$tools/shot" "$scene" "out/shot/$scene.png" 1710 1074 >/dev/null 2>&1 || echo "shot failed: $scene"
  done
fi
"$tools/parity-shot" --mac out/mac __none__ 2>&1 | head -1  # the layout values read
du -sh "$remote"
REMOTE

echo "== copying images back"
rm -rf "$out/port" "$out/shot"
scp -q -r "$host:$remote/out/mac" "$out/port"
scp -q -r "$host:$remote/out/shot" "$out/shot" 2>/dev/null || true
trap - EXIT
cleanup | tee "$out/cleanup.txt"

echo "== judging in the mac mode"
PARITY_MODE=mac python3 "$here/compare.py" compare "$out/port" "$out/report" ${ref:+"$ref"} || true
echo "report: $out/report/report.md"
