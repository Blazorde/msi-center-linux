#!/usr/bin/env bash
# Fast, offline checks for the GNOME Shell extension (called by scripts/ci.sh, runs in ~1 s):
#   1. JS syntax (node --check, ESM) + relative imports resolve
#   2. unit tests of pure helpers (node --test)
#   3. metadata.json validity
#   4. GSettings schema compiles (--strict)
#   5. bundle packs (gnome-extensions pack, if installed)
#   6. ESLint, only if already installed locally (never downloads)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
EXT=extension/msi-center@blazorde
UUID=msi-center@blazorde
SCHEMA_ID=org.gnome.shell.extensions.msi-center

step() { printf '  \033[1;34m·\033[0m %s\n' "$1"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$1" >&2; exit 1; }

command -v node >/dev/null || die "node absent (pacman -S nodejs)"
command -v glib-compile-schemas >/dev/null || die "glib-compile-schemas absent (glib2)"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

step "syntaxe JS (node --check)"
mapfile -t js < <(find "$EXT" extension/tests -name '*.js' -o -name '*.mjs' | sort)
for f in "${js[@]}"; do
    node --check "$f" || die "erreur de syntaxe : $f"
done

step "imports relatifs"
for f in "${js[@]}"; do
    dir=$(dirname "$f")
    while IFS= read -r rel; do
        [[ -f "$dir/$rel" ]] || die "$f importe $rel introuvable"
    done < <(grep -oE "from '\.{1,2}/[^']+'" "$f" | sed -E "s/from '(.*)'/\1/")
done

step "tests unitaires (node --test)"
node --test extension/tests/ >"$tmp/test.log" 2>&1 || { cat "$tmp/test.log"; die "tests JS en échec"; }

step "metadata.json"
node -e '
const fs = require("fs");
const [file, uuid, schema] = process.argv.slice(1);
const m = JSON.parse(fs.readFileSync(file, "utf8"));
const fail = msg => { console.error(msg); process.exit(1); };
if (m.uuid !== uuid) fail(`uuid ${m.uuid} != ${uuid}`);
for (const k of ["name", "description", "shell-version", "settings-schema"])
    if (!m[k]) fail(`clé manquante : ${k}`);
if (!Array.isArray(m["shell-version"]) || !m["shell-version"].includes("51")) fail("shell-version doit contenir \"51\"");
if (m["settings-schema"] !== schema) fail(`settings-schema ${m["settings-schema"]} != ${schema}`);
' "$EXT/metadata.json" "$UUID" "$SCHEMA_ID" || die "metadata.json invalide"
[[ -f "$EXT/schemas/$SCHEMA_ID.gschema.xml" ]] || die "schéma $SCHEMA_ID absent"

step "schéma GSettings (glib-compile-schemas --strict)"
glib-compile-schemas --strict --targetdir="$tmp" "$EXT/schemas"

if command -v gnome-extensions >/dev/null; then
    step "paquet (gnome-extensions pack)"
    gnome-extensions pack "$EXT" --force --out-dir="$tmp" \
        --extra-source=lib --extra-source=icons >/dev/null
    [[ -f "$tmp/$UUID.shell-extension.zip" ]] || die "pack en échec"
else
    step "paquet : gnome-extensions absent, ignoré"
fi

if (cd extension && npx --no-install eslint --version >/dev/null 2>&1); then
    step "ESLint"
    (cd extension && npx --no-install eslint .)
else
    step "ESLint non installé localement, ignoré (npm i -D eslint dans extension/)"
fi
