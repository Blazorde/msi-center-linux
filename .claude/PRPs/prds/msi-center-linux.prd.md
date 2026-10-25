# MSI Center Linux + Clipboard Manager

## Problem Statement

On Linux (CachyOS, GNOME 51 Wayland), the MSI Stealth 14 AI Studio A1VGG has no equivalent of MSI Center: the EC
fan curve is bad (fans stay off, the machine overheats, then fans jump to 100 %), there is no way to switch
performance / eco modes, turn off the RTX 4070 or limit battery charge, and no usage overview in the top bar.
Separately, GNOME has no clipboard history with persistent, tagged clips. Cost today: noise, heat, poor battery life
when working remotely, and booting into Windows for basic hardware control.

## Evidence

- Owner observation: "ils se lancent pas, puis ça chauffe trop, puis ils se mettent à fond".
- LaptopMedia review of the A1V: noisy, poor fan hysteresis in Extreme Performance mode.
- Machine probe (2026-10-25): `msi_wmi_platform` exposes fan RPM read-only (`fan1_input`=2171, `fan2_input`=2201);
  no `msi-ec`, no `ec_sys`; `platform_profile` absent; power-profiles-daemon has only a `placeholder` platform driver.
- RTX 4070 idles at 1.41 W in P8 with runtime PM `active`: possible savings on battery.

## Proposed Solution

A Rust monorepo with privilege-separated backends and thin frontends:
`msi-daemon` (root, system D-Bus) drives the EC through a forked `msi-ec` kernel module extended with fan curve
attributes, manages GPU power (RTD3 / PCI remove-rescan), profiles and battery charge limit, and enforces thermal
safety. `clipd` (user, session D-Bus) stores clipboard history. A GNOME Shell extension (GJS, mandatory for top bar
and Wayland clipboard access) shows live stats and relays clipboard events. A GTK4/libadwaita app (`msi-center`)
hosts the full UI. We do **not** reflash EC firmware: MSI Center itself only writes EC RAM registers, and `msi-ec`
already maps this model (`14K2EMS1.104`–`.109`), so the risk/benefit of firmware modification is unjustifiable.

## Key Hypothesis

We believe a progressive (exponential) EC fan curve, one-click profiles and runtime dGPU power-off will make the
laptop quiet at idle, safe under load and long-lasting on battery for its owner.
We'll know we're right when idle fans are near-silent without off→100 % jumps, temperatures never hit the 95 °C
emergency threshold in daily use, battery runtime in Éco improves measurably over the current baseline, and Windows
is no longer needed for hardware control.

## What We're NOT Building

- EC firmware modification / reflashing - brick risk, no documented firmware, unnecessary.
- Support for other MSI models - single-device project; the EC map is model-specific (design keeps it extensible).
- Running AI workloads on the NPU - only utilisation monitoring; no clear need.
- MUX switch (dGPU-only display mode) - requires reboot, explicitly unwanted.
- RGB / keyboard backlight control - not supported by the EC map for this model (`kbd_bl` unsupported).
- Password-manager exclusion in clipboard - owner chose to capture everything (see risks).
- Distribution on AUR - repository is private.

## Success Metrics

| Metric | Target | How Measured |
|--------|--------|--------------|
| Idle fan behaviour | No off→100 % jumps; fan ≤ 35 % below 60 °C | daemon log of temp/RPM over 1 h idle |
| Thermal safety | 0 events > 95 °C in daily use | daemon safety event counter |
| Battery runtime (Éco, light remote work) | ≥ +25 % vs current baseline | `upower` discharge rate, same workload |
| dGPU off in Éco | 4070 removed from bus, 0 W | `lspci`, battery discharge rate |
| Profile switch latency | < 2 s, no reboot | manual timing |
| Daemon footprint | < 10 MB RSS, < 0.5 % CPU | `ps`, `pidstat` |
| Clipboard purge | 0 unpinned clips after logout | test: copy, logout, login, check |

## Open Questions

