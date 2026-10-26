#!/usr/bin/env bash
# User-level install of the msi-center GTK app (no sudo): binary, desktop entry, icons, D-Bus activation
# file and the login autostart entry ("l'app se lance au démarrage, à chaque fois").
#
#   scripts/app-install.sh              # builds in release if needed, then installs
#   scripts/app-install.sh --uninstall
#
# Installed under ~/.local (XDG_DATA_HOME / XDG_CONFIG_HOME respected). The autostart entry starts the app
# in the background (no window); turn it off in Réglages or with --uninstall.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
data="$repo/crates/msi-center/data"
bindir="$HOME/.local/bin"
datadir="${XDG_DATA_HOME:-$HOME/.local/share}"
confdir="${XDG_CONFIG_HOME:-$HOME/.config}"
app_id=org.blazorde.MsiCenter

files=(
    "$bindir/msi-center"
    "$datadir/applications/$app_id.desktop"
    "$datadir/icons/hicolor/scalable/apps/$app_id.svg"
    "$datadir/icons/hicolor/symbolic/apps/$app_id-symbolic.svg"
    "$datadir/dbus-1/services/$app_id.service"
    "$confdir/autostart/$app_id.desktop"
)

refresh() {
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$datadir/applications" || true
    command -v gtk4-update-icon-cache >/dev/null && gtk4-update-icon-cache -qtf "$datadir/icons/hicolor" 2>/dev/null || true
}

if [[ ${1:-} == --uninstall ]]; then
    pkill -x msi-center 2>/dev/null || true
    rm -fv "${files[@]}"
    refresh
    echo "✓ msi-center désinstallé"
    exit 0
fi

bin="$repo/target/release/msi-center"
if [[ ! -x $bin || -n $(find "$repo/crates/msi-center" -newer "$bin" -type f -print -quit) ]]; then
    (cd "$repo" && cargo build --release -p msi-center)
fi

install -Dm755 "$bin" "$bindir/msi-center"
# Absolute paths: ~/.local/bin is not always in the PATH of the session (autostart, D-Bus activation).
sed "s|^Exec=msi-center|Exec=$bindir/msi-center|" "$data/$app_id.desktop" \
    | install -Dm644 /dev/stdin "$datadir/applications/$app_id.desktop"
install -Dm644 "$data/icons/hicolor/scalable/apps/$app_id.svg" "$datadir/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 "$data/icons/hicolor/symbolic/apps/$app_id-symbolic.svg" \
    "$datadir/icons/hicolor/symbolic/apps/$app_id-symbolic.svg"
sed "s|@bindir@|$bindir|" "$data/$app_id.service" \
    | install -Dm644 /dev/stdin "$datadir/dbus-1/services/$app_id.service"
sed "s|^Exec=msi-center|Exec=$bindir/msi-center|" "$data/$app_id-autostart.desktop" \
    | install -Dm644 /dev/stdin "$confdir/autostart/$app_id.desktop"
refresh

echo "✓ msi-center installé dans $bindir"
echo "  Lancement automatique à l'ouverture de session (en arrière-plan) : $confdir/autostart/$app_id.desktop"
echo "  Ouvrir maintenant : $bindir/msi-center"
