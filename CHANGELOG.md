# Changelog

Format : [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) · Versions : [SemVer](https://semver.org/lang/fr/).

## [Unreleased]

### Added
- Fondations du projet : workspace Cargo, documentation, ADR 0001–0007, workflow git.
- CI locale : `scripts/ci.sh`, hooks `pre-push` et `commit-msg`, `scripts/release.sh` (pas de GitHub Actions).
- `msi-daemon` : gestion d'alimentation du dGPU (détection, état `active`/`suspended`/`removed`, processus qui
  le tiennent, runtime PM, retrait PCI avec pré-contrôles et rescan) et télémétrie (CPU, RAM, iGPU, dGPU via NVML
  sans jamais réveiller le GPU, NPU, températures, ventilos). Commande `msi-daemon --probe` en lecture seule.
- Docs : [alimentation GPU & télémétrie](docs/hardware/gpu-power.md), ADR 0008.
