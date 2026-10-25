# 0007 — CI locale au lieu de GitHub Actions

- **Statut** : Accepté (remplace la partie CI de la mise en place initiale)
- **Date** : 2026-10-25

## Contexte

Le quota GitHub Actions du compte est épuisé : les jobs ne démarrent pas (« recent account payments have failed or
your spending limit needs to be increased »). Le propriétaire ne veut ni payer, ni passer le dépôt en public, ni
héberger un runner.

## Décision

- Aucun workflow GitHub Actions. Les checks requis ont été retirés de la protection des branches ; la PR reste
  obligatoire sur `main` et `develop`.
- `scripts/ci.sh` est la CI : `fmt`, `clippy -D warnings`, `test`, `cargo deny`.
- Hooks versionnés dans `.githooks/` (activés par `git config core.hooksPath .githooks`) :
  `pre-push` lance `scripts/ci.sh`, `commit-msg` impose Conventional Commits.
- Release via `scripts/release.sh <x.y.z>` (tag + `gh release create`), plus de workflow sur tag.
- Dependabot est conservé (il ne dépend pas des workflows du dépôt).

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Payer / augmenter la limite | CI serveur | Refusé |
| Runner auto-hébergé | CI sur GitHub | Refusé, le PC doit être allumé |
| Dépôt public | Actions gratuit | Refusé |

## Conséquences

Les checks ne sont pas vérifiés côté serveur : la discipline repose sur les hooks. `git push --no-verify` est
réservé aux urgences. Les anciens workflows restent dans l'historique git (commit initial) si Actions redevient
disponible.
