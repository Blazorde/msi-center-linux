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
| **Silencieux** (Éco) | 0 % | 52 | 88 | 30 % | 100 % | 2,5 | 52 60 67 75 83 85 | 4 °C |
| **Progressif** (Balanced) | 0 % | 48 | 85 | 30 % | 100 % | 2,2 | 48 56 64 71 79 85 | 4 °C |
| **Agressif** (Gaming) | 30 % | 40 | 74 | 40 % | 100 % | 1,4 | 40 47 54 61 68 74 | 3 °C |

Tables générées (testées dans `table::tests::presets_generate_the_documented_tables`). **Un seul palier relevé
par S2** : Silencieux palier 6 (entré à 85 °C), courbe 86 % → **100 %** (plancher 100 % dès 85 °C), car sa
courbe n'atteint 100 % qu'à `T1` = 88 °C. Le relèvement est signalé (`raised`), comme tout relèvement S2.
Progressif et Agressif : aucun palier relevé.

| Palier | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|---|
| Silencieux vitesse % | 0 | 30 | 32 | 38 | 53 | 78 | 100 (S2, courbe 86) |
| Silencieux monte ≥ / redescend < °C | – | 52 / 48 | 60 / 56 | 67 / 63 | 75 / 71 | 83 / 79 | 85 / 81 |
| Progressif vitesse % | 0 | 30 | 32 | 41 | 55 | 77 | 100 |
| Progressif monte ≥ / redescend < °C | – | 48 / 44 | 56 / 52 | 64 / 60 | 71 / 67 | 79 / 75 | 85 / 81 |
| Agressif vitesse % | 30 | 40 | 47 | 57 | 71 | 86 | 100 |
| Agressif monte ≥ / redescend < °C | – | 40 / 37 | 47 / 44 | 54 / 51 | 61 / 58 | 68 / 65 | 74 / 71 |
| *Usine CPU (rappel)* | *0* | *36* | *40* | *50* | *62* | *80* | *96* |
| *Usine monte ≥ / redescend < °C* | – | *57 / 53* | *62 / 57* | *70 / 65* | *77 / 73* | *83 / 80* | *90 / 87* |

Justification :

- **Vitesse active minimale 30 %** : c'est la plus basse valeur vérifiée sur la machine (≈ 2070 tr/min, le
  ventilo tourne). En dessous, rien n'est mesuré (risque de calage) ; et les bas régimes sont proches en
  tr/min (36 % ≈ 2200), donc le silence vient de la **forme** de la courbe, pas de vitesses très basses.
- **Recalage 2026-10-26 (retour sur la machine réelle)** : avec le pilotage hybride, la consigne CPU est
  calculée sur `cpu/realtime_temperature` **filtrée**, qui se tient **5 à 8 °C au-dessus** de la température
  lissée sur laquelle l'EC choisit ses paliers. Avec les anciens presets (Silencieux T0 50 / T1 82 / k 2,0,
  Progressif T0 42 / T1 79 / k 1,8), Progressif au repos (57–61 °C lus) demandait 44–50 % (≈ 2900 tr/min)
  là où l'usine reste à 36 % (≈ 2200 tr/min) : **plus bruyant que l'usine**. Les presets sont donc décalés
  vers le haut (`T0`, `T1`) et plus convexes (`k`) : bas régimes plus longs, montée tardive mais franche.
