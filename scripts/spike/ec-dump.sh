#!/usr/bin/env bash
# Phase 2 spike — READ-ONLY EC snapshot via msi-ec (loaded with debug=1).
# Never writes to the EC. Output goes to scripts/spike/out/ (gitignored).
#
#   scripts/spike/ec-dump.sh [label] [samples] [interval_s]
#   e.g. scripts/spike/ec-dump.sh idle 30 2
set -euo pipefail

label=${1:-snapshot}
samples=${2:-1}
interval=${3:-2}

root=/sys/devices/platform/msi-ec
[[ -r $root/debug/ec_dump ]] || {
    echo "msi-ec absent ou chargé sans debug=1 : sudo insmod msi-ec.ko debug=1" >&2
    exit 1
}

read_attr() { cat "$root/$1" 2>/dev/null || echo "n/a"; }

# Relative to this script, not to the cwd (it may be run from anywhere, e.g. via fan-mode-cycle.sh).
out="$(cd "$(dirname "$0")" && pwd)/out/$(date +%Y%m%dT%H%M%S)-$label"
mkdir -p "$out"

{
    echo "label=$label"
    echo "kernel=$(uname -r)"
    echo "fw_version=$(read_attr fw_version)"
    echo "fw_release_date=$(read_attr fw_release_date)"
    echo "shift_mode=$(read_attr shift_mode)"
    echo "fan_mode=$(read_attr fan_mode)"
    echo "cooler_boost=$(read_attr cooler_boost)"
} > "$out/meta.txt"

printf 'i\ttime\tcpu_temp\tcpu_fan\tgpu_temp\tgpu_fan\thwmon_fan1\thwmon_fan2\n' > "$out/samples.tsv"
hwmon=$(grep -l msi_wmi_platform /sys/class/hwmon/hwmon*/name 2>/dev/null | head -1 | xargs -r dirname)

for ((i = 0; i < samples; i++)); do
    cp "$root/debug/ec_dump" "$out/dump-$(printf %03d "$i").txt"
    printf '%d\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$i" "$(date +%T)" \
        "$(read_attr cpu/realtime_temperature)" "$(read_attr cpu/realtime_fan_speed)" \
        "$(read_attr gpu/realtime_temperature)" "$(read_attr gpu/realtime_fan_speed)" \
        "$(cat "$hwmon/fan1_input" 2>/dev/null || echo n/a)" "$(cat "$hwmon/fan2_input" 2>/dev/null || echo n/a)" \
        >> "$out/samples.tsv"
    if ((i + 1 < samples)); then sleep "$interval"; fi
done

echo "$out"
