#!/usr/bin/env bash
# Put a judge slot back to an empty store with, when setup.sh built it, the
# precomputed sample Project: sudo /opt/sheltersim/reset-slot.sh <n>
set -euo pipefail
n="${1:?usage: reset-slot.sh <slot> [--no-restart]}"
[[ $n =~ ^[0-9]+$ ]] || { echo "bad slot: $n" >&2; exit 1; }
slot=/var/lib/sheltersim/slot$n
sample=/var/lib/sheltersim/sample/projects/sample
[[ ${2:-} == --no-restart ]] || systemctl stop "sheltersim@$n" || true
rm -rf "$slot"
# The store that the slot's service opens (sheltersim@.service, --runs-dir slot/runs).
mkdir -p "$slot/runs/.projects"
if [[ -d $sample ]]; then
  cp -a "$sample" "$slot/runs/.projects/sample"
fi
chown -R sheltersim:sheltersim "$slot"
[[ ${2:-} == --no-restart ]] || systemctl start "sheltersim@$n"
echo "slot $n reset"
