# Changes vs upstream msi-ec

Upstream: <https://github.com/BeardOverflow/msi-ec> at `d7fbbd88e6831e56801b860e46475cbf8ddbc7c1`.
To see the exact diff, check out upstream at that commit and run
`diff -ru <upstream>/ kernel/msi-ec/ -x .git` (files listed under "Not vendored" below show up as "Only in").
In the code, every change is tagged
`msi-center-linux` (multi-line changes sit between `BEGIN msi-center-linux` / `END msi-center-linux` markers).

| # | File | Change | Why |
|---|---|---|---|
| 1 | `msi-ec.c` | `"14K2EMS1.110"` added to `ALLOWED_FW_G2_5` | Owner's firmware; `CONF_G2_5` registers verified on `.110` (`docs/hardware/ec-map.md`) |
| 2 | `ec_memory_configuration.h` | `struct msi_ec_fan_curve_conf` (`speeds_address`, `up_temps_address`, `down_temps_address`) + `MSI_EC_FAN_CURVE_SPEEDS` (7) / `MSI_EC_FAN_CURVE_TEMPS` (6); new `fan_curve` member in `msi_ec_cpu_conf` and `msi_ec_gpu_conf` | Per-config table addresses. `0` (implicit in every other config) = unsupported |
| 3 | `msi-ec.c` | `CONF_G2_5`: CPU tables `0x72` / `0x6a` / `0x7a`, GPU tables `0x8a` / `0x82` / `0x92` | Measured layout |
| 4 | `msi-ec.c` | New block "fan curve tables": `cpu/` and `gpu/` attributes `fan_curve_speeds`, `fan_curve_up_temps`, `fan_curve_down_temps` (0644), strict parser, validation, cross-table rule, write + read-back (`-EIO` + `pr_warn`), `fan_curve_mutex` | Contract: `docs/hardware/fan-curve-sysfs.md` |
| 5 | `msi-ec.c` | Attributes added to `msi_cpu_attrs` / `msi_gpu_attrs`; `msi_ec_is_visible()` hides them unless the table is supported and the config was matched from the EC's own firmware version | S8 |
| 6 | `msi-ec.c` | `load_configuration()`: sets `fan_curve_fw_verified` (false when the `firmware=` parameter forced the config) | S8 |
| 7 | `msi-ec.c` | `#include <linux/ctype.h>` | `isdigit()` in the parser |
| 8 | `msi-ec.c` | `MODULE_VERSION("0.13-blazorde.1")` | Tell the fork from upstream (`/sys/module/msi_ec/version`) |
| 9 | `dkms.conf` | Replaced: package `msi-ec-blazorde`, plain Kbuild `MAKE` (dkms adds `LLVM=1` for clang kernels) | No clash with upstream's `msi-ec` DKMS package |
| 10 | `PATCHES.md` | This file | |

Not vendored: `packaging/`, `.copr/`, `.github/`, `docs/` (except `docs/sysfs-platform-msi-ec`), `dkms.conf.old`,
`universalDKMSModuleUninstaller.sh`, `.editorconfig`. Upstream's `Makefile` is kept unchanged; its `install` /
`dkms-install` targets target the upstream package and must not be used — see `kernel/README.md`.

## Rebasing on a newer upstream

1. Copy the new upstream files over `kernel/msi-ec/` (same list as above) in a commit of their own.
2. Re-apply the tagged blocks (`grep -n msi-center-linux`), update the commit hash here and in `kernel/README.md`.
3. `scripts/kernel-build.sh` must build warning-free.
