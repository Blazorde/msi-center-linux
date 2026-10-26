#!/usr/bin/env bash
# Development install of msi-daemon + msictl (+ clipd), until the phase 9 package exists.
# Build first as your user, then install as root:
#
#   cargo build --release -p msi-daemon -p clipd
#   sudo scripts/dev-install.sh             # binaries, units, polkit; (re)starts msi-daemon once
#   sudo scripts/dev-install.sh --enable    # + start at every boot/session (see below)
#   sudo scripts/dev-install.sh --uninstall # undoes everything recorded in the manifest
#
# --enable (autostart, idempotent, re-run it after a rebuild):
#   1. DKMS: kernel/msi-ec as package msi-ec-blazorde (rebuilt automatically for new kernels, AUTOINSTALL=yes);
#      re-added when the sources changed. Checks that the module modprobe loads is ours (updates/dkms/).
#   2. /etc/modules-load.d/msi-ec.conf: load msi_ec at boot (the module has no modalias).
#   3. systemctl enable msi-daemon (multi-user.target) + system D-Bus activation file (opt-in: a plain install
#      never lets a D-Bus call start the daemon behind the owner's back).
#   4. clipd (if built): binary, user unit, session D-Bus activation, systemctl --global enable (every session).
#
# Not blacklisted on purpose: the in-tree driver and ours share the name msi_ec, so a blacklist would block both.
# depmod prefers updates/ over the in-tree kernel/ directory; the script verifies `modinfo -n msi_ec`.
#
# Manifest (/var/lib/msi-center/dev-install.manifest): one action per line, `file PATH`, `dkms PKG/VER`,
# `enable UNIT`, `global UNIT`. --uninstall undoes exactly those (older manifests with bare paths still work).
# Stopping msi-daemon hands the fans back to the EC: factory tables then fan mode auto (ExecStopPost, S5).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
manifest=/var/lib/msi-center/dev-install.manifest
dkms_pkg=msi-ec-blazorde
dkms_ver="$(sed -n 's/^PACKAGE_VERSION="\(.*\)"$/\1/p' "$repo/kernel/msi-ec/dkms.conf")"
modules_load=/etc/modules-load.d/msi-ec.conf
activation=/usr/share/dbus-1/system-services/org.blazorde.MsiCenter1.service
user="${SUDO_USER:-}"

die() { echo "✗ $*" >&2; exit 1; }
[[ $EUID -eq 0 ]] || die "À lancer avec sudo."
[[ -n $dkms_ver ]] || die "PACKAGE_VERSION introuvable dans kernel/msi-ec/dkms.conf"

record() { mkdir -p "$(dirname "$manifest")"; touch "$manifest"; grep -qxF "$1" "$manifest" || echo "$1" >> "$manifest"; }
recorded() { [[ -f $manifest ]] && grep -qxF "$1" "$manifest"; }
put() { install -Dm"$1" "$2" "$3"; record "file $3"; }
# systemctl --user for the invoking user (best effort: no session, no harm).
user_ctl() {
    if [[ -n $user && $user != root ]]; then
        systemctl --user -M "$user@" "$@" 2>/dev/null || true
    fi
}

uninstall() {
    [[ -f $manifest ]] || { echo "Rien à désinstaller (pas de manifeste)."; exit 0; }
    # Stop first, while the binary still exists: ExecStopPost restores the factory tables and fan mode auto.
    systemctl disable --now msi-daemon.service 2>/dev/null || true
    while read -r kind arg; do
        case $kind in
            global) systemctl --global disable "$arg" || true; user_ctl stop "$arg" ;;
            enable) systemctl disable "$arg" 2>/dev/null || true ;;
        esac
    done < "$manifest"
    while read -r kind arg; do
        case $kind in
            file) rm -fv "$arg" ;;
            /*) rm -fv "$kind" ;; # manifest of an older version: bare paths
        esac
    done < "$manifest"
    while read -r kind arg; do
        [[ $kind == dkms ]] || continue
        modprobe -r msi_ec 2>/dev/null || echo "⚠ msi_ec encore utilisé : il sera déchargé au prochain démarrage"
        dkms remove "$arg" --all || true
        rm -rf "/usr/src/${arg%%/*}-${arg#*/}"
    done < "$manifest"
    rm -f "$manifest"
    systemctl daemon-reload
    user_ctl daemon-reload
    systemctl reload dbus 2>/dev/null || true
    echo "✓ Désinstallé (fan_mode : $(cat /sys/devices/platform/msi-ec/fan_mode 2>/dev/null || echo 'module absent'))"
}

