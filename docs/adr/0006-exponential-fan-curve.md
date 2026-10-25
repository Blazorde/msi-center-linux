# 0006 — Courbe ventilo exponentielle échantillonnée + planchers

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Demande : silencieux à froid, montée exponentielle avec la chaleur, sans danger pour les composants.

## Décision

L'utilisateur règle 5 paramètres (`T0`, `T1`, `min`, `max`, `k`) ; `msi-core` les échantillonne sur les N points de la
table EC, puis applique les planchers de sécurité. C'est l'EC qui régule en temps réel (pas de boucle logicielle).
Détails et invariants : [fan-safety.md](../hardware/fan-safety.md).

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Points libres uniquement | Contrôle total | Facile de créer une courbe dangereuse |
| Boucle logicielle (le daemon pilote la vitesse en direct) | Précision | Si le daemon meurt, le ventilo reste bloqué |

## Conséquences

L'éditeur de points libres reste possible plus tard (Could), soumis aux mêmes invariants.
