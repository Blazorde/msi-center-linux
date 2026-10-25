#!/usr/bin/env bash
# Publishes a release from main: ./scripts/release.sh 0.1.0
# Prerequisite: release/<version> merged into main (see docs/development/git-workflow.md).
set -euo pipefail

version=${1:?usage: scripts/release.sh <x.y.z>}
tag="v$version"

cd "$(git rev-parse --show-toplevel)"

[[ $(git branch --show-current) == main ]] || { echo "Doit être lancé depuis main." >&2; exit 1; }
[[ -z $(git status --porcelain) ]] || { echo "Arbre de travail non propre." >&2; exit 1; }
git fetch --quiet origin
[[ $(git rev-parse HEAD) == $(git rev-parse origin/main) ]] || { echo "main n'est pas à jour avec origin." >&2; exit 1; }
grep -q "^version = \"$version\"" Cargo.toml || { echo "Cargo.toml n'est pas en version $version." >&2; exit 1; }
grep -q "^## \[$version\]" CHANGELOG.md || { echo "CHANGELOG.md n'a pas de section [$version]." >&2; exit 1; }

scripts/ci.sh
cargo build --workspace --release --locked

dist=$(mktemp -d)
trap 'rm -rf "$dist"' EXIT
archive="$dist/msi-center-linux-$tag-x86_64.tar.gz"
tar -C target/release -czf "$archive" msi-daemon clipd msi-center
awk -v v="$version" '$0 ~ "^## \\[" v "\\]" {p=1; next} /^## \[/ {p=0} p' CHANGELOG.md > "$dist/notes.md"

git tag -a "$tag" -m "$tag"
git push origin "$tag"
gh release create "$tag" "$archive" --notes-file "$dist/notes.md" --verify-tag

echo "✓ $tag publiée. Pense à : git switch develop && git merge main && git push"
