#!/usr/bin/env bash
# Phase 2 spike — cycles the EC fan mode auto → silent → advanced → auto through msi-ec's own
# fan_mode attribute (the modes MSI exposes), recording EC dumps in each mode.
# The ONLY write is to fan_mode, and auto is always restored on exit (normal, error or Ctrl+C).
#
#   sudo scripts/spike/fan-mode-cycle.sh
set -euo pipefail

attr=/sys/devices/platform/msi-ec/fan_mode
dir=$(cd "$(dirname "$0")" && pwd)
user=${SUDO_USER:?run with sudo from your user account}

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }
[[ -w $attr ]] || { echo "msi-ec non chargé avec firmware=14K2EMS1.109" >&2; exit 1; }

set_mode() {
    case $1 in auto | silent | advanced) ;; *) echo "mode refusé: $1" >&2; exit 1 ;; esac
    echo "$1" > "$attr"
    [[ $(cat "$attr") == "$1" ]] || { echo "l'EC n'a pas pris le mode $1" >&2; exit 1; }
    echo "→ fan_mode=$1"
}

trap 'echo auto > "$attr"; echo "→ fan_mode restauré: $(cat "$attr")"' EXIT

dump() { sudo -u "$user" "$dir/ec-dump.sh" "$1" "$2" 2 >/dev/null; }

dump mode-auto 5
for mode in silent advanced; do
    set_mode "$mode"
    dump "mode-$mode" 15
done
set_mode auto
dump mode-auto-restored 5
echo "✓ terminé"
