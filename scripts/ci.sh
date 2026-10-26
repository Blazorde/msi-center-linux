#!/usr/bin/env bash
# Local CI: the single source of truth for checks (GitHub Actions is not used, see docs/adr/0007-local-ci.md).
# Run by the pre-push hook and before every release.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$1"; }

step "fmt"
cargo fmt --all --check

step "clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "test"
cargo test --workspace --locked

step "extension GNOME"
scripts/ext-check.sh

step "deny"
# Via cargo (not `command -v`): cargo also finds subcommands in ~/.cargo/bin, which is often not in PATH.
if cargo deny --version >/dev/null 2>&1; then
    cargo deny check
else
    echo "cargo-deny absent : cargo install cargo-deny --locked" >&2
    exit 1
fi

printf '\n\033[1;32m✓ CI locale OK\033[0m\n'
