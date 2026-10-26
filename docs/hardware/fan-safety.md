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
- Bornes des paramètres (S3) : `20 ≤ T0 < T1 ≤ 100` °C, `0 ≤ min ≤ max ≤ 100` %, `1,0 ≤ k ≤ 4,0`.

### Du modèle à la table EC (modèle par paliers mesuré)

La table EC d'un ventilo a **7 vitesses**, **6 seuils de montée** et **6 seuils de descente**
([ec-map.md](ec-map.md)). Le palier `i ≥ 1` est atteint quand `T ≥ up[i−1]` et quitté quand `T < down[i−1]` ;
le palier 0 s'applique sous le premier seuil. Une courbe (`FanCurveSpec` dans `msi-core`) se décrit par :

| Champ | Rôle | Bornes (S3) |
|---|---|---|
| `T0, T1, min, max, k` | courbe exponentielle ci-dessus | voir ci-dessus |
| `idle` | vitesse du palier 0 (0 = ventilo arrêté) | `idle ≤ min` |
| `up[0..6]` | seuils de montée | strictement croissants, 20–100 °C |
| `hysteresis` | `up[i] − down[i]` | 2–10 °C, et `up[0] − hysteresis ≥ 20` |

Génération (`table::generate`) : `speeds[0] = idle`, `speeds[i] = f(up[i−1])` arrondi au %,
`down[i] = up[i] − hysteresis`, puis planchers S2 (ci-dessous). La vitesse appliquée par palier est la valeur
de la courbe **à l'entrée** du palier : la table reste sous la courbe, les planchers garantissent la sécurité.

### Lecture prudente : le palier montant

Comme `down[i] < up[i]`, à une température `T` donnée l'EC peut être sur deux paliers selon qu'il chauffe ou
refroidit. Le **palier montant** (nombre de seuils `up ≤ T`) est le plus bas des deux : c'est lui que S2 contrôle
(`StepCurve::min_speed_at`). En redescente, l'EC reste plus longtemps sur un palier plus rapide : plus sûr.

### Presets (calibrés sur la carte EC mesurée, firmware `.110`)

| Preset (profil) | `idle` | `T0` | `T1` | `min` | `max` | `k` | `up` (°C) | hyst. |
|---|---|---|---|---|---|---|---|---|
| **Silencieux** (Éco) | 0 % | 50 | 82 | 30 % | 100 % | 2,0 | 50 57 64 70 76 82 | 4 °C |
| **Progressif** (Balanced) | 0 % | 42 | 79 | 30 % | 100 % | 1,8 | 42 50 58 65 72 79 | 4 °C |
| **Agressif** (Gaming) | 30 % | 40 | 74 | 40 % | 100 % | 1,4 | 40 47 54 61 68 74 | 3 °C |

Tables générées (testées dans `table::tests::presets_generate_the_documented_tables`), **aucun palier relevé
par S2** :

| Palier | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|---|
| Silencieux vitesse % | 0 | 30 | 33 | 43 | 57 | 76 | 100 |
| Silencieux monte ≥ / redescend < °C | – | 50 / 46 | 57 / 53 | 64 / 60 | 70 / 66 | 76 / 72 | 82 / 78 |
| Progressif vitesse % | 0 | 30 | 34 | 45 | 60 | 78 | 100 |
| Progressif monte ≥ / redescend < °C | – | 42 / 38 | 50 / 46 | 58 / 54 | 65 / 61 | 72 / 68 | 79 / 75 |
| Agressif vitesse % | 30 | 40 | 47 | 57 | 71 | 86 | 100 |
| Agressif monte ≥ / redescend < °C | – | 40 / 37 | 47 / 44 | 54 / 51 | 61 / 58 | 68 / 65 | 74 / 71 |
| *Usine CPU (rappel)* | *0* | *36* | *40* | *50* | *62* | *80* | *96* |
| *Usine monte ≥ / redescend < °C* | – | *57 / 53* | *62 / 57* | *70 / 65* | *77 / 73* | *83 / 80* | *90 / 87* |

Justification :

- **Vitesse active minimale 30 %** : c'est la plus basse valeur vérifiée sur la machine (≈ 2070 tr/min, le
  ventilo tourne). En dessous, rien n'est mesuré (risque de calage) ; et les bas régimes sont proches en
  tr/min (36 % ≈ 2200), donc le silence vient de la **forme** de la courbe, pas de vitesses très basses.
