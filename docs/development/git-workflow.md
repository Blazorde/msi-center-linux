# Workflow git, branches, CI, releases

## Branches

```mermaid
gitGraph
  commit id: "init"
  branch develop
  commit id: "ci"
  branch feat/daemon-fan-curve
  commit id: "feat"
  checkout develop
  merge feat/daemon-fan-curve tag: "squash"
  branch release/0.1.0
  commit id: "bump"
  checkout main
  merge release/0.1.0 tag: "v0.1.0"
  checkout develop
  merge main
```

| Branche | Rôle | Protégée | Merge |
|---|---|---|---|
| `main` | Versions publiées uniquement, chaque commit = un tag | ✅ | merge commit depuis `release/*` ou `hotfix/*` |
| `develop` | Intégration, toujours verte | ✅ | squash depuis les branches de travail |
| `feat/*`, `fix/*`, `docs/*`, `chore/*`, `refactor/*`, `test/*`, `perf/*`, `ci/*` | Travail, vie courte | – | → `develop` |
| `release/x.y.z` | Gel, bump version, changelog | – | → `main`, puis `main` → `develop` |
| `hotfix/x.y.z` | Correctif urgent depuis `main` | – | → `main`, puis `main` → `develop` |

Nommage : `feat/<scope>-<description-courte>`, ex. `feat/clipd-tags`, `fix/daemon-restore-auto`.

## Commits — Conventional Commits

```
<type>(<scope>): <résumé à l'impératif, ≤ 72 car.>

<corps optionnel : pourquoi>

<footer optionnel : BREAKING CHANGE:, Refs #12>
```

Types : `feat` `fix` `docs` `refactor` `test` `perf` `chore` `ci` `build`.
Scopes : `core` `daemon` `clip` `clipd` `app` `ext` `kernel` `pkg` `ci` `docs`.
Le hook `commit-msg` refuse tout message non conforme. Le titre de PR doit aussi respecter ce format, car il
devient le message du squash.

## Pull requests

- Template obligatoire (`.github/pull_request_template.md`).
- `scripts/ci.sh` vert avant d'ouvrir la PR (le hook `pre-push` le garantit).
- Branche à jour avec `develop` avant merge.

## CI — locale (pas de GitHub Actions)

Le quota GitHub Actions n'est pas disponible : la CI tourne sur la machine de dev ([ADR-0007](../adr/0007-local-ci.md)).

| Quoi | Quand | Fait |
|---|---|---|
| `scripts/ci.sh` | à la main, et via le hook `pre-push` | `fmt`, `clippy -D warnings`, `test`, `cargo deny` (licences/vulnérabilités) |
| `.githooks/commit-msg` | à chaque commit | Conventional Commits, ≤ 72 caractères |
| `scripts/package.sh` | avant une release, ou pour installer | paquets Arch dans `dist/` (makepkg en dossier temporaire, n'installe rien) |
| `scripts/release.sh x.y.z` | release depuis `main` | CI, build release, tag, release GitHub avec notes du CHANGELOG et paquets `dist/` |
| `scripts/wiki-sync.sh` | après un merge qui touche `docs/` | régénère le [wiki](https://github.com/Blazorde/msi-center-linux/wiki) depuis `docs/` commité, commit + push (`--dry-run` : aperçu) |
| Dependabot | hebdo | MAJ cargo → PR sur `develop` |

Activer les hooks (une fois par clone) : `git config core.hooksPath .githooks`.
`git push --no-verify` : urgences uniquement.

## Versionnage

[SemVer](https://semver.org/lang/fr/). Version unique pour tout le workspace (`[workspace.package] version`).
`CHANGELOG.md` au format [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/).

## Faire une release

Checklist complète de la première : [release-checklist.md](../product/release-checklist.md).

```bash
# 1. Branche de release depuis develop
git switch develop && git pull
git switch -c release/0.1.0

# 2. Bump de version partout (même numéro)
#    Cargo.toml                                    [workspace.package] version = "0.1.0"   (puis cargo check → Cargo.lock)
#    extension/msi-center@blazorde/metadata.json   "version-name": "0.1.0"
#    packaging/arch/PKGBUILD                       pkgver=0.1.0, pkgrel=1
#    CHANGELOG.md                                  [Unreleased] → ## [0.1.0] - AAAA-MM-JJ (+ [Unreleased] vide)
cargo check --workspace && scripts/ci.sh
git commit -am "chore(release): 0.1.0"
git push -u origin release/0.1.0
gh pr create --base main --title "chore(release): 0.1.0"

# 3. Merge dans main en merge commit (pas de squash), puis depuis main à jour :
git switch main && git pull
scripts/package.sh                # dist/msi-center-linux-0.1.0-1-x86_64.pkg.tar.zst + msi-ec-blazorde-dkms-0.1.0-1-any…
scripts/release.sh 0.1.0          # vérifie les versions, CI, build, tag v0.1.0, release GitHub
                                  # (archive des binaires + paquets dist/*-0.1.0-*.pkg.tar.zst joints)

# 4. Back-merge dans develop
git switch develop && git pull && git merge main && git push   # via PR main → develop si le push direct est refusé
```

`scripts/release.sh` refuse de publier si `Cargo.toml`, `metadata.json`, le `PKGBUILD` ou le `CHANGELOG.md` ne
sont pas à la version demandée, et demande confirmation s'il ne trouve aucun paquet dans `dist/`.

## Protection des branches (en place)

`main` et `develop` : PR obligatoire (0 approbation, revues obsolètes rejetées), discussions résolues avant merge,
pas de force-push, pas de suppression. Pas de checks requis côté serveur (CI locale).
Squash et merge commit autorisés, rebase désactivé, branche supprimée après merge.
