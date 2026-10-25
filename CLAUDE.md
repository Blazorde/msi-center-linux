# CLAUDE.md — agent guide

Read this first. Human docs live in `docs/` (French); this file is the agent contract (English, terse).

## Project

MSI Center replacement for **one** laptop: MSI Stealth 14 AI Studio A1VGG (MS-14K2, EC fw `14K2EMS1.1xx`,
Core Ultra 9 185H, Arc iGPU, RTX 4070 Laptop, Intel NPU) on CachyOS + GNOME 51 Wayland. Plus a clipboard manager.
Product spec: `.claude/PRPs/prds/msi-center-linux.prd.md`. Next work: `/prp-plan` on that PRD.

## Layout

| Path | What | Lang |
|---|---|---|
| `crates/msi-core` | Pure logic: EC map, fan curve model, safety validation, profiles. No I/O. | Rust |
| `crates/msi-daemon` | Root system service, D-Bus `org.blazorde.MsiCenter1`, sysfs I/O, GPU power | Rust |
| `crates/clip-core` | Pure logic: clip model, tags, dedup, retention | Rust |
| `crates/clipd` | User service, D-Bus `org.blazorde.Clipd1`, storage | Rust |
| `crates/msi-center` | GTK4/libadwaita app | Rust |
| `extension/msi-center@blazorde` | GNOME Shell extension: top bar, Super+V popup, clipboard capture | GJS (ESM) |
| `kernel/` | Fork of `msi-ec` with fan curve sysfs (DKMS) | C |
| `packaging/arch` | PKGBUILD | sh |
| `systemd/` | Units, D-Bus service files, polkit policy | ini/xml |
| `docs/` | Human docs → future wiki. ADRs in `docs/adr/` | md |

## Commands

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run all three before declaring work done. CI runs the same (`.github/workflows/ci.yml`).

## Hard rules (hardware safety)

1. **Never reflash or modify EC firmware.** EC RAM writes only, only through the msi-ec sysfs interface.
2. **No raw EC access** (`/dev/port`, `ec_sys`, `acpi_call`) in production code. Spike scripts under
   `scripts/spike/` may read EC, never write.
3. Every value sent to the EC goes through `msi-core` validation (see `docs/hardware/fan-safety.md`). Safety
   invariants must have unit tests; never weaken them to make a test pass.
4. Daemon shutdown/crash path must restore EC fan mode `auto`.
5. Do not run commands that write to `/sys` on the owner's machine without explicit confirmation.

## Code rules

- Business logic in `*-core` crates, side effects in binaries (functional core, imperative shell).
- Errors: `thiserror` in libs, `anyhow` only in binaries. No `unwrap()`/`expect()` outside tests.
- `unsafe` forbidden (`#![forbid(unsafe_code)]`) unless an ADR says otherwise.
- New dependency → justify in PR description; prefer what's already in `[workspace.dependencies]`.
- D-Bus interfaces are versioned (`…1`); breaking change = new interface name. Document in `docs/architecture/dbus-api.md`.
- Architectural decision → new ADR in `docs/adr/` (copy `0000-template.md`).

## Git workflow

- `main` = released, `develop` = integration. Both protected, PR only.
- Branch from `develop`: `feat/<scope>-<desc>`, `fix/…`, `docs/…`, `chore/…`, `refactor/…`, `test/…`.
- Conventional Commits: `feat(daemon): add fan curve sampling`. Scopes: `core`, `daemon`, `clip`, `clipd`,
  `app`, `ext`, `kernel`, `pkg`, `ci`, `docs`.
- PR into `develop` (squash). Release: `release/x.y.z` → `main` (merge commit) + tag `vx.y.z` → back-merge to `develop`.
- Never commit/push/merge without the owner asking. Never force-push `main`/`develop`.
- Full doc: `docs/development/git-workflow.md`.

## Docs

Keep docs in sync with code in the same PR. Update `CHANGELOG.md` (`[Unreleased]`) for user-visible changes.
Update the PRD phase table status when a phase starts/completes.
