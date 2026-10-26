# Changelog

Format : [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) · Versions : [SemVer](https://semver.org/lang/fr/).

## [Unreleased]

### Added
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

### Changed
- `msi-core` : la table EC suit le modèle mesuré par paliers (7 vitesses, 6 seuils de montée, 6 de descente)
  au lieu de la disposition provisoire de 7 points ; nouveaux presets **Silencieux** (ventilo arrêté sous 50 °C,
  démarrage doux à 30 %), **Progressif** et **Agressif** ; filtre anti-pics pour S4 (2 lectures consécutives) ;
  firmware `14K2EMS1.110` reconnu.
- Presse-papiers : taguer un clip (`SetTags` avec au moins un tag) l'épingle automatiquement, il survit donc au
  logout. Vider les tags ne désépingle pas ; désépingler un clip tagué conserve ses tags. Signature D-Bus
  inchangée (ADR 0005, mise à jour du 2026-10-25).
