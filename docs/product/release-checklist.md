# Checklist de release — v0.1.0

Première release (phase 9). Procédure générale : [workflow git](../development/git-workflow.md#faire-une-release).
Les cases « propriétaire » se font sur la machine réelle ; personne d'autre ne lance le daemon ni n'installe.

## 1. Avant de geler (sur `develop`)

- [ ] Toutes les PR de la v0.1.0 mergées dans `develop` (phases 3 à 9), `scripts/ci.sh` vert sur `develop`.
- [ ] Paquet construit depuis `develop` : `scripts/package.sh` → deux fichiers dans `dist/`.
- [ ] Contenu vérifié sans installer :
      `tar -tf dist/msi-center-linux-*.pkg.tar.zst | sort` et `tar -tf dist/msi-ec-blazorde-dkms-*.pkg.tar.zst`
      (binaires `msi-daemon msictl clipd msi-center`, unités, presets, polkit, D-Bus, autostart,
      extension avec `schemas/gschemas.compiled`, `/usr/src/msi-ec-blazorde-0.13-blazorde.1/dkms.conf`,
      `/usr/lib/modules-load.d/msi-ec.conf`).
- [ ] `namcap` sur le PKGBUILD et les paquets si disponible (`sudo pacman -S namcap`), avertissements lus.

## 2. Test propriétaire (machine réelle)

- [ ] Migration depuis l'installation de dev ([installation.md](../user/installation.md#passer-de-linstallation-de-développement-au-paquet)) :
      `sudo scripts/dev-install.sh --uninstall`, `scripts/app-install.sh --uninstall`, extension `~/.local` retirée,
      `dkms status | grep -i msi` vide.
- [ ] `sudo pacman -U dist/*.pkg.tar.zst` : le hook DKMS compile le module pour chaque noyau installé, sans erreur.
- [ ] Messages post-install lus ; `systemctl is-enabled msi-daemon` → `enabled`,
      `systemctl --global is-enabled clipd` → `enabled`.
- [ ] `gnome-extensions enable msi-center@blazorde`, puis redémarrage.
- [ ] Après redémarrage, **sans rien lancer à la main** :
  - [ ] `modinfo -n msi_ec` → `…/updates/dkms/…`, `cat /sys/module/msi_ec/version` → `0.13-blazorde.1`
  - [ ] `systemctl status msi-daemon` actif, `msictl status` cohérent (firmware reconnu, pas lecture seule)
  - [ ] `systemctl --user status clipd` actif, `Super+V` montre l'historique
  - [ ] `msi-center` tourne en arrière-plan (`pgrep -a msi-center` → `--background`), la fenêtre s'ouvre depuis le menu
  - [ ] l'extension est dans la barre du haut avec les métriques
- [ ] Profils `eco` / `balanced` / `gaming` depuis l'app et l'extension, courbes relues dans l'EC.
- [ ] Réglages → « Lancer à l'ouverture de session » éteint → reconnexion : l'app ne démarre pas ;
      rallumé → reconnexion : elle démarre.
- [ ] `sudo systemctl stop msi-daemon` → `fan_mode` repasse à `auto` (S5) ; `sudo systemctl start msi-daemon`.
- [ ] Désinstallation testée puis réinstallation : `sudo pacman -Rns msi-center-linux msi-ec-blazorde-dkms` →
      message d'arrêt du daemon, EC en `auto`, `dkms status` sans `msi-ec-blazorde` ; puis `pacman -U` à nouveau.
- [ ] Mise à jour du noyau (si une est disponible) : le module est reconstruit, tout redémarre.

## 3. Gel et publication (lead, après le feu vert du propriétaire)

- [ ] `git switch develop && git pull && git switch -c release/0.1.0`
- [ ] Versions à `0.1.0` : `Cargo.toml` (`[workspace.package] version`, puis `cargo check` pour `Cargo.lock`),
      `extension/msi-center@blazorde/metadata.json` (`version-name`, déjà `0.1.0`), `packaging/arch/PKGBUILD`
      (`pkgver=0.1.0`, `pkgrel=1`).
- [ ] `CHANGELOG.md` : `[Unreleased]` → `## [0.1.0] - AAAA-MM-JJ`, nouvelle section `[Unreleased]` vide.
- [ ] Roadmap / PRD : phase 9 ✅.
- [ ] `scripts/ci.sh` vert, `git commit -am "chore(release): 0.1.0"`, push, PR `release/0.1.0` → `main`.
- [ ] Merge en **merge commit** (pas de squash).
- [ ] `git switch main && git pull && scripts/package.sh` → `dist/*-0.1.0-1-*.pkg.tar.zst`.
- [ ] `scripts/release.sh 0.1.0` → tag `v0.1.0`, release GitHub avec l'archive des binaires et les deux paquets.
- [ ] Release vérifiée sur GitHub (notes = section `[0.1.0]`, 3 fichiers joints).
- [ ] Back-merge : `main` → `develop` (PR si le push direct est refusé).
- [ ] Le propriétaire installe le paquet de la release (`sudo pacman -U` sur les fichiers téléchargés) : même
      contrôle rapide qu'en section 2 après redémarrage.
