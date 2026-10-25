## Quoi

<!-- Résumé en 1–3 phrases. Titre de PR au format Conventional Commits : feat(daemon): … -->

## Pourquoi

<!-- Lien vers la phase du PRD / l'issue : Refs # -->

## Comment tester

- [ ] `scripts/ci.sh` vert (fmt, clippy, tests, cargo deny)
- [ ] Test manuel (décrire) :

## Checklist

- [ ] Docs mises à jour (`docs/`, `CLAUDE.md` si la structure change)
- [ ] `CHANGELOG.md` → `[Unreleased]` si visible pour l'utilisateur
- [ ] ADR ajouté si décision d'architecture
- [ ] **Touche à l'EC / aux ventilos ?** Invariants de `docs/hardware/fan-safety.md` respectés et testés