dkms_install() {
    command -v dkms >/dev/null || die "dkms absent : sudo pacman -S dkms"
    [[ -d /usr/lib/modules/$(uname -r)/build ]] || die "en-têtes du noyau absents : sudo pacman -S linux-cachyos-headers"
    if dkms status | grep -qiE '^msi-ec[/,]'; then
        die "paquet DKMS upstream « msi-ec » installé : sudo dkms remove msi-ec/<version> --all (voir kernel/README.md)"
    fi
    # Older versions of our package.
    local other
    for other in $(dkms status -m "$dkms_pkg" | sed -n 's/^[^/]*\/\([^,:]*\).*/\1/p' | sort -u); do
        [[ $other == "$dkms_ver" ]] && continue
        echo "→ DKMS : retrait de $dkms_pkg/$other"
        dkms remove "$dkms_pkg/$other" --all || true
        rm -rf "/usr/src/$dkms_pkg-$other"
    done
    local src=/usr/src/$dkms_pkg-$dkms_ver
    if [[ -d $src ]] && ! diff -rq "$repo/kernel/msi-ec" "$src" >/dev/null; then
        echo "→ DKMS : sources modifiées, reconstruction"
        dkms remove "$dkms_pkg/$dkms_ver" --all || true
        rm -rf "$src"
    fi
    if ! dkms status -m "$dkms_pkg" -v "$dkms_ver" | grep -q .; then
        dkms add "$repo/kernel/msi-ec"
    fi
    record "dkms $dkms_pkg/$dkms_ver"
    if ! dkms status -m "$dkms_pkg" -v "$dkms_ver" -k "$(uname -r)" | grep -q installed; then
        dkms install "$dkms_pkg/$dkms_ver"
    fi
    depmod -a
    local path version
    path="$(modinfo -n msi_ec)"
    version="$(modinfo -F version msi_ec)"
    [[ $path == */updates/* && $version == "$dkms_ver" ]] ||
        die "modprobe chargerait $path (version $version), pas le module DKMS $dkms_ver"
    echo "✓ DKMS : $path ($version)"

    if [[ -f $modules_load ]] && ! recorded "file $modules_load"; then
        grep -qx msi_ec "$modules_load" || die "$modules_load existe déjà sans « msi_ec » : à vérifier à la main"
    else
        printf '# Written by msi-center-linux scripts/dev-install.sh --enable\nmsi_ec\n' > "$modules_load"
        record "file $modules_load"
    fi

    if [[ $(cat /sys/module/msi_ec/version 2>/dev/null) != "$dkms_ver" ]]; then
        echo "→ Rechargement de msi_ec (DKMS $dkms_ver)"
        systemctl stop msi-daemon.service 2>/dev/null || true
        modprobe -r msi_ec 2>/dev/null || true
        modprobe msi_ec
    fi
}

mode=install
case ${1:-} in
    "") ;;
    --enable) mode=enable ;;
    --uninstall) uninstall; exit 0 ;;
    *) die "usage : sudo scripts/dev-install.sh [--enable | --uninstall]" ;;
esac

for bin in msi-daemon msictl; do
    [[ -x $repo/target/release/$bin ]] ||
        die "target/release/$bin absent : lance d'abord (sans sudo) cargo build --release -p msi-daemon -p clipd"
done

if [[ $mode == enable ]]; then
    dkms_install
fi
[[ $(cat /sys/module/msi_ec/version 2>/dev/null) == *blazorde* ]] ||
    die "Le module msi-ec du projet n'est pas chargé : sudo scripts/dev-install.sh --enable (DKMS) ou sudo kernel/test-fan-curve.sh"

put 755 "$repo/target/release/msi-daemon" /usr/bin/msi-daemon
put 755 "$repo/target/release/msictl" /usr/bin/msictl
put 644 "$repo/systemd/msi-daemon.service" /usr/lib/systemd/system/msi-daemon.service
put 644 "$repo/systemd/org.blazorde.MsiCenter1.conf" /usr/share/dbus-1/system.d/org.blazorde.MsiCenter1.conf
put 644 "$repo/systemd/org.blazorde.msicenter.policy" /usr/share/polkit-1/actions/org.blazorde.msicenter.policy
# D-Bus activation of the system daemon is opt-in (--enable): otherwise any client call would start it.
if [[ $mode == enable ]] || recorded "enable msi-daemon.service"; then
    put 644 "$repo/systemd/org.blazorde.MsiCenter1.system.service" "$activation"
elif [[ -f $activation ]] && recorded "file $activation"; then
    rm -f "$activation"
fi

clipd=no
if [[ -x $repo/target/release/clipd ]]; then
    put 755 "$repo/target/release/clipd" /usr/bin/clipd
    put 644 "$repo/systemd/clipd.service" /usr/lib/systemd/user/clipd.service
    put 644 "$repo/systemd/org.blazorde.Clipd1.service" /usr/share/dbus-1/services/org.blazorde.Clipd1.service
    clipd=yes
elif [[ $mode == enable ]]; then
    echo "⚠ target/release/clipd absent : clipd non installé (cargo build --release -p clipd puis relancer)"
fi

systemctl daemon-reload
systemctl reload dbus 2>/dev/null || true

if [[ $mode == enable ]]; then
    systemctl enable msi-daemon.service
    record "enable msi-daemon.service"
    if [[ $clipd == yes ]]; then
        systemctl --global enable clipd.service
        record "global clipd.service"
    fi
fi
if [[ $clipd == yes ]]; then
    user_ctl daemon-reload
    if recorded "global clipd.service"; then
        user_ctl restart clipd.service
    fi
fi

systemctl restart msi-daemon.service
sleep 2
systemctl --no-pager --lines=15 status msi-daemon.service || true
echo
echo "✓ Installé. Essaie : msictl status · msictl profile eco · msictl factory · msictl telemetry --watch"
if recorded "enable msi-daemon.service"; then
    echo "  Démarrage automatique : msi_ec (modules-load.d) + msi-daemon (enabled)$([[ $clipd == yes ]] && echo ' + clipd (session)')."
    echo "  Vérifier : systemctl is-enabled msi-daemon · systemctl --user is-enabled clipd · modinfo -n msi_ec"
else
    echo "  Pas de démarrage automatique : sudo scripts/dev-install.sh --enable"
fi
echo "  Logs : journalctl -u msi-daemon -f · Désinstaller : sudo scripts/dev-install.sh --uninstall"
