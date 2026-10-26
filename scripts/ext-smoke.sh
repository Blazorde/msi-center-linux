#!/usr/bin/env bash
# End-to-end smoke test of the GNOME Shell extension in a fully ISOLATED headless GNOME Shell:
#   - private session bus (dbus-run-session) and a private "system" bus (DBUS_SYSTEM_BUS_ADDRESS)
#   - mock org.blazorde.MsiCenter1 (extension/tests/mock-msi-daemon.js) — the real msi-daemon is never contacted
#   - real clipd (target/debug/clipd) with storage in a temp dir
#   - GSETTINGS_BACKEND=memory and temp XDG dirs: the owner's dconf / extensions are never touched
# Manual tool (≈30 s, needs gnome-shell + a built clipd): not part of scripts/ci.sh.
# Usage: scripts/ext-smoke.sh [--shots DIR]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
REPO=$PWD
UUID=msi-center@blazorde
SHOTS=""
[[ ${1:-} == --shots ]] && SHOTS=$(realpath -m "${2:?--shots DIR}") && mkdir -p "$SHOTS"

for bin in gnome-shell gnome-extensions dbus-run-session dbus-daemon gjs glib-compile-schemas unzip; do
    command -v "$bin" >/dev/null || { echo "$bin absent" >&2; exit 1; }
done
[[ -x target/debug/clipd ]] || cargo build -p clipd --locked

S=$(mktemp -d "${TMPDIR:-/tmp}/msi-ext-smoke.XXXXXX")
E=$S/data/gnome-shell/extensions/$UUID
mkdir -p "$E" "$S/config" "$S/cache"
# Unix socket paths are limited to 108 bytes: keep the runtime dir (Wayland + buses) short.
R=$(mktemp -d /tmp/msr.XXXXXX)
chmod 700 "$R"

gnome-extensions pack extension/$UUID --force --out-dir="$S" --extra-source=lib --extra-source=icons >/dev/null
unzip -q -o "$S/$UUID.shell-extension.zip" -d "$E"
glib-compile-schemas "$E/schemas"
# Inject the probe into the scratch copy only.
cp extension/tests/smoke-probe.js "$E/smoke-probe.js"
sed -i "s|^            () => this._popup.toggle());|&\n        if (!globalThis.__msiSmoke) { globalThis.__msiSmoke = true; import('./smoke-probe.js').then(m => m.run(this)); }|" "$E/extension.js"
grep -q smoke-probe "$E/extension.js" || { echo "probe injection failed" >&2; exit 1; }

cat >"$S/inner.sh" <<EOS
#!/usr/bin/env bash
set -u
export XDG_DATA_HOME=$S/data XDG_CONFIG_HOME=$S/config XDG_CACHE_HOME=$S/cache XDG_RUNTIME_DIR=$R
export GSETTINGS_BACKEND=memory MSI_SMOKE_DIR=$R MSI_SMOKE_SHOTS=$SHOTS
unset WAYLAND_DISPLAY DISPLAY JOURNAL_STREAM
# Private "system" bus: nothing below may reach the real one.
dbus-daemon --session --address=unix:path=$R/system_bus --nofork --nopidfile >$S/sysbus.log 2>&1 &
for _ in \$(seq 50); do [[ -S $R/system_bus ]] && break; sleep 0.1; done
[[ -S $R/system_bus ]] || { echo "private system bus did not start" >&2; exit 3; }
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=$R/system_bus
[[ \$DBUS_SYSTEM_BUS_ADDRESS == unix:path=$R/* ]] || { echo "unsafe system bus" >&2; exit 3; }
gjs -m $REPO/extension/tests/mock-msi-daemon.js >$S/mock.log 2>&1 &
$REPO/target/debug/clipd >$S/clipd.log 2>&1 &
timeout 45 gnome-shell --headless --wayland --no-x11 --virtual-monitor 1920x1080 \
    --wayland-display=msi-smoke-0 >$S/shell.log 2>&1 &
shell=\$!
sleep 6
gnome-extensions enable $UUID
for _ in \$(seq 60); do grep -q 'SMOKE DONE\|SMOKE FAIL exception' $S/shell.log && break; sleep 0.5; done
kill \$shell 2>/dev/null; wait \$shell 2>/dev/null
kill %1 %2 %3 2>/dev/null
exit 0
EOS
chmod +x "$S/inner.sh"
timeout 90 dbus-run-session -- "$S/inner.sh"

grep -E 'SMOKE' "$S/shell.log" | sed 's/^.*SMOKE /  /'
errors=$(grep -E 'JS ERROR|Gjs-CRITICAL|SMOKE FAIL' "$S/shell.log" || true)
if [[ -n $errors ]] || ! grep -q 'SMOKE DONE' "$S/shell.log"; then
    echo "$errors" >&2
    echo "✗ smoke test en échec — journal : $S/shell.log" >&2
    exit 1
fi
echo "✓ smoke test OK (journal : $S/shell.log${SHOTS:+, captures : $SHOTS})"
