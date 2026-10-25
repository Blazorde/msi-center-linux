# Changelog

Format : [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) · Versions : [SemVer](https://semver.org/lang/fr/).

## [Unreleased]

### Added
- Fondations du projet : workspace Cargo, documentation, ADR 0001–0007, workflow git.
- CI locale : `scripts/ci.sh`, hooks `pre-push` et `commit-msg`, `scripts/release.sh` (pas de GitHub Actions).
- Presse-papiers (phase 6) : `clip-core` (modèle de clip, dédup par hash, tags, recherche, rétention 500
  non épinglés, limite 10 Mio) et service utilisateur `clipd` exposant `org.blazorde.Clipd1` sur le bus session
  (non épinglés sur tmpfs effacés au logout, épinglés dans `~/.local/share/clipd/clips.db`). Unité systemd
  utilisateur durcie et activation D-Bus.
