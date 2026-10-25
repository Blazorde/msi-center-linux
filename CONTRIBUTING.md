# Contribuer

## Prérequis

Voir [docs/development/setup.md](docs/development/setup.md).

## Workflow en 6 étapes

1. Partir de `develop` à jour : `git switch develop && git pull`
2. Créer une branche : `git switch -c feat/daemon-fan-curve`
3. Commits au format [Conventional Commits](https://www.conventionalcommits.org/fr/) : `feat(daemon): …`
4. Vérifier en local : `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
5. Ouvrir une PR vers `develop` (le template s'affiche), CI verte obligatoire
6. Squash merge, la branche est supprimée

Détails (releases, hotfix, protection des branches) : [docs/development/git-workflow.md](docs/development/git-workflow.md).

## Règles

- Une PR = un sujet. Petite de préférence (< 400 lignes hors lockfile).
- Tests pour toute logique dans les crates `*-core`.
- Docs mises à jour dans la même PR que le code.
- Toute décision d'architecture → un ADR dans [`docs/adr/`](docs/adr/README.md).
- **Sécurité matérielle** : lire [docs/hardware/fan-safety.md](docs/hardware/fan-safety.md) avant de toucher au
  code ventilateurs/EC.
