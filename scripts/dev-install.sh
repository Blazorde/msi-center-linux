#!/usr/bin/env bash
# Development install of msi-daemon + msictl (until the phase 9 package exists).
# Build first as your user, then install as root:
#
#   cargo build --release -p msi-daemon && sudo scripts/dev-install.sh
#   sudo scripts/dev-install.sh --uninstall
#
# Every installed path is recorded in the manifest so --uninstall removes exactly those files.
# Stopping the service restores the EC fan mode to `auto` (ExecStopPost, safety rule S5).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
manifest=/var/lib/msi-center/dev-install.manifest

[[ $EUID -eq 0 ]] || { echo "À lancer avec sudo." >&2; exit 1; }

if [[ ${1:-} == --uninstall ]]; then
    systemctl disable --now msi-daemon.service 2>/dev/null || true
    [[ -f $manifest ]] || { echo "Rien à désinstaller (pas de manifeste)." >&2; exit 0; }
    xargs -r rm -fv < "$manifest"
    rm -f "$manifest"
    systemctl daemon-reload
    echo "✓ Désinstallé (fan_mode : $(cat /sys/devices/platform/msi-ec/fan_mode 2>/dev/null || echo 'module absent'))"
    exit 0
fi

for bin in msi-daemon msictl; do
    [[ -x $repo/target/release/$bin ]] || {
        echo "target/release/$bin absent : lance d'abord (sans sudo) cargo build --release -p msi-daemon" >&2
        exit 1
    }
done
[[ $(cat /sys/module/msi_ec/version 2>/dev/null) == *blazorde* ]] || {
    echo "Le module msi-ec du projet n'est pas chargé : sudo kernel/test-fan-curve.sh (ou insmod target/kernel/msi-ec/msi-ec.ko)" >&2
    exit 1
}

mkdir -p "$(dirname "$manifest")"
: > "$manifest"
put() { install -Dm"$1" "$2" "$3"; echo "$3" >> "$manifest"; }

put 755 "$repo/target/release/msi-daemon" /usr/bin/msi-daemon
put 755 "$repo/target/release/msictl" /usr/bin/msictl
put 644 "$repo/systemd/msi-daemon.service" /usr/lib/systemd/system/msi-daemon.service
put 644 "$repo/systemd/org.blazorde.MsiCenter1.conf" /usr/share/dbus-1/system.d/org.blazorde.MsiCenter1.conf
put 644 "$repo/systemd/org.blazorde.MsiCenter1.system.service" \
    /usr/share/dbus-1/system-services/org.blazorde.MsiCenter1.service
put 644 "$repo/systemd/org.blazorde.msicenter.policy" /usr/share/polkit-1/actions/org.blazorde.msicenter.policy

systemctl daemon-reload
systemctl reload dbus 2>/dev/null || true
systemctl restart msi-daemon.service
sleep 2
systemctl --no-pager --lines=15 status msi-daemon.service || true
echo
echo "✓ Installé. Essaie : msictl status · msictl profile eco · msictl telemetry --watch"
echo "  Logs : journalctl -u msi-daemon -f · Désinstaller : sudo scripts/dev-install.sh --uninstall"
