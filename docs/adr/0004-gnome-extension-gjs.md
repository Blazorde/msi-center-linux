# 0004 — Extension GNOME en GJS, minimale

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Afficher du texte dans la top bar et lire le presse-papiers sous Wayland n'est possible que depuis GNOME Shell.
GNOME Shell ne charge que des extensions JavaScript (GJS, modules ES).

## Décision

Une extension `msi-center@blazorde` qui ne contient **aucune logique métier** : abonnement aux signaux D-Bus,
affichage des métriques choisies (GSettings), menu de profils, capture du presse-papiers → `clipd`, popup `Super+V`.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Icône AppIndicator depuis Rust | 100 % Rust | Pas de texte dans la barre, pas d'accès presse-papiers Wayland |
| `wl-paste --watch` | Hors Shell | GNOME n'implémente pas `wlr-data-control` |

## Conséquences

Lint JS en CI. `shell-version` à mettre à jour à chaque version majeure de GNOME.