- **Silencieux** (« je veux un mode silencieux ») : ventilo **arrêté sous 50 °C** (redémarre à 50, s'arrête
  sous 46), **démarrage doux à 30 % avant 55 °C** au lieu de 0 % jusqu'à 57 °C puis des sauts tardifs.
  Plus silencieux que l'usine à chaque température jusqu'à 69 °C, sans saut de plus de 24 points entre deux
  paliers. Hystérésis de 4 °C : pas de « pompage » entre deux paliers, source de bruit.
- **Progressif** : démarre dès 42 °C, montée exponentielle régulière, 100 % à 79 °C.
- **Agressif** : jamais arrêté (30 %), réagit tôt, 100 % dès 74 °C ; hystérésis 3 °C pour redescendre vite.
- Tous atteignent **100 % avant 85 °C** (S2), alors que la table d'usine plafonne à 96 % à 90 °C.
- Les mêmes presets s'appliquent aux deux ventilos (CPU et GPU).

## Invariants (non négociables)

| # | Règle | Où |
|---|---|---|
| S1 | Seuils de montée et de descente strictement croissants, `down[i] < up[i]` avec un écart ≥ 2 °C, vitesses croissantes ou égales | `msi-core` + noyau (sauf l'écart de 2 °C) |
| S2 | Vitesse du **palier montant** ≥ **30 %** dès 60 °C, ≥ **50 %** dès 75 °C, **100 %** dès 85 °C (planchers, quelle que soit la courbe). Table générée depuis les paramètres : paliers relevés **explicitement et signalés** (`SafetyReport`). Table saisie palier par palier : **rejetée** | `msi-core` |
| S3 | Toute **entrée** hors bornes (paramètres de courbe, disposition, table brute) est **rejetée**, jamais « corrigée silencieusement » | `msi-core` + noyau |
| S4 | CPU ou GPU ≥ **95 °C** sur **2 lectures consécutives** → Cooler Boost forcé jusqu'au retour **des deux** sous 85 °C sur 2 lectures (hystérésis + filtre anti-pics). Capteur illisible → forcé immédiatement | `msi-core` (machine à états) + `msi-daemon` (application, ~1 Hz) |
| S5 | Arrêt du daemon (normal, signal, panic) → **tables d'usine réécrites** puis EC remis en fan mode `auto` (voir [Restitution à l'EC](#restitution-à-lec-tables-dusine)) | `msi-daemon` (`Controller::shutdown`, hook de panic) + `ExecStopPost=msi-daemon --restore-factory` |
| S6 | Watchdog systemd (`WatchdogSec=15`) : `WATCHDOG=1` après chaque tick de 1 s réussi ; daemon figé → tué, redémarré, et S5 s'applique (`ExecStopPost`) | systemd + `msi-daemon` (`notify`) |
| S7 | Après écriture, relecture de la table EC (et du fan mode / shift mode) ; divergence → tables d'usine + `auto` + `SafetyEvent` | `msi-daemon` |
| S8 | Firmware EC non reconnu, ou attributs de courbe absents (module amont, `firmware=` forcé) → aucune écriture, mode lecture seule | `msi-ec` fork + daemon (liste dans `msi-core`) |
| S9 | Nos tables en vigueur (`advanced`) et un ventilo à **0 %** (et 0 tr/min si connu) alors que la température **filtrée** (minimum des 2 dernières lectures) de son capteur est **≥ 70 °C** pendant **≥ 30 s** → tables d'usine + `auto`, `SafetyEvent("fan_stalled")`, **lecture seule jusqu'au redémarrage** du daemon (`ReadOnly`, `ReadOnlyReason` commençant par `S9`) | `msi-core` (`stall::StallDetector`) + `msi-daemon` (tick 1 Hz) |

Les seuils S2/S4 sont prudents : le Core Ultra 9 185H a un Tjmax de 110 °C, la RTX 4070 Laptop réduit sa fréquence vers 87 °C.

## S2 et S3 : relèvement signalé vs rejet

Les deux règles semblent en conflit (une courbe très silencieuse peut passer sous un plancher alors que S3
interdit de corriger). Résolution :

- **S3 porte sur les entrées** : ce que l'utilisateur fournit (paramètres `T0/T1/min/max/k`, `idle`, seuils,
  hystérésis, table brute d'un futur éditeur de paliers) est validé tel quel ; hors bornes → erreur, rien n'est
  modifié.
- **S2 est une transformation documentée** de la table *générée* : les planchers font partie de la définition de la
  courbe appliquée (ADR-0006, modèle par paliers : ADR-0009). Chaque palier relevé est retourné dans un `SafetyReport` (palier, seuil d'entrée,
  vitesse de la courbe, vitesse appliquée, plancher en cause) que le daemon expose (`SetFanCurve`, `FanCurve`) et
  que l'UI affiche. Rien n'est silencieux. Un plancher `(Tf, Sf)` relève le palier montant à `Tf` et tous les
  suivants (S1 conservé) ; si le premier seuil est au-dessus de 60 °C, c'est le palier 0 (`idle`) qui est relevé.
- Une table brute saisie palier par palier n'est **pas** relevée : si elle viole S1 ou S2, elle est rejetée.

## Filtre anti-pics de S4

La température instantanée de l'EC (`0x68`) a des pics isolés jusqu'à ~90 °C au repos. Chaque capteur est filtré
sur ses **deux dernières lectures** (`supervisor::ThermalSupervisor`) : Cooler Boost n'est forcé que si les deux
sont ≥ 95 °C (minimum glissant) et n'est relâché que si les deux sont < 85 °C (maximum glissant). Un pic isolé
est ignoré, une vraie surchauffe est prise en compte une lecture (~1 s) plus tard. Une lecture en échec force
immédiatement, sans filtre. Après le démarrage ou le retour d'un capteur, une seule lecture ne suffit ni à forcer
ni à relâcher.

## S9 : ventilo arrêté sur puce chaude

Origine : premier essai réel (2026-10-25), ventilo CPU à 0 % à 57–60 °C lus alors que la table demandait 30–45 %.
Les spikes suivants ont montré que c'était le palier 0 normal (l'EC pilote le CPU sur une température lissée,
nettement sous `cpu/realtime_temperature`, et ignore les seuils CPU écrits : [carte EC](ec-map.md)). S9 ne se
base donc **pas** sur les seuils de notre table (il se déclencherait sans cesse) mais sur une température fixe :