- [ ] Confirm EC firmware string (expected `14K2EMS1.1xx`; BIOS is `E14K2IMS.10F`) - spike phase 2.
- [ ] Confirm EC fan curve table layout (CPU ~`0x72`, GPU ~`0x8A`, temperature thresholds) on this firmware.
- [ ] Which processes keep the 4070 awake under GNOME 51 Wayland (gnome-shell? Xwayland?) - spike phase 5.
- [ ] Auto profile switch on AC plug/unplug: default ON, user-toggleable (assumed, confirm in use).
- [ ] Battery charge limit default (proposed 80 %, with a "full charge once" action before travel).

---

## Users & Context

**Primary User**
- **Who**: The owner - Rust developer, CachyOS + GNOME daily driver on the Stealth 14 A1VGG, works remotely on battery.
- **Current behavior**: Lives with noisy/hot default EC behaviour; boots Windows for MSI Center.
- **Trigger**: Fans roaring after silence; battery running low away from a plug; re-finding an old copied snippet.
- **Success state**: Glances at the top bar, one click to change profile, never thinks about fans again.

**Job to Be Done**
When I work on my laptop (remote on battery or gaming plugged in), I want the machine to pick the right
thermal/power behaviour and show me what it is doing, so I can stay focused without noise, heat or battery anxiety.

**Non-Users**
Other MSI owners, other distros/desktops (KDE, Hyprland), people wanting a generic hardware monitor.

---

## Solution Detail

### Core Capabilities (MoSCoW)

| Priority | Capability | Rationale |
|----------|------------|-----------|
| Must | Custom exponential fan curve (CPU + GPU) written to EC | Core pain |
| Must | Thermal safety: speed floors, 95 °C Cooler Boost, EC `auto` on daemon exit/crash, write validation | Hardware protection |
| Must | Profiles Éco / Balanced / Gaming without reboot | Explicit request |
| Must | dGPU power-off at runtime (Éco), RTD3 in Balanced | Battery + explicit request |
| Must | Top bar stats in % (CPU, dGPU, iGPU, RAM, NPU) + temps + RPM, each toggleable in settings | Explicit request |
| Must | Battery charge limit (longevity) | Explicit request |
| Must | Clipboard history text + images, Super+V, pin, tags, search, quick delete | Explicit request |
| Must | Unpinned clips wiped at logout | Explicit request |
| Should | Auto profile on AC plug/unplug | Remote work battery life |
| Should | Show which processes hold the dGPU before power-off | Makes Éco reliable |
| Should | Manual Cooler Boost toggle | Parity with MSI Center |
| Could | Fan curve editor with draggable points (on top of exponential presets) | Power-user tuning |
| Could | Optional clipboard exclusion of password managers | Security toggle if owner changes mind |
| Could | Upstream fan curve support to `msi-ec` | Community, less maintenance |
| Won't | Firmware reflash, MUX, NPU workloads, RGB | See "not building" |

### MVP Scope

Fan curve + safety + three profiles via CLI/D-Bus (phases 2–4). This validates the core hypothesis (quiet & safe)
before any UI work.

### User Flow

Login → extension shows `CPU 12 % · GPU 0 % · 48 °C · 1800 rpm` → click → menu with profile switch → pick Éco →
daemon sets EC shift `eco`, silent curve, removes dGPU → top bar reflects new state.
Copy text → Super+V → popup history → type to search → Enter pastes / Ctrl+P pins / Del deletes / add tag.

---

## Technical Approach

**Feasibility**: HIGH for fans/profiles/battery/clipboard, MEDIUM for runtime dGPU power-off.

**Architecture Notes**
- Rust workspace; pure logic in `msi-core` / `clip-core` (unit-testable without hardware), I/O in binaries.
- EC access only through a forked `msi-ec` (DKMS) extended with fan curve sysfs attributes; the daemon never
  touches `/dev/port` or raw EC memory (ADR-0002).
- IPC over D-Bus (`zbus`), polkit for privileged actions (ADR-0003).
- GNOME Shell extension in GJS is unavoidable for the panel and Wayland clipboard; kept thin (ADR-0004).
- Unpinned clips live in `$XDG_RUNTIME_DIR` (tmpfs, wiped by logind at logout); pinned clips in
  `$XDG_DATA_HOME` SQLite (ADR-0005).
