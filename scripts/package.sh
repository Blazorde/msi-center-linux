#!/usr/bin/env bash
# Builds the Arch packages (user level, never installs anything, never needs sudo):
#
#   scripts/package.sh                       # committed HEAD of this clone
#   scripts/package.sh --ref tag=v0.1.0      # makepkg git fragment: tag=…, commit=<sha>, branch=…
#   scripts/package.sh -- --nocheck          # extra makepkg options after --
#
# makepkg runs in a temporary directory (sources cloned from this repository, cargo build --release --frozen);
# the packages land in dist/ (gitignored): msi-center-linux-<ver>-<rel>-x86_64.pkg.tar.zst and
# msi-ec-blazorde-dkms-<ver>-<rel>-any.pkg.tar.zst. Uncommitted changes are NOT packaged (a warning says so).
# Install afterwards (as the owner, see docs/user/installation.md): sudo pacman -U dist/*.pkg.tar.zst
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo/dist"
ref=""
extra=()

while (($#)); do
    case $1 in
        --ref) ref=${2:?--ref attend tag=…, commit=… ou branch=…}; shift 2 ;;
        --) shift; extra=("$@"); break ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "option inconnue : $1" >&2; exit 2 ;;
    esac
done

[[ $EUID -ne 0 ]] || { echo "À lancer en utilisateur (makepkg refuse root)." >&2; exit 1; }
command -v makepkg >/dev/null || { echo "makepkg absent (pacman)." >&2; exit 1; }

if [[ -z $ref ]]; then
    # A worktree's HEAD is not the HEAD a clone would see: always pin the exact commit.
    ref="commit=$(git -C "$repo" rev-parse HEAD)"
    [[ -z $(git -C "$repo" status --porcelain) ]] ||
        echo "⚠ modifications non commitées ignorées : seul $(git -C "$repo" rev-parse --short HEAD) est empaqueté" >&2
fi

if command -v namcap >/dev/null; then
    namcap "$repo/packaging/arch/PKGBUILD" || true
fi

build=$(mktemp -d "${TMPDIR:-/tmp}/msi-center-pkg.XXXXXX")
trap 'rm -rf "$build"' EXIT
cp "$repo/packaging/arch/PKGBUILD" "$repo/packaging/arch/msi-center-linux.install" "$build/"

mkdir -p "$out"
echo "→ makepkg dans $build (source : $repo#$ref)"
(
    cd "$build"
    MSI_CENTER_REPO="$repo" MSI_CENTER_REF="$ref" PKGDEST="$out" SRCPKGDEST="$out" \
        BUILDDIR="$build" makepkg --cleanbuild --force "${extra[@]}"
)

if command -v namcap >/dev/null; then
    for p in "$out"/*.pkg.tar.zst; do namcap "$p" || true; done
fi
echo "✓ Paquets dans $out :"
ls -1 "$out"/*.pkg.tar.zst
echo "  Installer : sudo pacman -U $(cd "$repo" && printf '%s ' dist/*.pkg.tar.zst)"