- **arrêté** = vitesse temps réel EC 0 % **et**, si le régime hwmon est disponible, 0 tr/min (un ventilo qui
  ralentit encore n'est pas arrêté) ;
- **chaud** = minimum des **deux dernières** lectures ≥ 70 °C : un pic isolé du capteur `0x68` (jusqu'à ~90 °C au
  repos) ne compte jamais ;
- la condition doit durer **30 s** ; une seule lecture « non bloquée » entre deux bloquées est ignorée, deux de
  suite remettent le compteur à zéro ; une lecture illisible n'est pas un blocage (S4 s'en charge) ;
- surveillé seulement quand nos tables sont en vigueur (`advanced` vérifié) ; remis à zéro à chaque application.

Au déclenchement : tables d'usine puis `auto` (le firmware reprend la main, S4 continue de forcer Cooler Boost
si besoin), événement `fan_stalled` (valeur : °C), et le daemon refuse toute écriture EC jusqu'à son redémarrage
pour ne pas ré-appliquer aussitôt. `msictl status` affiche `SAFETY STOP (S9)` avec la raison.

## Restitution à l'EC (tables d'usine)

L'EC ne garde **qu'une copie** des tables, utilisée aussi par `auto` ([carte EC](ec-map.md)) : remettre seulement
`fan_mode=auto` laisserait nos tables actives et « auto » ne serait plus le comportement d'usine. Les tables
d'usine du firmware `14K2EMS1.110` sont des constantes de `msi-core` (`ec::FACTORY_14K2EMS1_110`, issues du spike
phase 2). Restitution (`msi_daemon::ec::restore_factory`) : tables d'usine dans l'ordre sûr
(`table::write_plan_raw`, mêmes garanties que ci-dessous), relecture, **puis** `fan_mode=auto`, tenté même si une
écriture de table a échoué. Idempotente (écritures sans effet sautées). Firmware sans tables d'usine connues
(`.104`–`.109`) : seul `auto` est restauré.

Appliquée à l'arrêt (S5 : signal, sortie normale, hook de panic, `ExecStopPost=msi-daemon --restore-factory`),
sur repli S7 et S9, et à la demande (`RestoreFactory`, `msictl factory`).

## Écriture ordonnée des tables

Le noyau vérifie `down[i] < up[i]` à **chaque** écriture (`EINVAL` sinon). `table::write_plan` ordonne les
écritures : vitesses ← max(actuelles, cibles), descente ← min(actuelle, cible), montée ← cible, descente ← cible,
vitesses ← cible (les écritures sans effet sont sautées). Chaque état intermédiaire est accepté par le noyau et au
moins aussi sûr que l'ancienne ou la nouvelle table (test de propriété). Le noyau refuse aussi une table de
vitesses dont le dernier palier est < 80 % (`RawTables::kernel_check` reproduit toutes ses règles) ; une table
`StepCurve` a toujours 100 % au dernier palier (plancher S2 de 85 °C).
