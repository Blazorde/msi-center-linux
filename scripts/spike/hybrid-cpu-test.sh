#!/usr/bin/env bash
# Owner test (ADR-0010): 60 s supervised demo of the hybrid CPU fan control, done by hand exactly the way
# msi-daemon does it, through the forked msi-ec sysfs files only (every write is validated by the kernel).
#
#   - factory CPU thresholds kept (the EC ignores written ones), fan mode `advanced`;
#   - every second: CPU realtime temperature → median of the last 5 → moving average (α 0.3);
#   - target = exponential curve of the preset (Silencieux by default), idle below T0 − hysteresis,
#     restart from the start speed at T0, dead band 3 % / 20 s, slew +5 %/s and −1 %/s;
#   - vector written to cpu/fan_curve_speeds only when it changes, then read back:
#       steps 0–3 = target (≥ 30 % on 1–2 and ≥ 50 % on 3, S2), step 4 ≥ max(curve(77 °C), 60 %), 5–6 = 100 %;
#   - logs: time, realtime °C, filtered °C, target %, vector, EC realtime fan %, rpm.
#
# Safety: aborts if the CPU reads ≥ 85 °C twice in a row or if a read-back differs; factory CPU tables then
# fan mode `auto` are restored on EVERY exit path (end, abort, Ctrl+C, error). It stops nothing itself:
# msi-daemon must already be stopped (sudo systemctl stop msi-daemon), restart it afterwards.
#
#   sudo scripts/spike/hybrid-cpu-test.sh [silent|progressive|aggressive]
#
# Tip: run a load in another terminal half-way through (e.g. `stress-ng --cpu 4 -t 20`) to see the ramp.
set -euo pipefail

D=/sys/devices/platform/msi-ec
F_SPEEDS="0 36 40 50 62 80 96"
F_UP="57 62 70 77 83 90"
F_DOWN="53 57 65 73 80 87"
DURATION=60
ABORT_TEMP=85

# Curve presets (crates/msi-core/src/curve.rs): idle t0 t1 min max k hysteresis.
case "${1:-silent}" in
    silent) read -r IDLE T0 T1 MIN MAX K HYST <<< "0 50 82 30 100 2.0 4" ;;
    progressive) read -r IDLE T0 T1 MIN MAX K HYST <<< "0 42 79 30 100 1.8 4" ;;
    aggressive) read -r IDLE T0 T1 MIN MAX K HYST <<< "30 40 74 40 100 1.4 3" ;;
    *) echo "usage: $0 [silent|progressive|aggressive]" >&2; exit 2 ;;
esac

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }
[[ -w $D/cpu/fan_curve_speeds ]] || { echo "Module msi-ec du projet absent (cpu/fan_curve_speeds)." >&2; exit 1; }
[[ $(cat $D/fw_version) == 14K2EMS1.110 ]] || { echo "Firmware EC $(cat $D/fw_version) : tables d'usine inconnues." >&2; exit 1; }
if systemctl is-active -q msi-daemon; then
    echo "Arrête d'abord msi-daemon : sudo systemctl stop msi-daemon (ce script n'arrête rien lui-même)." >&2
    exit 1
fi

RPM=""
for h in /sys/class/hwmon/hwmon*; do
    [[ $(cat "$h/name" 2>/dev/null) == msi_wmi_platform && -r $h/fan1_input ]] && RPM=$h/fan1_input
done

restore() {
    local rc=$?
    # Raising thresholds back to factory: up first, then down (kernel rule down[i] < up[i]), then speeds.
    echo "$F_UP" > $D/cpu/fan_curve_up_temps || true
    echo "$F_DOWN" > $D/cpu/fan_curve_down_temps || true
    echo "$F_SPEEDS" > $D/cpu/fan_curve_speeds || true
    echo auto > $D/fan_mode || true
    echo "== Restauré : speeds=$(cat $D/cpu/fan_curve_speeds) up=$(cat $D/cpu/fan_curve_up_temps) mode=$(cat $D/fan_mode)"
    echo "== Relance le daemon : sudo systemctl start msi-daemon"
    exit "$rc"
}
trap restore EXIT
trap 'exit 130' INT TERM HUP

