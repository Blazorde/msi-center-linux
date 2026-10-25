# Changelog

Format : [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) · Versions : [SemVer](https://semver.org/lang/fr/).

## [Unreleased]

### Added
- Fondations du projet : workspace Cargo, documentation, ADR 0001–0007, workflow git.
- CI locale : `scripts/ci.sh`, hooks `pre-push` et `commit-msg`, `scripts/release.sh` (pas de GitHub Actions).
- `msi-core` : courbe ventilo exponentielle validée, échantillonnage sur la table EC (disposition provisoire de
  7 points), planchers de sécurité S2 relevés et signalés, validation des tables brutes (S1–S3), superviseur
  thermique Cooler Boost (S4), profils `eco` / `balanced` / `gaming`, carte EC msi-ec `CONF_G2_5`.
