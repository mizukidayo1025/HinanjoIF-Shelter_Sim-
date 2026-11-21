#!/usr/bin/env bash
# Put a judge slot back to an empty store with, when setup.sh built it, the
# precomputed example Projects (sample, sample-…): sudo /opt/sheltersim/reset-slot.sh <n>
set -euo pipefail
n="${1:?usage: reset-slot.sh <slot> [--no-restart]}"
[[ $n =~ ^[0-9]+$ ]] || { echo "bad slot: $n" >&2; exit 1; }
slot=/var/lib/sheltersim/slot$n
samples=/var/lib/sheltersim/sample/projects
[[ ${2:-} == --no-restart ]] || systemctl stop "sheltersim@$n" || true
rm -rf "$slot"
# The store that the slot's service opens (sheltersim@.service, --runs-dir slot/runs).
mkdir -p "$slot/runs/.projects"
for sample in "$samples"/sample "$samples"/sample-*; do
  if [[ -d $sample ]]; then
    cp -a "$sample" "$slot/runs/.projects/${sample##*/}"
  fi
done
chown -R sheltersim:sheltersim "$slot"
[[ ${2:-} == --no-restart ]] || systemctl start "sheltersim@$n"
echo "slot $n reset"