- Exponential curve: `speed(T) = min + (max-min)·((T-T0)/(T1-T0))^k`, sampled to the EC's N points, clamped by
  safety floors (ADR-0006).

**Technical Risks**

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| EC curve layout differs from sibling model | M | Read-only EC dump spike first; diff dumps while MSI Center changes the curve (Windows) |
| Bad EC write overheats hardware | L | Validation in `msi-core` and in kernel, EC firmware thermal shutdown remains, restore `auto` on exit, watchdog |
| dGPU busy, cannot be removed | H | List holders, refuse with clear message, retry on next idle |
| msi-ec fork drifts from upstream | M | Keep patch small, rebase regularly, try to upstream |
| Clipboard stores passwords on disk if pinned | M | Unpinned only in tmpfs; pinned DB mode 0600; documented |
| GNOME Shell API breaks on major update | M | Pin `shell-version`, thin extension, CI lint |

---

## Implementation Phases

| # | Phase | Description | Status | Parallel | Depends | PRP Plan |
|---|-------|-------------|--------|----------|---------|----------|
| 1 | Foundations | Repo, workspace, docs, CI, git workflow | in-progress | - | - | - |
| 2 | EC spike | Install msi-ec, read fw version, dump & map fan curve registers (read-only) | pending | with 5, 6 | 1 | - |
| 3 | msi-ec fork | Fan curve sysfs attributes + DKMS package | pending | - | 2 | - |
| 4 | msi-core + msi-daemon | Curve engine, safety, profiles, battery limit, D-Bus API, CLI | pending | - | 3 | - |
| 5 | GPU manager | RTD3 config, holder detection, PCI remove/rescan | pending | with 2, 6 | 1 | - |
| 6 | clip-core + clipd | SQLite store, tags, search, runtime/persistent split, D-Bus API | pending | with 2, 5 | 1 | - |
| 7 | GNOME extension | Top bar stats (toggleable), profile menu, clipboard capture + Super+V popup | pending | with 8 | 4, 5, 6 | - |
| 8 | msi-center GTK app | Tabs: Fans, Performance/GPU, Battery, Clipboard, Settings | pending | with 7 | 4, 5, 6 | - |
| 9 | Packaging & release | PKGBUILD, systemd units, polkit, release workflow v0.1.0 | pending | - | 7, 8 | - |

### Phase Details

**Phase 1: Foundations**
- **Goal**: A repo anyone (human or agent) can contribute to safely.
- **Scope**: Cargo workspace skeleton, docs tree, ADRs, CLAUDE.md, local CI (no GitHub Actions, ADR-0007), branch model, templates.
- **Success signal**: `scripts/ci.sh` green on `develop`, hooks active, branches protected.

**Phase 2: EC spike**
- **Goal**: Know exactly which EC bytes encode the fan curve on this firmware.
- **Scope**: Read-only. Install upstream msi-ec, record `fw_version`, dump EC in several fan modes/temperatures.
- **Success signal**: `docs/hardware/ec-map.md` documents curve addresses with evidence.

**Phase 3: msi-ec fork**
- **Goal**: Safe kernel interface for curves.
- **Scope**: sysfs `cpu/fan_curve_*`, `gpu/fan_curve_*`, bounds checks in kernel, DKMS.
- **Success signal**: Writing a curve via sysfs changes fan behaviour; invalid values rejected.

**Phase 4: msi-core + msi-daemon**
- **Goal**: The MVP.
- **Scope**: Exponential curve model, safety supervisor, profiles, charge limit, `org.blazorde.MsiCenter1` D-Bus, `msictl` CLI.
- **Success signal**: "Idle fan behaviour" and "thermal safety" metrics met for a week.

**Phase 5: GPU manager**
- **Goal**: dGPU off without reboot.
- **Scope**: Detect holders (`/proc/*/fd` on `/dev/nvidia*`, DRM clients), unbind/remove, rescan.
- **Success signal**: Éco removes the 4070 and restores it without reboot or crash.

