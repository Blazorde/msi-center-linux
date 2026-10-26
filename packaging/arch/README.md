# packaging/arch

`PKGBUILD` scindé en deux paquets, construit depuis un commit du dépôt (jamais l'arbre de travail) :

| Paquet | Contenu |
|---|---|
| `msi-center-linux` | binaires, unités systemd + presets, D-Bus, polkit, app (menu, icônes, `/etc/xdg/autostart`), extension GNOME |
| `msi-ec-blazorde-dkms` | sources DKMS du module `msi-ec` (`/usr/src/msi-ec-blazorde-<ver>/`), `/usr/lib/modules-load.d/msi-ec.conf` |

`msi-center-linux.install` : applique les presets (`msi-daemon` au démarrage, `clipd` à chaque session), affiche
les commandes (français) et arrête le daemon avant la désinstallation (tables d'usine puis `auto`).

```bash
scripts/package.sh                  # → dist/*.pkg.tar.zst, n'installe rien
sudo pacman -U dist/*.pkg.tar.zst   # installation (propriétaire)
```

Au bump de version : `pkgver` = `[workspace.package] version` de `Cargo.toml` (vérifié dans `prepare()`), et
`_dkms_ver` = `PACKAGE_VERSION` de `kernel/msi-ec/dkms.conf`. Voir [installation](../../docs/user/installation.md)
et le [workflow de release](../../docs/development/git-workflow.md#faire-une-release).
