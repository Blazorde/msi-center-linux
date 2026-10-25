#!/usr/bin/env bash
# Owner test of the forked msi-ec (phase 3), automating kernel/TESTING.md.
# Loads target/kernel/msi-ec/msi-ec.ko temporarily (gone at reboot), checks the curve files, checks that invalid
# curves are rejected, applies a quieter CPU step 1 (36 → 30 %) for 20 s, then restores factory values and auto.
#
#   scripts/kernel-build.sh && sudo kernel/test-fan-curve.sh
set -euo pipefail

ko="$(cd "$(dirname "$0")/.." && pwd)/target/kernel/msi-ec/msi-ec.ko"
D=/sys/devices/platform/msi-ec
FACTORY_CPU_SPEEDS="0 36 40 50 62 80 96"
QUIET_CPU_SPEEDS="0 30 40 50 62 80 96"

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }
[[ -f $ko ]] || { echo "Module absent : lance d'abord scripts/kernel-build.sh" >&2; exit 1; }

ok() { printf '  ✓ %s\n' "$1"; }
ko_() { printf '  ✗ %s\n' "$1" >&2; exit 1; }

echo "== Chargement"
rmmod msi_ec 2>/dev/null || true
insmod "$ko"
[[ $(cat /sys/module/msi_ec/version) == *blazorde* ]] && ok "module fork chargé ($(cat /sys/module/msi_ec/version))" || ko_ "mauvais module"
[[ $(cat $D/fw_version) == 14K2EMS1.110 ]] && ok "firmware 14K2EMS1.110 accepté sans forçage" || ko_ "firmware: $(cat $D/fw_version)"

restore() {
    echo "$FACTORY_CPU_SPEEDS" > $D/cpu/fan_curve_speeds 2>/dev/null || true
    echo auto > $D/fan_mode
    echo "== Restauré : cpu speeds = $(cat $D/cpu/fan_curve_speeds), fan_mode = $(cat $D/fan_mode)"
}
trap restore EXIT

echo "== Lecture des courbes"
for f in cpu gpu; do
    for t in speeds up_temps down_temps; do printf '  %s/%-16s %s\n' "$f" "$t" "$(cat $D/$f/fan_curve_$t)"; done
done
[[ $(cat $D/cpu/fan_curve_speeds) == "$FACTORY_CPU_SPEEDS" ]] || ko_ "courbe CPU inattendue, arrêt sans écrire"

echo "== Valeurs invalides (doivent toutes être refusées)"
for bad in "0 36 40 50 62 80" "0 36 30 50 62 80 96" "0 36 40 50 62 80 101" "0  36 40 50 62 80 96" "0 0 0 0 0 0 0"; do
    if echo "$bad" > $D/cpu/fan_curve_speeds 2>/dev/null; then ko_ "accepté à tort : '$bad'"; else ok "refusé : '$bad'"; fi
done
if echo "50 62 70 77 83 90" > $D/cpu/fan_curve_down_temps 2>/dev/null; then ko_ "règle descente < montée non appliquée"; else ok "refusé : descente ≥ montée"; fi
[[ $(cat $D/cpu/fan_curve_speeds) == "$FACTORY_CPU_SPEEDS" ]] && ok "rien n'a été écrit" || ko_ "table modifiée par une écriture refusée"

echo "== Écriture d'un palier plus silencieux (CPU palier 1 : 36 → 30 %), 20 s"
echo "$QUIET_CPU_SPEEDS" > $D/cpu/fan_curve_speeds
echo advanced > $D/fan_mode
[[ $(cat $D/cpu/fan_curve_speeds) == "$QUIET_CPU_SPEEDS" ]] && ok "écrit et relu" || ko_ "relecture différente"
for i in $(seq 10); do
    printf '  cpu=%3s°C  ventilo=%3s %%\n' "$(cat $D/cpu/realtime_temperature)" "$(cat $D/cpu/realtime_fan_speed)"
    sleep 2
done

restore
trap - EXIT
echo "✓ Test terminé. Le module reste chargé jusqu'au prochain redémarrage."
