# Contribuer

## Prérequis

Voir [docs/development/setup.md](docs/development/setup.md).

## Workflow en 6 étapes

0. Une seule fois par clone : `git config core.hooksPath .githooks`
1. Partir de `develop` à jour : `git switch develop && git pull`
2. Créer une branche : `git switch -c feat/daemon-fan-curve`
3. Commits au format [Conventional Commits](https://www.conventionalcommits.org/fr/) : `feat(daemon): …` (vérifié par le hook `commit-msg`)
4. Vérifier : `scripts/ci.sh` (lancé automatiquement par le hook `pre-push`)
5. Ouvrir une PR vers `develop` (le template s'affiche)
6. Squash merge, la branche est supprimée

Il n'y a pas de GitHub Actions : la CI est locale ([ADR-0007](docs/adr/0007-local-ci.md)).

Détails (releases, hotfix, protection des branches) : [docs/development/git-workflow.md](docs/development/git-workflow.md).

## Règles

- Une PR = un sujet. Petite de préférence (< 400 lignes hors lockfile).
- Tests pour toute logique dans les crates `*-core`.
- Docs mises à jour dans la même PR que le code.
- Toute décision d'architecture → un ADR dans [`docs/adr/`](docs/adr/README.md).
- **Sécurité matérielle** : lire [docs/hardware/fan-safety.md](docs/hardware/fan-safety.md) avant de toucher au
  code ventilateurs/EC.
