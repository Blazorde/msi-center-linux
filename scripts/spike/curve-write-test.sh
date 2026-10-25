#!/usr/bin/env bash
# Phase 3 spike — FIRST EC curve write, approved by the owner (2026-10-25).
# Proves that in fan mode `advanced` the EC applies the curve table, by changing the speed of the CPU step in
# use for 20 s, then restoring everything:
#  - step 0 (factory 0 % = fan stopped) → 25 %;
#  - step 1 or 2 (36 % / 40 %)          → 30 % (quieter; S2 floor ≥30 % at ≥60 °C still holds).
# Higher steps are never touched, so if the CPU heats up the EC moves to its factory steps as usual.
#
# Safety:
#  - only one byte is ever written: the speed of the active CPU step (0x72, 0x73 or 0x74);
#  - aborts and restores if the CPU stays ≥ 80 °C on two consecutive reads (single spikes are normal);
#  - the original byte and fan_mode=auto are restored on every exit path (trap).
#
#   sudo scripts/spike/curve-write-test.sh
set -euo pipefail

ec=/sys/devices/platform/msi-ec
ABORT_TEMP=80

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }
[[ -w $ec/debug/ec_set && -w $ec/fan_mode ]] || {
    echo "msi-ec doit être chargé avec debug=1 firmware=14K2EMS1.109" >&2
    exit 1
}

get() { printf '%02x' "$1" > "$ec/debug/ec_get"; echo $((16#$(cat "$ec/debug/ec_get"))); }
put() { printf '%02x=%02x' "$1" "$2" > "$ec/debug/ec_set"; }

cpu_temp() { get 0x68; }
cpu_fan() { get 0x71; }
rpm() { cat "$(grep -l msi_wmi_platform /sys/class/hwmon/hwmon*/name | head -1 | xargs dirname)/fan1_input"; }

# Active CPU step = the table entry (0x72..0x74) matching the realtime speed.
current=$(cpu_fan)
ADDR=""
for a in 0x72 0x73 0x74; do
    [[ $(get "$a") -eq $current ]] && ADDR=$a && break
done
case $ADDR in
    0x72) TEST_SPEED=25 ;;
    0x73 | 0x74) TEST_SPEED=30 ;;
    *)
        echo "Refusé : ventilo CPU à ${current} % ($(cpu_temp) °C), pas sur les paliers 0–2. Laisse le PC au repos et réessaie." >&2
        exit 1
        ;;
esac
original=$(get "$ADDR")
echo "Palier actif : EC[$ADDR]=$original % → test à $TEST_SPEED %"

restore() {
    put "$ADDR" "$original"
    echo auto > "$ec/fan_mode"
    printf '→ restauré : EC[%s]=%d %%, fan_mode=%s\n' "$ADDR" "$(get "$ADDR")" "$(cat "$ec/fan_mode")"
}
trap restore EXIT

log() { printf '%-26s cpu=%3d°C  ventilo=%3d %%  %5s rpm\n' "$1" "$(cpu_temp)" "$(cpu_fan)" "$(rpm)"; }

for i in 1 2 3; do log "auto (avant)"; sleep 2; done

echo advanced > "$ec/fan_mode"
for i in 1 2 3; do log "advanced, table usine"; sleep 2; done

put "$ADDR" "$TEST_SPEED"
[[ $(get "$ADDR") -eq $TEST_SPEED ]] || { echo "L'EC n'a pas gardé la valeur écrite." >&2; exit 1; }
echo "→ écrit EC[$ADDR]=$TEST_SPEED %"
hot=0
for i in $(seq 10); do
    if [[ $(cpu_temp) -ge $ABORT_TEMP ]]; then hot=$((hot + 1)); else hot=0; fi
    ((hot < 2)) || { echo "CPU ≥ ${ABORT_TEMP} °C sur 2 mesures : arrêt." >&2; exit 1; }
    log "advanced, palier à ${TEST_SPEED} %"
    sleep 2
done

restore
trap - EXIT
for i in 1 2 3; do log "auto (après)"; sleep 2; done
echo "✓ terminé"
