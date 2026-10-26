# kernel/ — fork de msi-ec

Fork de [`BeardOverflow/msi-ec`](https://github.com/BeardOverflow/msi-ec) (GPL-2.0-or-later, voir
[`msi-ec/LICENSE`](msi-ec/LICENSE)) avec les attributs sysfs de courbe ventilo, distribué en DKMS.
Voir [ADR-0002](../docs/adr/0002-ec-access-via-msi-ec-fork.md) et le contrat
[`docs/hardware/fan-curve-sysfs.md`](../docs/hardware/fan-curve-sysfs.md).

## Origine

| | |
|---|---|
| Upstream | <https://github.com/BeardOverflow/msi-ec> |
| Commit vendu | `d7fbbd88e6831e56801b860e46475cbf8ddbc7c1` (2026-08-10, « add firmware 155LEMS1.104 for Modern 15 A5M ») |
| Fichiers repris | `msi-ec.c`, `ec_memory_configuration.h`, `Makefile`, `Makefile.vars`, `LICENSE`, `README.md`, `.gitignore`, `.clang-format`, `docs/sysfs-platform-msi-ec` |
| Différences | listées dans [`msi-ec/PATCHES.md`](msi-ec/PATCHES.md) ; balises `msi-center-linux` dans le code |

## Construire (sans installer)

```bash
scripts/kernel-build.sh            # noyau courant ; ou scripts/kernel-build.sh <release>
```

Copie les sources dans `target/kernel/msi-ec/`, compile hors arbre (`LLVM=1` si le noyau est compilé avec clang,
comme sur CachyOS), échoue au moindre avertissement du compilateur. Prérequis : `linux-cachyos-headers`.

Test manuel par le propriétaire : [`TESTING.md`](TESTING.md).

## Installer via DKMS

Paquet DKMS `msi-ec-blazorde`, version `0.13.1` (sans tiret : le hook pacman de DKMS coupe au dernier `-`), module `0.13-blazorde.1` ; le module garde le nom `msi_ec`. Ne **pas** utiliser
les cibles `make install` / `make dkms-install` du `Makefile` upstream (elles visent le paquet upstream `msi_ec`).

```bash
# 0. Retirer un éventuel paquet upstream (AUR msi-ec-git / msi-ec-dkms, ou dkms "msi-ec")
dkms status | grep -i msi
# sudo dkms remove msi-ec/<version> --all

# 1. Installer
sudo dkms add ./kernel/msi-ec                     # copie dans /usr/src/msi-ec-blazorde-0.13.1
sudo dkms install msi-ec-blazorde/0.13.1 # build (LLVM=1 ajouté par dkms pour un noyau clang) + install
modinfo -F version msi_ec                         # 0.13-blazorde.1 (le module DKMS dans updates/ prime)

# 2. Charger (et au démarrage, facultatif)
sudo modprobe -r msi_ec; sudo modprobe msi_ec
echo msi_ec | sudo tee /etc/modules-load.d/msi-ec.conf
```

Tout cela (plus le daemon au démarrage) est automatisé par `sudo scripts/dev-install.sh --enable` ; voir
[installation](../docs/user/installation.md).

Désinstaller :

```bash
sudo modprobe -r msi_ec
sudo dkms remove msi-ec-blazorde/0.13.1 --all
sudo rm -rf /usr/src/msi-ec-blazorde-0.13.1
sudo rm -f /etc/modules-load.d/msi-ec.conf
```

Après une mise à jour du dépôt avec une nouvelle `PACKAGE_VERSION` : désinstaller l'ancienne version puis
réinstaller.
