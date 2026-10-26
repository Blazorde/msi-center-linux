# 0012 — Profils de ventilation (Silencieux, Auto, Équilibré, Turbo, Personnalisé)

- **Statut** : Proposé
- **Date** : 2026-10-26
- Complète [0009](0009-step-curve-and-daemon-runtime.md) et [0010](0010-hybrid-cpu-fan-control.md).

## Contexte

Demande du propriétaire : « des profils de courbes, genre silencieux, auto, courbes que tu fais toi et turbo ».
Jusqu'ici la courbe découlait du profil de performance (Éco → Silencieux, Équilibré → Progressif, Gaming →
Agressif), avec une courbe personnalisée par profil et par ventilo qui remplaçait le preset. Décisions du
propriétaire : Turbo = la courbe Agressive (Cooler Boost reste séparé) ; chaque profil a un profil de
ventilation par défaut, modifiable, et le choix est retenu par profil.

## Décision

1. `msi_core::fan_profile::FanProfile` : `silent | auto | balanced | turbo | custom` → preset Silencieux /
   courbe d'usine / Progressif / Agressif / courbe du propriétaire. `Profile::default_fan_profile()`.
2. Le daemon retient le choix **par profil de performance** (`[fan_profiles]` de `state.toml` v2, seulement
   les choix différents du défaut). Les courbes personnalisées restent stockées par (profil, ventilo) : aucune
   perte à la migration ; un fichier v1 qui avait une courbe personnalisée passe en `custom`.
3. **Auto** = tables d'usine + `fan_mode=auto` + shift mode, relus (S7), pilotage hybride arrêté. Même état EC
   que `RestoreFactory`, mais **choisi et persisté** : ré-appliqué au démarrage, après la veille et par le
   contrôle de dérive (qui compare alors aux tables d'usine en `auto`). S4 actif ; S9 ne surveille que nos
   tables.
4. API D-Bus additive : `SetFanProfile`, `CustomFanCurve`, propriétés `FanProfile`, `DefaultFanProfile`.
   `SetFanCurve` garde sa signature et devient l'édition du profil Personnalisé (il y bascule) ; un ancien
   client obtient le même effet qu'avant.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Une seule courbe personnalisée globale par ventilo | modèle plus simple | migration avec perte si plusieurs profils avaient une courbe ; casse la sémantique « par profil » de `SetFanCurve` |
| Auto = `RestoreFactory` (non persisté) | rien de nouveau dans le daemon | perdu au redémarrage et à la sortie de veille : ce n'est pas un choix |
| Nouvelle méthode `SetCustomFanCurve` | séparation nette | doublon de `SetFanCurve`, deux façons de faire la même chose |
| Profil de ventilation global (indépendant du profil) | un réglage de moins | contraire à la décision du propriétaire (lié mais modifiable, retenu par profil) |

## Conséquences

- Aucun changement incompatible (interface `…1` conservée). Ancienne app/extension : fonctionnent, sans voir le
  profil de ventilation ; nouvelle app/extension face à un ancien daemon : sélecteur masqué.
- En Auto, la courbe d'usine plafonne à 96 % et ne respecte pas les planchers S2 : c'est le comportement
  constructeur, choisi explicitement ; S4 (Cooler Boost à 95 °C) reste la protection.