**Phase 6: clip-core + clipd**
- **Goal**: Clipboard backend.
- **Scope**: Store, dedup (content hash), tags, search, limits (500 session items, 10 MB per image).
- **Success signal**: Unit tests + logout purge test pass.

**Phase 7 / 8: Frontends**
- **Goal**: Daily usable UI.
- **Success signal**: Owner uses it daily instead of Windows MSI Center.

**Phase 9: Packaging**
- **Goal**: Reproducible install via `makepkg -si`.
- **Success signal**: Clean install on a fresh user, `v0.1.0` tagged on `main`.

### Parallelism Notes

Phases 2, 5 and 6 share nothing (EC vs PCI/NVIDIA vs clipboard) and can run in parallel after phase 1.
Frontends (7, 8) only depend on the D-Bus contracts and can be built against mocks once APIs are frozen.

---

## Decisions Log

| Decision | Choice | Alternatives | Rationale |
|----------|--------|--------------|-----------|
| Firmware | Write EC RAM via msi-ec fork | Reflash EC firmware | Brick risk; MSI Center does the same RAM writes |
| Language | Rust (backends + GTK app) | Tauri, Electron, Python | Owner's language, performance, memory safety |
| Top bar | GNOME Shell extension (GJS) | Tray icon (AppIndicator) | Only way for native panel text + Wayland clipboard |
| GUI | GTK4 + libadwaita (gtk4-rs) | Tauri/web | Native GNOME look, low RAM |
| IPC | D-Bus via zbus + polkit | Unix socket, HTTP | Desktop standard, GJS-native, auth built in |
| Clip storage | tmpfs (session) + SQLite (pinned) | Single DB + logout hook | Purge guaranteed by logind, even after a crash |
| Profiles | Éco / Balanced / Gaming | MSI's 4 modes | Owner validated |
| Packaging | PKGBUILD in repo | AUR, Flatpak | Private repo; Flatpak can't ship a root daemon/DKMS |
| License | GPL-3.0-or-later | MIT, none | Compatible with msi-ec (GPL-2.0+) & GNOME extension ecosystem |
| Workflow | main/develop + short-lived branches, Conventional Commits | Trunk-based | Owner request |
| CI | Local: `scripts/ci.sh` + git hooks | GitHub Actions, self-hosted runner, public repo | Actions quota exhausted; owner refused alternatives (ADR-0007) |

---

## Research Summary

**Market Context**
- [msi-ec](https://github.com/BeardOverflow/msi-ec): kernel module, supports A1VGG (`ALLOWED_FW_G2_5`: `14K2EMS1.104`–`.109`): shift modes (`0xd2`), fan modes (`0xd4`), cooler boost (`0x98` bit 7), charge limit (`0xd7`), CPU temp/fan `0x68`/`0x71`, GPU `0x80`/`0x89`. No fan curves.
- [GhostDeck #107](https://github.com/wygodad/ghostdeck/issues/107): Windows tool; reports curve tables at `0x72` (CPU) / `0x8A` (GPU) on sibling Stealth 14 Studio A13V.
- [msi-wmi-platform patch series](https://patchew.org/linux/20250511204427.327558-1-lkml@antheas.dev): fan curves/profiles via WMI, stalled and unmerged as of [May 2026](https://lkml.indiana.edu/2605.1/01517.html).
- Existing GUIs: MControlCenter (Qt, msi-ec based), GNOME Clipboard Indicator / Pano (GJS-only, no root daemon).

**Technical Context**
- No existing codebase. Machine: kernel 7.2.11-cachyos, NVIDIA 615.78, `intel_vpu` with `/dev/accel0`.
- Runtime dGPU off: mode switch tools (EnvyControl) need a reboot; PCI remove/rescan works without reboot only if no
  process holds the device ([Arch forum](https://bbs.archlinux.org/viewtopic.php?id=314016)).

---

*Generated: 2026-10-25*
*Status: DRAFT - needs validation (phase 2 spike)*
