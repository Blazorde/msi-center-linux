# Sécurité ventilateurs & thermique

> Document **normatif** : toute modification du code ventilateurs/EC doit respecter ces règles, et chaque règle
> doit être couverte par un test dans `msi-core`.

## Pourquoi pas de modification du firmware

Reflasher le firmware EC peut rendre la machine inutilisable (brick) et aucun firmware MSI n'est documenté.
MSI Center sous Windows n'en a pas besoin : il écrit simplement des registres en RAM de l'EC. On fait pareil, via
le module noyau `msi-ec`. Le firmware conserve ses propres protections (arrêt thermique d'urgence).

## Le problème actuel

Comportement « tout ou rien » : ventilo arrêté → la température monte → ventilo à fond. Bruyant et mauvais pour
les composants (cycles thermiques).

## La courbe exponentielle

```
          ⎧ min                                   si T ≤ T0
speed(T) =⎨ min + (max − min) · ((T − T0)/(T1 − T0))^k   si T0 < T < T1
          ⎩ max                                   si T ≥ T1
```

- `k > 1` : très doux à basse température, accélère fort quand ça chauffe (ce qui a été demandé).
- Le daemon échantillonne cette fonction sur les N points de la table EC (N à confirmer, probablement 6–7).
- Presets (indicatifs, à calibrer après le spike) :

| Preset | T0 | T1 | min | max | k |
|---|---|---|---|---|---|
| Silencieuse (Éco) | 50 °C | 90 °C | 20 % | 100 % | 2,5 |
| Progressive (Balanced) | 45 °C | 87 °C | 25 % | 100 % | 2,0 |
| Agressive (Gaming) | 40 °C | 82 °C | 35 % | 100 % | 1,5 |

## Invariants (non négociables)

| # | Règle | Où |
|---|---|---|
| S1 | Les températures de la table sont strictement croissantes, les vitesses croissantes ou égales | `msi-core` |
| S2 | Vitesse ≥ **30 %** dès 60 °C, ≥ **50 %** dès 75 °C, **100 %** dès 85 °C (planchers, quelle que soit la courbe) | `msi-core` |
| S3 | Toute valeur hors bornes est **rejetée**, jamais « corrigée silencieusement » | `msi-core` + noyau |
| S4 | CPU ou GPU ≥ **95 °C** → Cooler Boost forcé jusqu'au retour sous 85 °C (hystérésis) | `msi-daemon` |
| S5 | Arrêt du daemon (normal, signal, panic) → EC remis en fan mode `auto` | `msi-daemon` + `ExecStopPost` systemd |
| S6 | Watchdog systemd (`WatchdogSec`) : daemon figé → redémarré, et S5 s'applique | systemd |
| S7 | Après écriture, relecture de la table EC ; divergence → retour `auto` + `SafetyEvent` | `msi-daemon` |
| S8 | Firmware EC non reconnu → aucune écriture, mode lecture seule | `msi-ec` fork + daemon |

Les seuils S2/S4 sont prudents : le Core Ultra 9 185H a un Tjmax de 110 °C, la RTX 4070 Laptop réduit sa fréquence vers 87 °C.
