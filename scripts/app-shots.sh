#!/usr/bin/env bash
# Screenshots of every page of the msi-center app, light and dark, in demo mode (MSI_CENTER_DEMO=1), inside a
# fully ISOLATED headless compositor:
#   - private session bus (dbus-run-session) and a private, empty "system" bus (DBUS_SYSTEM_BUS_ADDRESS)
#   - demo mode: the app serves fake msi-daemon / clipd in-process over socket pairs, no bus is contacted
#   - GSETTINGS_BACKEND=memory and temp XDG dirs: the owner's settings are never read or written
# Manual tool (≈1 min, needs mutter): not part of scripts/ci.sh.
# Usage: scripts/app-shots.sh [OUT_DIR]   (default: docs/design/screenshots)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
OUT=$(realpath -m "${1:-docs/design/screenshots}")
mkdir -p "$OUT"
for bin in mutter dbus-run-session dbus-daemon; do
    command -v "$bin" >/dev/null || { echo "$bin absent" >&2; exit 1; }
done
cargo build -p msi-center --locked
BIN=$PWD/target/debug/msi-center

S=$(mktemp -d "${TMPDIR:-/tmp}/msi-app-shots.XXXXXX")
R=$(mktemp -d /tmp/mas.XXXXXX) # short: Unix socket paths are limited to 108 bytes
chmod 700 "$R"
mkdir -p "$S/data" "$S/cache"

cat >"$S/inner.sh" <<EOS
#!/usr/bin/env bash
set -u
export XDG_DATA_HOME=$S/data XDG_CACHE_HOME=$S/cache XDG_RUNTIME_DIR=$R GSETTINGS_BACKEND=memory
unset WAYLAND_DISPLAY DISPLAY JOURNAL_STREAM
dbus-daemon --session --address=unix:path=$R/system_bus --nofork --nopidfile >$S/sysbus.log 2>&1 &
for _ in \$(seq 50); do [[ -S $R/system_bus ]] && break; sleep 0.1; done
[[ -S $R/system_bus ]] || { echo "private system bus did not start" >&2; exit 3; }
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=$R/system_bus
mutter --headless --wayland --no-x11 --virtual-monitor 1400x940 --wayland-display=msi-shots-0 \
    >$S/mutter.log 2>&1 &
comp=\$!
for _ in \$(seq 50); do [[ -S $R/msi-shots-0 ]] && break; sleep 0.1; done
export WAYLAND_DISPLAY=msi-shots-0 GDK_BACKEND=wayland MSI_CENTER_DEMO=1
for theme in ${SHOT_THEMES:-light dark}; do
    mkdir -p $S/config-\$theme/msi-center
    printf 'theme=%s\nstart-minimised=false\n' \$theme >$S/config-\$theme/msi-center/settings.ini
    for page in ${SHOT_PAGES:-home fans perf battery clipboard prefs}; do
        XDG_CONFIG_HOME=$S/config-\$theme MSI_CENTER_PAGE=\$page MSI_CENTER_SCREENSHOT=$OUT/\$page-\$theme.png \
            GDK_DEBUG=no-portals timeout 30 $BIN >>$S/app.log 2>&1 || echo "\$page-\$theme: exit \$?" >&2
    done
done
kill \$comp 2>/dev/null; wait \$comp 2>/dev/null
kill %1 2>/dev/null
exit 0
EOS
chmod +x "$S/inner.sh"
timeout 600 dbus-run-session -- "$S/inner.sh"
# The grain texture makes PNGs ~1.6 MB each: commit JPEGs (~10× smaller) instead.
for png in "$OUT"/*.png; do
    magick "$png" -strip -quality 85 "${png%.png}.jpg" && rm -f "$png"
done
ls -1 "$OUT"/*.jpg
echo "journal : $S/app.log"
