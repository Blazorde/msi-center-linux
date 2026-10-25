#!/usr/bin/env bash
# Spike (2026-10-25): why does the CPU fan stay at 0 % in `advanced` after msi-daemon writes full tables,
# while the GPU fan follows its table? Uses only the forked msi-ec sysfs files (validated by the kernel).
#
# Phases (CPU and GPU realtime values logged every 2 s):
#   A. auto, factory tables                     (baseline)
#   B. advanced, factory tables untouched       (does advanced alone change the CPU fan?)
#   C. advanced, CPU thresholds LOWERED, factory speeds  → only ever more cooling than factory
#   D. factory tables restored, auto
# Aborts (and restores) if the CPU reads ≥ 85 °C twice in a row. Restore runs on every exit path.
#
#   sudo scripts/spike/cpu-fan-follow-test.sh
set -euo pipefail

D=/sys/devices/platform/msi-ec
F_SPEEDS="0 36 40 50 62 80 96"
F_UP="57 62 70 77 83 90"
F_DOWN="53 57 65 73 80 87"
LOW_UP="42 47 52 57 62 67"
LOW_DOWN="38 43 48 53 58 63"

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }
[[ -w $D/cpu/fan_curve_up_temps ]] || { echo "Module msi-ec du projet absent." >&2; exit 1; }
systemctl is-active -q msi-daemon && { echo "Arrête d'abord msi-daemon : sudo systemctl stop msi-daemon" >&2; exit 1; }

restore() {
    # Raising thresholds back to factory: up first, then down (kernel cross rule down[i] < up[i]).
    echo "$F_UP" > $D/cpu/fan_curve_up_temps || true
    echo "$F_DOWN" > $D/cpu/fan_curve_down_temps || true
    echo "$F_SPEEDS" > $D/cpu/fan_curve_speeds || true
    echo auto > $D/fan_mode
    echo "== Restauré : up=$(cat $D/cpu/fan_curve_up_temps) down=$(cat $D/cpu/fan_curve_down_temps) mode=$(cat $D/fan_mode)"
}
trap restore EXIT

# msi-daemon left its own tables in the EC when it stopped (only fan_mode was reset): start from factory.
restore

hot=0
watch() {
    for _ in $(seq "$2"); do
        local t; t=$(cat $D/cpu/realtime_temperature)
        if ((t >= 85)); then hot=$((hot + 1)); else hot=0; fi
        ((hot < 2)) || { echo "CPU ≥ 85 °C : arrêt." >&2; exit 1; }
        printf '%-28s cpu=%3s°C fan=%3s%%  | gpu=%3s°C fan=%3s%%\n' "$1" "$t" "$(cat $D/cpu/realtime_fan_speed)" \
            "$(cat $D/gpu/realtime_temperature)" "$(cat $D/gpu/realtime_fan_speed)"
        sleep 2
    done
}

watch "A auto/usine" 5
echo advanced > $D/fan_mode
watch "B advanced/usine" 10
# Lowering thresholds: down first, then up.
echo "$LOW_DOWN" > $D/cpu/fan_curve_down_temps
echo "$LOW_UP" > $D/cpu/fan_curve_up_temps
echo "   → CPU up=$(cat $D/cpu/fan_curve_up_temps) down=$(cat $D/cpu/fan_curve_down_temps)"
watch "C advanced/seuils bas" 15
restore
trap - EXIT
watch "D auto/usine" 5
echo "✓ terminé"
