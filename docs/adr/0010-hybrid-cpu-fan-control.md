# 0010 — Pilotage hybride du ventilo CPU

- **Statut** : Accepté (2026-10-26)
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

## Mise en œuvre (2026-10-26)

- `msi_core::hybrid` : `TempFilter` (médiane de 5 + moyenne exponentielle α 0,3), `CpuFanPilot` (repos sous
  `T0 − hystérésis`, redémarrage à `T0` depuis `min`, bande morte 3 % / 20 s, pente +5 %/s et −1 %/s),
  `hybrid_cpu_table` (paliers 0–3 = consigne, 4–6 ≥ max(courbe, 60 / 80 / 100 %), puis S2 sur les seuils
  d'usine : le palier 5 est donc à 100 %, 85 °C tombant entre 83 et 90 °C). Tests de propriété : tout vecteur
  passe `StepCurve::new` (S1–S3, S2, noyau) et la consigne ne bouge que par petits pas.
- `msi-daemon` : la boucle de 1 s filtre la température CPU et calcule la consigne ; elle n'écrit que
  `cpu/fan_curve_speeds`, seulement au changement, courbe appliquée, EC en `advanced` avec les seuils d'usine,
  puis relit (S7). Le contrôle de dérive compare au dernier vecteur écrit. Rien n'est écrit après
  `RestoreFactory`, S7, S9 ni en lecture seule.
- API (compatible) : `FanCurve("cpu")` renvoie le vecteur en vigueur ; clé `cpu_fan_target_pct` dans
  `Telemetry`. Comportement par preset : [fan-safety.md](../hardware/fan-safety.md#pilotage-hybride-du-ventilo-cpu-adr-0010).
- Firmware sans tables d'usine connues : pas de mode hybride (table CPU générée comme avant).
- Essai propriétaire : `scripts/spike/hybrid-cpu-test.sh` (60 s supervisées, daemon arrêté).

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Tables CPU fixes (vitesses seulement, seuils usine) | Simple, EC autonome | Pas de démarrage précoce : 0 % jusqu'à ~57 °C lissés |
| Boucle logicielle sur tous les paliers (table plate) | Contrôle total | Si le daemon meurt, ventilo figé bas quand ça chauffe (rejeté par l'ADR-0006) |
| Chercher l'adresse réelle des seuils CPU | Solution « propre » | Inconnue, coûteuse, peut ne pas exister en RAM |

## Conséquences

Quelques écritures EC par minute au plus (seulement aux changements). Le preset « Silencieux » CPU devient :
0 % au repos, démarrage doux piloté par le daemon, paliers chauds d'usine renforcés.
