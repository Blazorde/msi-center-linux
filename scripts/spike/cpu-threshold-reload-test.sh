#!/usr/bin/env bash
# Spike (2026-10-25), follow-up of cpu-fan-follow-test.sh: the EC applies CPU *speeds* live but seems to ignore
# freshly written CPU *thresholds* (GPU thresholds do apply). Find which event makes the EC reload them.
# Every step only lowers CPU thresholds (more cooling than factory). Restores CPU + GPU factory tables, auto
# fan mode and the original shift mode on every exit path. Aborts if the CPU reads ≥ 85 °C twice in a row.
#
#   sudo scripts/spike/cpu-threshold-reload-test.sh
set -euo pipefail

D=/sys/devices/platform/msi-ec
CPU_F="0 36 40 50 62 80 96|57 62 70 77 83 90|53 57 65 73 80 87"
GPU_F="0 36 42 50 62 80 96|50 60 65 70 78 83|45 55 63 67 75 80"
LOW_UP="42 47 52 57 62 67"
LOW_DOWN="38 43 48 53 58 63"

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }
[[ -w $D/cpu/fan_curve_up_temps ]] || { echo "Module msi-ec du projet absent." >&2; exit 1; }
systemctl is-active -q msi-daemon && { echo "Arrête d'abord msi-daemon : sudo systemctl stop msi-daemon" >&2; exit 1; }

shift0=$(cat $D/shift_mode)

set_factory() { # $1 = fan dir, $2 = "speeds|up|down"
    local speeds up down
    IFS='|' read -r speeds up down <<< "$2"
    # Thresholds go back up to factory: up first, then down (kernel rule down[i] < up[i]).
    echo "$up" > "$D/$1/fan_curve_up_temps" || true
    echo "$down" > "$D/$1/fan_curve_down_temps" || true
    echo "$speeds" > "$D/$1/fan_curve_speeds" || true
}
restore() {
    set_factory cpu "$CPU_F"
    set_factory gpu "$GPU_F"
    echo "$shift0" > $D/shift_mode || true
    echo auto > $D/fan_mode
    echo "== Restauré : CPU up=$(cat $D/cpu/fan_curve_up_temps) | GPU speeds=$(cat $D/gpu/fan_curve_speeds) | shift=$(cat $D/shift_mode) | mode=$(cat $D/fan_mode)"
}
trap restore EXIT
restore

hot=0
watch() {
    for _ in $(seq "$2"); do
        local t; t=$(cat $D/cpu/realtime_temperature)
        if ((t >= 85)); then hot=$((hot + 1)); else hot=0; fi
        ((hot < 2)) || { echo "CPU ≥ 85 °C : arrêt." >&2; exit 1; }
        printf '%-34s cpu=%3s°C fan=%3s%%  (mode=%s shift=%s)\n' "$1" "$t" "$(cat $D/cpu/realtime_fan_speed)" \
            "$(cat $D/fan_mode)" "$(cat $D/shift_mode)"
        sleep 2
    done
}
lower_cpu_thresholds() { echo "$LOW_DOWN" > $D/cpu/fan_curve_down_temps; echo "$LOW_UP" > $D/cpu/fan_curve_up_temps; }

echo "Seuils bas visés : up=$LOW_UP (à ~58 °C le ventilo devrait passer à 50-62 %)"
watch "0 référence auto/usine" 4

lower_cpu_thresholds
watch "1 seuils bas écrits en auto" 6
echo advanced > $D/fan_mode
watch "1 puis → advanced" 8

echo auto > $D/fan_mode; sleep 1; echo advanced > $D/fan_mode
watch "2 bascule auto→advanced" 8

other=eco; [[ $shift0 == eco ]] && other=comfort
echo "$other" > $D/shift_mode; sleep 1; echo "$shift0" > $D/shift_mode
watch "3 bascule shift $other→$shift0" 8

restore
trap - EXIT
watch "4 auto/usine" 4
echo "✓ terminé"
