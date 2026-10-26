# Changelog

Format : [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) · Versions : [SemVer](https://semver.org/lang/fr/).

## [Unreleased]

### Added
- App : **changer de profil sans commande**. Grand sélecteur en verre Éco / Équilibré / Gaming sur l'Accueil
  (icône, nom, description, un clic applique, état optimiste avec « Application… », retour au profil réel et
  message en cas d'erreur, avertissement « RTX 4070 occupée » dans la carte), sélecteur compact dans la barre
  de titre des autres pages, section Profil du menu, raccourcis `Ctrl+1/2/3`, actions GApplication
  `app.profile::eco|balanced|gaming` et `app.profile-eco|…` pour l'extension et les scripts, et actions du
  fichier `.desktop` (clic droit sur l'icône du dock), utilisables fenêtre fermée.
- App : refonte visuelle « Liquid Glass × Golden Gate » ([brief et captures](docs/design/app-liquid-glass.md)) :
  verre en couches (reflet spéculaire, bords intérieur/extérieur, profondeur), grands titres de page,
  chiffres tabulaires, jauges en anneau au balayage animé, graphique des paliers avec remplissage et
  repère lisible, légende en pastilles, survol/appui animés (respecte « mouvement réduit »), parité
  clair/sombre, contraste élevé inchangé (Adwaita pur).
- App : mode démo `MSI_CENTER_DEMO=1` (services simulés dans le processus, aucun bus) et
  `scripts/app-shots.sh` (captures de toutes les pages en clair et sombre dans un GNOME headless isolé).
- Sécurité S9 (ventilo bloqué) : ventilo à 0 % (et 0 tr/min si connu) sur puce ≥ 70 °C (filtrée sur 2 lectures)
  pendant 30 s → tables d'usine + `auto`, `SafetyEvent("fan_stalled")`, lecture seule jusqu'au redémarrage
  (`msictl status` : `SAFETY STOP (S9)`).
- `RestoreFactory()` (D-Bus, polkit `set-fan-curve`) et `msictl factory` : rendre la main à l'EC (tables d'usine
  puis `auto`) jusqu'au prochain profil. `msi-daemon --restore-factory` (sans D-Bus, idempotent).
- Paquets Arch (phase 9) : `packaging/arch/PKGBUILD` scindé en `msi-center-linux` (daemon, `msictl`, `clipd`,
  app, unités, presets systemd, D-Bus, polkit, autostart `/etc/xdg/autostart`, extension GNOME avec schéma
  compilé) et `msi-ec-blazorde-dkms` (sources DKMS, `/usr/lib/modules-load.d/msi-ec.conf`, en conflit avec les
  paquets `msi-ec` upstream). `msi-daemon` (démarrage) et `clipd` (chaque session) activés à l'installation par
  presets ; arrêt du daemon (tables d'usine, `auto`) avant désinstallation. `scripts/package.sh` construit dans
  `dist/` sans rien installer ; `scripts/release.sh` joint ces paquets à la release et vérifie les versions
  (`Cargo.toml`, `PKGBUILD`, `metadata.json`). Docs : [installation](docs/user/installation.md) (installation en
  une fois, migration depuis `dev-install.sh`), [checklist v0.1.0](docs/product/release-checklist.md).
- `scripts/dev-install.sh --enable` : démarrage automatique (DKMS `msi-ec-blazorde`, `modules-load.d`,
  `msi-daemon` activé, `clipd` pour chaque session) ; `--uninstall` défait tout via le manifeste.

### Changed
- App : le profil `balanced` s'affiche « Équilibré » ; les événements `gpu_busy` et `fan_stalled` ont un
  message en français ; en liste, les cartes du presse-papiers prennent la hauteur de leur contenu.
- Ventilo CPU piloté en direct ([ADR-0010](docs/adr/0010-hybrid-cpu-fan-control.md), accepté) : l'EC ignore
  les seuils CPU écrits, donc `msi-daemon` garde les seuils d'usine et module chaque seconde les vitesses des
  paliers 0–3 depuis la courbe exponentielle du profil, sur une température CPU filtrée (médiane + moyenne,
  pics ignorés), avec hystérésis d'arrêt, bande morte et montée/descente progressives ; paliers chauds
  toujours ≥ 60 / 100 / 100 %. Silencieux : arrêté sous 46–50 °C, démarrage doux à 30 % dès 50 °C, 100 % à
  82 °C. Seul `cpu/fan_curve_speeds` est écrit, au changement, relu (S7). `FanCurve("cpu")` renvoie le
  vecteur en vigueur, nouvelle clé `cpu_fan_target_pct` dans `Telemetry` (compatible). L'app affiche le CPU
  « piloté en direct » ; `msictl curve show cpu` aussi. Essai propriétaire :
  `scripts/spike/hybrid-cpu-test.sh`.
- Fondations du projet : workspace Cargo, documentation, ADR 0001–0007, workflow git.
- CI locale : `scripts/ci.sh`, hooks `pre-push` et `commit-msg`, `scripts/release.sh` (pas de GitHub Actions).
- `msi-core` : courbe ventilo exponentielle validée, échantillonnage sur la table EC (disposition provisoire de
  7 points), planchers de sécurité S2 relevés et signalés, validation des tables brutes (S1–S3), superviseur
  thermique Cooler Boost (S4), profils `eco` / `balanced` / `gaming`, carte EC msi-ec `CONF_G2_5`.
- Presse-papiers (phase 6) : `clip-core` (modèle de clip, dédup par hash, tags, recherche, rétention 500
  non épinglés, limite 10 Mio) et service utilisateur `clipd` exposant `org.blazorde.Clipd1` sur le bus session
  (non épinglés sur tmpfs effacés au logout, épinglés dans `~/.local/share/clipd/clips.db`). Unité systemd
  utilisateur durcie et activation D-Bus.
- `msi-daemon` : gestion d'alimentation du dGPU (détection, état `active`/`suspended`/`removed`, processus qui
  le tiennent, runtime PM, retrait PCI avec pré-contrôles et rescan) et télémétrie (CPU, RAM, iGPU, dGPU via NVML
  sans jamais réveiller le GPU, NPU, températures, ventilos). Commande `msi-daemon --probe` en lecture seule.
- Docs : [alimentation GPU & télémétrie](docs/hardware/gpu-power.md), ADR 0008.
- Noyau (phase 3) : fork de `msi-ec` vendu dans `kernel/msi-ec/` (upstream `d7fbbd8`), firmware `14K2EMS1.110`
  accepté, attributs sysfs `cpu/` et `gpu/` `fan_curve_speeds`, `fan_curve_up_temps`, `fan_curve_down_temps`
  validés dans le noyau (bornes, monotonie, `down < up`, relecture après écriture), paquet DKMS
  `msi-ec-blazorde`, `scripts/kernel-build.sh`, procédure de test `kernel/TESTING.md`. Docs :
  [interface sysfs des courbes](docs/hardware/fan-curve-sysfs.md).
- `msi-daemon` (phase 4) : service système exposant `org.blazorde.MsiCenter1` (API figée,
  [dbus-api.md](docs/architecture/dbus-api.md)) : profils `eco` / `balanced` / `gaming` appliqués à l'EC
  (courbes ordonnées, `fan_mode=advanced`, shift mode, relecture S7 avec repli `auto`), courbes personnalisées
  par profil et ventilo avec rapport des planchers S2, Cooler Boost forcé par S4, limite de charge 50–100 %,
  profil automatique au branchement secteur, réglages dans `/var/lib/msi-center/state.toml` ré-appliqués au
  démarrage et après la veille, watchdog systemd, retour en `auto` à l'arrêt et en cas de panic. Autorisations
  polkit `org.blazorde.msicenter.*`. Mode lecture seule si le firmware EC est inconnu (S8).
- `msictl` : `status`, `profile`, `curve show`, `charge-limit`, `telemetry --watch`.
- `systemd/` : `msi-daemon.service` (`Type=notify`, `WatchdogSec`, `ExecStopPost`), politique et activation
  D-Bus système, politique polkit. ADR 0009.
- App `msi-center` (phase 8, GTK 4 / libadwaita) : pages Accueil (profils, jauges en direct), Ventilateurs
  (graphique des 7 paliers avec la température actuelle, paliers relevés par S2 signalés, réglage de la
  courbe avec aperçu `PreviewFanCurve`, Cooler Boost), Performance & GPU (processus qui tiennent la RTX 4070,
  profil automatique secteur), Batterie (limite de charge), Presse-papiers (recherche, tags, liste/grille,
  miniatures, épingles, sélection multiple) et Réglages (lancement à l'ouverture de session, arrière-plan,
  thème). Style « Liquid Glass » aux couleurs du Golden Gate, clair et sombre. Lancement automatique en
  arrière-plan à chaque session avec notifications des alertes de sécurité ; installation utilisateur par
  `scripts/app-install.sh`. Erreurs D-Bus traduites, services absents gérés (réessai). L'app dessine sur
  l'iGPU et ne réveille jamais la RTX 4070. Docs : [l'application](docs/user/app.md), ADR 0011.
- Extension GNOME Shell `msi-center@blazorde` (phase 7, GNOME 51) : métriques dans la barre supérieure poussées
  par `Telemetry` (CPU, dGPU ou son état, iGPU, RAM, NPU, températures, ventilos ; chacune activable, ordre
  réglable, mode icône seule), menu (profils Éco / Équilibré / Gaming, Cooler Boost, profil auto secteur,
  limite de charge appliquée au relâchement, bandeaux lecture seule / sécurité / `GpuBusy`, mini-graphe, ouverture
  de l'app), capture du presse-papiers (texte et images) vers `clipd` et popup `Super+V` (recherche, tags,
  épinglage, suppression multiple, vignettes, navigation clavier, copier / coller). Style « Liquid Glass » ×
  Golden Gate clair et sombre. Ne démarre jamais `msi-daemon` (pas d'auto-activation D-Bus).
  Docs : [extension](docs/user/extension.md).
- `scripts/ext-install.sh` (installation utilisateur, sans sudo), `scripts/ext-check.sh` (vérifications
  hors-ligne de l'extension, appelées par `scripts/ci.sh`), `scripts/ext-smoke.sh` (test de bout en bout dans un
  GNOME Shell headless isolé avec un faux `msi-daemon`).
- Arrêt du daemon (signal, sortie, panic, `ExecStopPost`) et replis S7 : les **tables d'usine** sont réécrites
  avant `fan_mode=auto` (l'EC n'a qu'une copie des tables : avant, « auto » gardait nos courbes). Tables d'usine
  du firmware `14K2EMS1.110` dans `msi-core`.
- `SetProfile("eco")` réussit quand le dGPU est tenu : profil appliqué, dGPU gardé en veille runtime,
  `SafetyEvent("gpu_busy")` ; `msictl profile eco` affiche un avertissement (processus, règle udev ADR-0008,
  `nvidia-powerd`) au lieu d'échouer.
- Activation D-Bus système de `msi-daemon` : installée seulement par `dev-install.sh --enable` (jamais par le
  paquet).
- App : l'interrupteur « Lancer à l'ouverture de session » tient compte de l'entrée système
  `/etc/xdg/autostart` du paquet (l'éteindre écrit une entrée utilisateur `Hidden=true`).
- `msi-core` : la table EC suit le modèle mesuré par paliers (7 vitesses, 6 seuils de montée, 6 de descente)
  au lieu de la disposition provisoire de 7 points ; nouveaux presets **Silencieux** (ventilo arrêté sous 50 °C,
  démarrage doux à 30 %), **Progressif** et **Agressif** ; filtre anti-pics pour S4 (2 lectures consécutives) ;
  firmware `14K2EMS1.110` reconnu.
- Presse-papiers : taguer un clip (`SetTags` avec au moins un tag) l'épingle automatiquement, il survit donc au
  logout. Vider les tags ne désépingle pas ; désépingler un clip tagué conserve ses tags. Signature D-Bus
  inchangée (ADR 0005, mise à jour du 2026-10-25).