- **Silencieux** (« je veux un mode silencieux ») : ventilo **arrêté sous 52 °C** filtrés (redémarre à 52,
  s'arrête sous 48), **démarrage doux à 30 %**, ≈ 32 % à 60 °C, 42 % à 70 °C, 67 % à 80 °C, 100 % à 88 °C
  (S2 impose 100 % dès 85 °C sur la table générée, relèvement signalé). Table GPU plus silencieuse que l'usine
  de 57 à 74 °C, sans saut de plus de 25 points entre deux paliers. Hystérésis de 4 °C : pas de « pompage ».
- **Progressif** : arrêté sous 48 °C (s'arrête sous 44), ≈ 36 % à 60 °C (niveau de l'usine au repos), 52 % à
  70 °C, 81 % à 80 °C, 100 % à 85 °C.
- **Agressif** : jamais arrêté (30 %), réagit tôt, 100 % dès 74 °C ; hystérésis 3 °C pour redescendre vite.
- Toutes les tables générées sont à **100 % dès 85 °C** (S2) — par la courbe pour Progressif et Agressif,
  par relèvement signalé pour Silencieux —, alors que la table d'usine plafonne à 96 % à 90 °C.
- Les mêmes presets s'appliquent aux deux ventilos (CPU et GPU). Le **GPU** reçoit la table ci-dessus telle
  quelle. Le **CPU** est piloté en hybride (section suivante) : l'EC ignore les seuils CPU écrits.

### Pilotage hybride du ventilo CPU (ADR-0010)

Mesures ([carte EC](ec-map.md#seuils-cpu-ignorés-par-lec-2026-10-25-premier-essai-du-daemon--2-spikes)) :
l'EC **ignore les seuils CPU écrits** et choisit son palier CPU sur une température **lissée**, nettement sous
`cpu/realtime_temperature` ; il applique en revanche les **vitesses** CPU écrites en direct (< 2 s). On garde
donc les seuils d'usine (montée 57 62 70 77 83 90, descente 53 57 65 73 80 87, jamais réécrits) et le daemon
module **en direct** les vitesses (`msi_core::hybrid`, boucle de 1 s de `msi-daemon`) :

1. **Filtre** (`TempFilter`) : médiane des 5 dernières lectures de `cpu/realtime_temperature` (un pic isolé,
   ou deux d'affilée, ne passe jamais), puis moyenne glissante exponentielle (α = 0,3, ≈ 3 s). Rien avant
   3 lectures ; une lecture illisible est ignorée (S4 s'en charge).
2. **Consigne** (`CpuFanPilot`) : `f(T_filtrée)` de la courbe exponentielle du profil (paramètres `T0, T1,
   min, max, k` ci-dessus ; les seuils `up` de la courbe ne servent plus qu'au GPU).
   - **Repos** : sous `T0 − hystérésis` la consigne est `idle` (0 % permis) ; le ventilo ne redémarre qu'à
     `T0`. Au redémarrage, il part de la vitesse de départ `min` (≥ 30 % pour les presets : plus basse vitesse
     vérifiée qui tourne, ≈ 2070 tr/min).
   - **Bande morte** : un écart < 3 % n'est appliqué qu'après 20 s (une montée à `max` n'attend jamais) : pas
     de « pompage » sur une température qui oscille.
   - **Pente** : au plus **+5 %/s** en montée (30 → 100 % en 14 s), **−1 %/s** en descente (redescente lente et
     silencieuse). Seuls sauts : l'arrêt (vers `idle`) et le démarrage (vers `min`).
   - Avant la première lecture filtrée : `min` (le ventilo tourne doucement 3 s au démarrage du daemon).
3. **Vecteur écrit** (`hybrid_cpu_table`) : paliers **0–3 = consigne** (quel que soit le palier 0–3 où l'EC se
   trouve, le ventilo suit notre courbe), paliers **4–6 ≥ max(consigne, f(seuil d'entrée), 60 / 80 / 100 %)**,
   puis les planchers **S2** sur le palier montant des seuils d'usine : ≥ 30 % aux paliers 1–2 (60 °C),
   ≥ 50 % au palier 3 (75 °C), **100 % au palier 5** (85 °C tombe entre 83 et 90 °C) et au palier 6. Chaque
   relèvement est signalé (`raised`). Le vecteur passe `StepCurve::new` (S1–S3, S2, règles noyau) avant toute
   écriture ; sinon rien n'est écrit.
4. **Écriture** : seulement `cpu/fan_curve_speeds`, seulement quand le vecteur change, seulement si la courbe
   est appliquée et vérifiée, l'EC en `advanced` et les seuils CPU toujours ceux d'usine (sinon le contrôle de
   dérive de 10 s ré-applique). Relecture (S7) : divergence ou échec → tables d'usine + `auto` + `SafetyEvent`.
   Jamais après `RestoreFactory`, un repli S7 ou S9, ni en lecture seule (S8).

Comportement obtenu (consigne des paliers 0–3 selon la température CPU **filtrée**, régime établi) :

| °C filtrés | 40 | 45 | 50 | 55 | 60 | 65 | 70 | 75 | 80 | ≥ T1 | Paliers 4 / 5 / 6 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Silencieux** (T0 52, arrêt < 48, T1 88) | 0 | 0 | 0* | 30 | 32 | 35 | 42 | 53 | 67 | 100 | 60 / 100 / 100 |
| **Progressif** (T0 48, arrêt < 44, T1 85) | 0 | 0* | 30 | 32 | 36 | 43 | 52 | 65 | 81 | 100 | 71 / 100 / 100 |
| **Agressif** (T0 40, repos 30 %, T1 74) | 40 | 44 | 51 | 59 | 69 | 79 | 90 | 100 | 100 | 100 | 100 / 100 / 100 |
| *Usine (rappel, température lissée de l'EC)* | *0* | *0* | *0* | *0* | *36* | *40* | *50* | *50* | *62* | | *62 / 80 / 96* |

\* 30 % si le ventilo tournait déjà (hystérésis : arrêt sous 48 °C pour Silencieux, 44 °C pour Progressif).
Les paliers 1–3 sont en plus relevés à 30 / 30 / 50 % au minimum (S2) : l'EC n'y entre que vers 57 / 62 /
70 °C lissés. Comme la température filtrée se tient 5 à 8 °C au-dessus de la lissée, l'EC au repos (palier
1, ~57–61 °C lissés) correspond à ~62–69 °C filtrés : Silencieux y demande 33–41 % et Progressif 38–50 %,
contre 40–55 % et 53–70 % avec les anciens presets. Silencieux reste **arrêté sous 48–52 °C**, démarre **doucement à
30 %**, monte de façon exponentielle et lente, et atteint 100 % à 88 °C (paliers chauds : ≥ 60 % dès 77 °C
lissés, 100 % dès 83 °C, inchangé).

Argument de sécurité :

- Les paliers chauds 4–6 sont **toujours** sûrs dans l'EC, même si le daemon se fige : ≥ 60 % dès 77 °C
  lissés, 100 % dès 83 °C ; et le vecteur respecte S2 **statiquement** sur les seuils d'usine. Seul le palier
  0 (EC lissé < 57 °C) peut être à `idle`.
- Daemon figé : watchdog (S6) → tué, `ExecStopPost` restaure les tables d'usine (S5). Daemon tué : hook de
  panic / `ExecStopPost` (S5). S4 (Cooler Boost à 95 °C) et S9 (ventilo arrêté à 70 °C) inchangés.
- La consigne dépend de la vraie température (filtrée, pas lissée par l'EC) : elle monte **avant** que l'EC
  change de palier.
- Quelques écritures EC par minute au plus en régime établi (une par seconde pendant une rampe).
- Firmware sans tables d'usine connues (`.104`–`.109`) : pas de pilotage hybride, la table CPU générée est
  écrite comme pour le GPU (comportement précédent).

## Profils de ventilation

Demande du propriétaire (2026-10-26) : « des profils de courbes, genre silencieux, auto, courbes que tu fais toi
et turbo ». Chaque profil de performance (Éco, Équilibré, Gaming) a un **profil de ventilation** ; par défaut
celui de la colonne « défaut », que le propriétaire peut changer. Le choix est **retenu par profil de
performance** (`state.toml`, `[fan_profiles]`) et ré-appliqué à chaque changement de profil, au démarrage et
après la veille. Le shift mode et le dGPU restent ceux du profil de performance. Code :
`msi_core::fan_profile::FanProfile`, appliqué par `msi-daemon` (`Controller::apply_profile`), [ADR-0012](../adr/0012-fan-profiles.md).

| Profil de ventilation | Id | Courbe | Défaut pour |
|---|---|---|---|
| **Silencieux** | `silent` | preset Silencieux | Éco |
| **Auto** | `auto` | **courbe d'usine MSI**, gérée par le contrôleur (EC) | – |
| **Équilibré** | `balanced` | preset Progressif | Équilibré |
| **Turbo** | `turbo` | preset Agressif (monte tôt et fort, 100 % dès 74 °C, redescend) | Gaming |
| **Personnalisé** | `custom` | courbe du propriétaire, une par ventilo et par profil de performance | – |

- **Turbo n'est pas Cooler Boost** : Cooler Boost (ventilos à fond) reste un interrupteur séparé.
- **Silencieux / Équilibré / Turbo / Personnalisé** : comportement décrit ci-dessus (GPU : table complète écrite ;
  CPU : pilotage hybride ADR-0010 avec la courbe du profil de ventilation). S1–S9 inchangés.
- **Personnalisé** : la courbe saisie est validée telle quelle (S3, rejet sans rien écrire), la table générée
  applique les planchers S2 avec relèvements **signalés** (`raised`), comme toute courbe. Un ventilo jamais
  édité suit le preset du profil de ventilation par défaut. Éditer une courbe passe le profil en Personnalisé ;
  la courbe est conservée quand on choisit un autre profil de ventilation.
- **Auto** : tables d'usine des deux ventilos (ordre sûr `write_plan_raw`, relues) puis `fan_mode=auto`, puis
  shift mode, puis relecture de tout (S7 : divergence → tables d'usine + `auto` + `SafetyEvent`, ce qui est
  déjà l'état visé). Le pilotage hybride CPU est **arrêté** (aucune écriture en direct). C'est le même état EC
  que `RestoreFactory`, mais **choisi et persisté** : le contrôle de dérive (10 s) et la sortie de veille le
  ré-appliquent. **S4** (Cooler Boost forcé à 95 °C) reste actif ; **S9** n'est pas surveillé (il ne porte que
  sur nos tables, ici c'est la courbe du firmware) ; **S5** : l'arrêt restaure les tables d'usine comme
  toujours ; **S8** : refusé en lecture seule, comme toute écriture. Firmware sans tables d'usine connues :
  seul `fan_mode=auto` est écrit.
- Migration : un `state.toml` version 1 n'a pas de `[fan_profiles]` ; un profil qui y avait une courbe
  personnalisée passe en Personnalisé (comportement identique), les autres gardent leur défaut.

## Invariants (non négociables)

| # | Règle | Où |
|---|---|---|
| S1 | Seuils de montée et de descente strictement croissants, `down[i] < up[i]` avec un écart ≥ 2 °C, vitesses croissantes ou égales | `msi-core` + noyau (sauf l'écart de 2 °C) |
| S2 | Vitesse du **palier montant** ≥ **30 %** dès 60 °C, ≥ **50 %** dès 75 °C, **100 %** dès 85 °C (planchers, quelle que soit la courbe). Table générée depuis les paramètres : paliers relevés **explicitement et signalés** (`SafetyReport`). Table saisie palier par palier : **rejetée** | `msi-core` |
| S3 | Toute **entrée** hors bornes (paramètres de courbe, disposition, table brute) est **rejetée**, jamais « corrigée silencieusement » | `msi-core` + noyau |
| S4 | CPU ou GPU ≥ **95 °C** sur **2 lectures consécutives** → Cooler Boost forcé jusqu'au retour **des deux** sous 85 °C sur 2 lectures (hystérésis + filtre anti-pics). Capteur illisible → forcé immédiatement | `msi-core` (machine à états) + `msi-daemon` (application, ~1 Hz) |
| S5 | Arrêt du daemon (normal, signal, panic) → **tables d'usine réécrites** puis EC remis en fan mode `auto` (voir [Restitution à l'EC](#restitution-à-lec-tables-dusine)) | `msi-daemon` (`Controller::shutdown`, hook de panic) + `ExecStopPost=msi-daemon --restore-factory` |
| S6 | Watchdog systemd (`WatchdogSec=15`) : `WATCHDOG=1` après chaque tick de 1 s réussi ; daemon figé → tué, redémarré, et S5 s'applique (`ExecStopPost`) | systemd + `msi-daemon` (`notify`) |
| S7 | Après écriture, relecture de la table EC (et du fan mode / shift mode) ; divergence → tables d'usine + `auto` + `SafetyEvent`. Vaut aussi pour chaque écriture en direct des vitesses CPU (ADR-0010) | `msi-daemon` |
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
