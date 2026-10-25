# 0001 — Monorepo Rust (workspace Cargo)

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Deux apps (MSI Center, presse-papiers) partagent l'outillage, la CI et l'app GTK. Le propriétaire code en Rust et
veut des performances maximales.

## Décision

Un seul dépôt, workspace Cargo : `msi-core`, `msi-daemon`, `clip-core`, `clipd`, `msi-center`. Version unique,
dépendances centralisées dans `[workspace.dependencies]`, lints communs dans `[workspace.lints]`.
Les seules exceptions au Rust sont imposées par la plateforme : extension GNOME (GJS) et module noyau (C).

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Deux dépôts | Isolation | CI et app GTK dupliquées |
| Tauri / Electron | UI web | RAM ×5–10, look non-GNOME |

## Conséquences

Une PR peut toucher backend + frontend atomiquement. La CI compile tout le workspace.
