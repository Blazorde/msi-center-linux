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

### Changed
- Presse-papiers : taguer un clip (`SetTags` avec au moins un tag) l'épingle automatiquement, il survit donc au
  logout. Vider les tags ne désépingle pas ; désépingler un clip tagué conserve ses tags. Signature D-Bus
  inchangée (ADR 0005, mise à jour du 2026-10-25).
