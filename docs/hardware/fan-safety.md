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
- `msi-core` échantillonne cette fonction sur les N points de la table EC (arrondi au % le plus proche), puis
  applique les planchers S2 (voir plus bas).
- **Disposition provisoire** (`PROVISIONAL_LAYOUT_TEMPS`) : 7 points à 40, 50, 60, 70, 75, 80, 85 °C.
  TODO : à remplacer par le nombre de points et les seuils réels une fois `docs/hardware/ec-map.md` rédigé (spike).
- Bornes des paramètres (S3) : `20 ≤ T0 < T1 ≤ 100` °C, `0 ≤ min ≤ max ≤ 100` %, `1,0 ≤ k ≤ 4,0`.
- Presets (indicatifs, à calibrer après le spike) :

| Preset | T0 | T1 | min | max | k |
|---|---|---|---|---|---|
| Silencieuse (Éco) | 50 °C | 90 °C | 20 % | 100 % | 2,5 |
| Progressive (Balanced) | 45 °C | 87 °C | 25 % | 100 % | 2,0 |
| Agressive (Gaming) | 40 °C | 82 °C | 35 % | 100 % | 1,5 |

Sur la disposition provisoire, la Silencieuse donne ~22,5 % à 60 °C, 45 % à 75 °C et 77 % à 85 °C : les trois
points sont relevés par S2 (30 / 50 / 100 %) et le relèvement est signalé. La Progressive et l'Agressive
respectent déjà les planchers.

## Invariants (non négociables)

| # | Règle | Où |
|---|---|---|
| S1 | Les températures de la table sont strictement croissantes, les vitesses croissantes ou égales | `msi-core` |
| S2 | Vitesse ≥ **30 %** dès 60 °C, ≥ **50 %** dès 75 °C, **100 %** dès 85 °C (planchers, quelle que soit la courbe). Table générée depuis les paramètres : points relevés **explicitement et signalés** (`SafetyReport`). Table saisie point par point : **rejetée** | `msi-core` |
| S3 | Toute **entrée** hors bornes (paramètres de courbe, disposition, table brute) est **rejetée**, jamais « corrigée silencieusement » | `msi-core` + noyau |
| S4 | CPU ou GPU ≥ **95 °C** → Cooler Boost forcé jusqu'au retour **des deux** sous 85 °C (hystérésis). Capteur illisible → forcé | `msi-core` (machine à états) + `msi-daemon` (application) |
| S5 | Arrêt du daemon (normal, signal, panic) → EC remis en fan mode `auto` | `msi-daemon` + `ExecStopPost` systemd |
| S6 | Watchdog systemd (`WatchdogSec`) : daemon figé → redémarré, et S5 s'applique | systemd |
| S7 | Après écriture, relecture de la table EC ; divergence → retour `auto` + `SafetyEvent` | `msi-daemon` |
| S8 | Firmware EC non reconnu → aucune écriture, mode lecture seule | `msi-ec` fork + daemon (liste dans `msi-core`) |

Les seuils S2/S4 sont prudents : le Core Ultra 9 185H a un Tjmax de 110 °C, la RTX 4070 Laptop réduit sa fréquence vers 87 °C.

## S2 et S3 : relèvement signalé vs rejet

Les deux règles semblent en conflit (la Silencieuse passe sous le plancher de 60 °C alors que S3 interdit de
corriger). Résolution :

- **S3 porte sur les entrées** : ce que l'utilisateur fournit (paramètres `T0/T1/min/max/k`, table brute de l'éditeur
  de points, disposition) est validé tel quel ; hors bornes → erreur, rien n'est modifié.
- **S2 est une transformation documentée** de la table *générée* : les planchers font partie de la définition de la
  courbe appliquée (ADR-0006 : « échantillonne … puis applique les planchers »). Chaque point relevé est retourné
  dans un `SafetyReport` (point, vitesse de la courbe, vitesse appliquée, plancher en cause) que le daemon expose
  et que l'UI affiche. Rien n'est silencieux.
- Une table brute saisie point par point n'est **pas** relevée : si elle viole S1 ou S2, elle est rejetée.

## Lecture des tables

Le comportement de l'EC entre deux points (paliers ou interpolation) n'est pas encore connu. Les contrôles S2
utilisent la lecture **en paliers** : la vitesse en vigueur à `T` est celle du dernier point `≤ T`. C'est la borne
basse des deux comportements, donc la plus prudente. La vitesse sous le premier point étant inconnue elle aussi,
le premier point doit être `≤ 60 °C` (plancher le plus bas), sinon la disposition ou la table est rejetée.