# Start from the factory tables (what msi-daemon's ExecStopPost leaves), then take the fans in advanced.
echo "$F_UP" > $D/cpu/fan_curve_up_temps
echo "$F_DOWN" > $D/cpu/fan_curve_down_temps
echo "$F_SPEEDS" > $D/cpu/fan_curve_speeds
echo advanced > $D/fan_mode

# f(T) of the exponential curve, rounded to the nearest % (half away from zero, like Rust).
curve() {
    awk -v t="$1" -v t0="$T0" -v t1="$T1" -v mn="$MIN" -v mx="$MAX" -v k="$K" 'BEGIN {
        if (t <= t0) s = mn; else if (t >= t1) s = mx; else s = mn + (mx - mn) * ((t - t0) / (t1 - t0)) ^ k;
        printf "%d", s + 0.5 }'
}
max() { (( $1 > $2 )) && echo "$1" || echo "$2"; }
ge() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a >= b) }'; }

STEP4=$(max "$(curve 77)" 60)
win=() ema="" out="" active=0 pending="" last="" hot=0
printf '%4s %6s %7s %7s  %-24s %6s %6s\n' "s" "temp" "filtré" "consigne" "vecteur écrit" "EC %" "rpm"
for ((s = 0; s < DURATION; s++)); do
    t=$(cat $D/cpu/realtime_temperature)
    if ((t >= ABORT_TEMP)); then hot=$((hot + 1)); else hot=0; fi
    ((hot < 2)) || { echo "CPU ≥ $ABORT_TEMP °C deux fois de suite : arrêt." >&2; exit 1; }

    # Filter: median of the last 5 readings (upper median), then moving average.
    win+=("$t")
    ((${#win[@]} <= 5)) || win=("${win[@]:1}")
    if ((${#win[@]} >= 3)); then
        med=$(printf '%s\n' "${win[@]}" | sort -n | sed -n "$((${#win[@]} / 2 + 1))p")
        ema=$(awk -v e="$ema" -v m="$med" 'BEGIN { if (e == "") printf "%.2f", m; else printf "%.2f", e + 0.3 * (m - e) }')
    fi

    # Pilot (same rules as msi_core::hybrid::CpuFanPilot).
    if [[ -z $ema ]]; then
        out=${out:-$MIN}
    else
        if ((active)); then
            ge "$ema" "$((T0 - HYST))" || active=0
        else
            ge "$ema" "$T0" && active=1
        fi
        if ((!active)); then
            out=$IDLE pending=""
        elif [[ -z $out ]] || ((out < MIN)); then
            out=$MIN pending=""
        else
            want=$(curve "$ema")
            diff=$((want > out ? want - out : out - want))
            if ((want == out)); then
                pending=""
            elif ((diff < 3)) && ! ((want > out && want == MAX)) && { pending=${pending:-$s}; ((s - pending < 20)); }; then
                :
            elif ((want > out)); then
                out=$((want < out + 5 ? want : out + 5))
            else
                out=$((want > out - 1 ? want : out - 1))
            fi
        fi
    fi

    s13=$(max "$out" 30) s3=$(max "$out" 50) s4=$(max "$out" "$STEP4")
    vec="$out $s13 $s13 $s3 $s4 100 100"
    if [[ $vec != "$last" ]]; then
        echo "$vec" > $D/cpu/fan_curve_speeds
        back=$(cat $D/cpu/fan_curve_speeds)
        [[ $back == "$vec" ]] || { echo "Relecture $back ≠ $vec : arrêt." >&2; exit 1; }
        last=$vec
    fi
    rpm=$([[ -n $RPM ]] && cat "$RPM" || echo "?")
    printf '%4d %4s°C %5s°C %6s %%  %-24s %5s%% %6s\n' "$s" "$t" "${ema:-–}" "$out" "$vec" \
        "$(cat $D/cpu/realtime_fan_speed)" "$rpm"
    sleep 1
done
echo "✓ terminé (${1:-silent})"
