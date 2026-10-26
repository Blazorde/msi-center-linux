# 0010 — Pilotage hybride du ventilo CPU

- **Statut** : Proposé
- **Date** : 2026-10-25
- Complète [0006](0006-exponential-fan-curve.md) et [0009](0009-step-curve-and-daemon-runtime.md) pour le ventilo CPU.

## Contexte

Mesures réelles ([ec-map.md](../hardware/ec-map.md#seuils-cpu-ignorés-par-lec-2026-10-25-premier-essai-du-daemon--2-spikes)) :
l'EC ignore les seuils CPU écrits, applique les vitesses CPU en direct, et décide du palier CPU sur une
température lissée. On ne peut donc pas déplacer les paliers CPU : la courbe « démarrage doux dès 45–50 °C »
de l'ADR-0006 est impossible côté CPU en écrivant seulement des tables. Le GPU, lui, fonctionne comme prévu.

## Décision

- **GPU** : inchangé (tables complètes écrites, l'EC régule seul).
- **CPU** : les seuils restent ceux d'usine. Les paliers chauds 4–6 gardent des vitesses ≥ 60 / 80 / 100 % en
  permanence. Le daemon module **en direct** (≈ 1 Hz, seulement si la valeur change) les vitesses des paliers
  0–3 à partir de sa courbe exponentielle évaluée sur une température filtrée (moyenne glissante, pics ignorés).
  Il écrit uniquement `cpu/fan_curve_speeds`, toujours validé par `msi-core` (S1–S3) et par le noyau.
- **Sécurité si le daemon meurt** : les paliers 4–6 restent sûrs dans l'EC ; `ExecStopPost` + panic hook
  restaurent les tables d'usine ; watchdog systemd (S6) ; S4/S9 inchangés.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Tables CPU fixes (vitesses seulement, seuils usine) | Simple, EC autonome | Pas de démarrage précoce : 0 % jusqu'à ~57 °C lissés |
| Boucle logicielle sur tous les paliers (table plate) | Contrôle total | Si le daemon meurt, ventilo figé bas quand ça chauffe (rejeté par l'ADR-0006) |
| Chercher l'adresse réelle des seuils CPU | Solution « propre » | Inconnue, coûteuse, peut ne pas exister en RAM |

## Conséquences

Quelques écritures EC par minute au plus (seulement aux changements). Le preset « Silencieux » CPU devient :
0 % au repos, démarrage doux piloté par le daemon, paliers chauds d'usine renforcés.
