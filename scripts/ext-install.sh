#!/usr/bin/env bash
# Install the GNOME Shell extension for the current user (no sudo):
# pack → install into ~/.local/share/gnome-shell/extensions → compile schemas.
# Usage: scripts/ext-install.sh [--enable]
#   --enable  also run `gnome-extensions enable` (once; GNOME then loads it at every login).
# On Wayland a newly installed extension is only seen by GNOME Shell after logging out and back in.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
EXT=extension/msi-center@blazorde
UUID=msi-center@blazorde
OUT=dist
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$UUID"

enable=0
for arg in "$@"; do
    case $arg in
        --enable) enable=1 ;;
        -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
        *) echo "option inconnue : $arg" >&2; exit 2 ;;
    esac
done

[[ $EUID -ne 0 ]] || { echo "À lancer en utilisateur, pas en root." >&2; exit 1; }
command -v gnome-extensions >/dev/null || { echo "gnome-extensions absent (gnome-shell)." >&2; exit 1; }

mkdir -p "$OUT"
gnome-extensions pack "$EXT" --force --out-dir="$OUT" --extra-source=lib --extra-source=icons
gnome-extensions install --force "$OUT/$UUID.shell-extension.zip"
glib-compile-schemas "$DEST/schemas"
echo "✓ Installée dans $DEST"

if (( enable )); then
    gnome-extensions enable "$UUID" && echo "✓ Activée (chargée à chaque connexion)"
else
    echo "Activer une fois (puis chargée à chaque connexion) : gnome-extensions enable $UUID"
fi
echo "Wayland : déconnectez-vous / reconnectez-vous si GNOME ne voit pas encore l'extension."
