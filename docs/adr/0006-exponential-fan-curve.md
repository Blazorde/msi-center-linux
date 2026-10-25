# 0006 — Courbe ventilo exponentielle échantillonnée + planchers

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Demande : silencieux à froid, montée exponentielle avec la chaleur, sans danger pour les composants.

## Décision

L'utilisateur règle 5 paramètres (`T0`, `T1`, `min`, `max`, `k`) plus la vitesse de repos, les 6 seuils de montée et
l'hystérésis ; `msi-core` échantillonne la courbe à l'entrée de chacun des 7 paliers de la table EC (modèle mesuré :
7 vitesses, 6 seuils de montée, 6 de descente), puis applique les planchers de sécurité sur le palier montant. C'est l'EC qui régule en temps réel (pas de boucle logicielle).
Détails et invariants : [fan-safety.md](../hardware/fan-safety.md).

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Points libres uniquement | Contrôle total | Facile de créer une courbe dangereuse |
| Boucle logicielle (le daemon pilote la vitesse en direct) | Précision | Si le daemon meurt, le ventilo reste bloqué |

## Conséquences

L'éditeur de points libres reste possible plus tard (Could), soumis aux mêmes invariants.

**Mise à jour du 2026-10-25** : la table EC réelle fonctionne par paliers avec hystérésis
([ec-map.md](../hardware/ec-map.md)). La disposition provisoire de 7 points est remplacée par le modèle
`StepCurve` ; les presets sont recalibrés (mode Silencieux demandé par le propriétaire).
