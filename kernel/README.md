# kernel/ — fork de msi-ec

Fork de [`BeardOverflow/msi-ec`](https://github.com/BeardOverflow/msi-ec) (GPL-2.0-or-later, voir
[`msi-ec/LICENSE`](msi-ec/LICENSE)) avec les attributs sysfs de courbe ventilo, distribué en DKMS.
Voir [ADR-0002](../docs/adr/0002-ec-access-via-msi-ec-fork.md).

## Origine

| | |
|---|---|
| Upstream | <https://github.com/BeardOverflow/msi-ec> |
| Commit vendu | `d7fbbd88e6831e56801b860e46475cbf8ddbc7c1` (2026-08-10, « add firmware 155LEMS1.104 for Modern 15 A5M ») |
| Fichiers repris | `msi-ec.c`, `ec_memory_configuration.h`, `Makefile`, `Makefile.vars`, `LICENSE`, `README.md`, `.gitignore`, `.clang-format`, `docs/sysfs-platform-msi-ec` |
| Différences | listées dans [`msi-ec/PATCHES.md`](msi-ec/PATCHES.md) ; blocs balisés `msi-center-linux` dans le code |
