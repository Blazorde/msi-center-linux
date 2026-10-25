#!/usr/bin/env bash
# Out-of-tree build of the msi-ec fork (kernel/msi-ec) against an installed kernel. Build only: never installs or
# loads the module. Sources are copied to target/kernel/msi-ec so build artefacts stay out of the source tree.
# Fails on any compiler warning.
#
# Usage: scripts/kernel-build.sh [kernel-release]   (default: running kernel)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

kver="${1:-$(uname -r)}"
kdir="/usr/lib/modules/${kver}/build"
src="kernel/msi-ec"
out="target/kernel/msi-ec"

if [[ ! -d "$kdir" ]]; then
    echo "kernel headers not found: $kdir (install the matching *-headers package)" >&2
    exit 1
fi

llvm=()
if grep -q '^CONFIG_CC_IS_CLANG=y' "$kdir/.config" 2>/dev/null; then
    llvm=(LLVM=1)
fi

rm -rf "$out"
mkdir -p "$out"
cp "$src"/msi-ec.c "$src"/ec_memory_configuration.h "$src"/Makefile "$src"/Makefile.vars "$out"/

log="$out/build.log"
make -C "$kdir" M="$PWD/$out" "${llvm[@]}" modules 2>&1 | tee "$log"

# Compiler diagnostics only (file:line:col: warning: ...), not kbuild's "compiler differs" notice.
if grep -q ': warning: ' "$log"; then
    echo "build produced warnings (see $log)" >&2
    exit 1
fi

printf '\n\033[1;32m✓ %s/msi-ec.ko built for %s%s\033[0m\n' "$out" "$kver" "${llvm[*]:+ (${llvm[*]})}"
