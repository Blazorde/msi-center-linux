# Carte EC — firmware `14K2EMS1.110`

> Résultat du spike phase 2 (2026-10-25), **lecture seule** via `msi-ec` (`d7fbbd8`) chargé avec `debug=1`.
> Outils : [`scripts/spike/ec-dump.sh`](../../scripts/spike/ec-dump.sh) (relevés) et
> [`scripts/spike/ec-decode.py`](../../scripts/spike/ec-decode.py) (décodage).

## Firmware

| Champ | Valeur | Adresse |
|---|---|---|
| Version EC | `14K2EMS1.110` | `0xa0`–`0xab` (ASCII) |
| Date firmware | `04/24/2026 15:22:03` | `0xac`–`0xbb` (ASCII `MMDDYYYYHH:MM:SS`) |

⚠️ `.110` **n'est pas** dans la liste de `msi-ec` (upstream et module intégré au noyau 7.3, qui refuse de se charger :
« Firmware version is not supported »). Les registres connus de `CONF_G2_5` (`.104`–`.109`) ont été vérifiés
cohérents sur `.110` (tableau ci-dessous) → le fork doit ajouter `14K2EMS1.110` à `ALLOWED_FW_G2_5`.

## Registres de `CONF_G2_5` vérifiés sur `.110`

| Fonction | Adresse | Valeur observée | Interprétation |
|---|---|---|---|
| Shift mode | `0xd2` | `0xc1` | comfort ✓ (mode Balanced Windows) |
| Fan mode | `0xd4` | `0x0d` | auto ✓ |
| Cooler Boost | `0x98` | `0x06` | bit 7 = 0 → off ✓ |
| Limite de charge | `0xd7` | `0xe4` | bit 7 = 1 (supporté), `0x64` = 100 % ✓ |
| Super battery | `0xeb` | `0x00` | off |
| Temp CPU temps réel | `0x68` | 38–100 | °C ✓ (suit la charge) |
| Ventilo CPU temps réel | `0x71` | 36–96 | % ✓ (suit la table) |
| Temp GPU temps réel | `0x80` | 48–70 | °C ✓ |
| Ventilo GPU temps réel | `0x89` | 36–62 | % ✓ |

## Tables de courbe (mode `auto`, valeurs d'usine)

7 vitesses, 6 seuils de montée, 6 seuils de descente (hystérésis), par ventilo. Fonctionnement **par paliers**.

| | Adresses | Palier 0 | 1 | 2 | 3 | 4 | 5 | 6 |
|---|---|---|---|---|---|---|---|---|
| CPU vitesse % | `0x72`–`0x78` | **0** | 36 | 40 | 50 | 62 | 80 | 96 |
| CPU monte à ≥ °C | `0x6a`–`0x6f` | – | 57 | 62 | 70 | 77 | 83 | 90 |
| CPU redescend < °C | `0x7a`–`0x7f` | – | 53 | 57 | 65 | 73 | 80 | 87 |
| GPU vitesse % | `0x8a`–`0x90` | **0** | 36 | 42 | 50 | 62 | 80 | 96 |
| GPU monte à ≥ °C | `0x82`–`0x87` | – | 50 | 60 | 65 | 70 | 78 | 83 |
| GPU redescend < °C | `0x92`–`0x97` | – | 45 | 55 | 63 | 67 | 75 | 80 |

Règle déduite : palier `i` (i ≥ 1) atteint quand T ≥ `up[i-1]`, quitté quand T < `down[i-1]`.

## Observations (test de charge CPU 100 %, 2 min, puis 2 min de repos)

- **Ventilos à 0 % sous 57 °C (CPU) / 50 °C (GPU)** : cause du « ils ne se lancent pas ».
- **Réaction en retard** : à 83 °C le CPU n'est qu'à 40 % (l'EC lisse la température), puis saute à 80 % → 96 % :
  cause du « ça chauffe puis à fond ».
- Le CPU atteint **100 °C** pendant ~1 min ventilos à 96 % (maximum de la table) → S4 (Cooler Boost ≥ 95 °C) aurait
  déclenché. RPM max relevé via `msi_wmi_platform` : 3870 (`fan1_input`, unité à confirmer).
- Hystérésis confirmée : retour de 96 % → 62 % → 50 % → 40 % en descendant (40 % maintenu jusqu'à ~61 °C).
- Octets « vivants » non identifiés (changent seuls) : `0xc9`, `0xcb`, `0xdd`, `0x9e`, `0xf4`, `0x46`, `0x4a`, `0xf7`.

## Conséquences pour le code

| Sujet | Décision |
|---|---|
| `CurveLayout` (`msi-core`) | 7 vitesses + 6 seuils montée + 6 seuils descente, au lieu de 7 points provisoires |
| Planchers S2 | S'appliquent au **palier effectif** ; le seuil de descente fixe la vitesse minimale pendant la redescente |
| Premier palier | Ne plus laisser 0 % : palier 0 ≥ 20–25 % (ventilo jamais arrêté) |
| Fork `msi-ec` | Ajouter `.110` ; attributs sysfs pour les 3 tables × 2 ventilos, avec validation noyau |

## Bascule des modes ventilo (autorisée par le propriétaire)

[`scripts/spike/fan-mode-cycle.sh`](../../scripts/spike/fan-mode-cycle.sh), msi-ec chargé avec
`firmware=14K2EMS1.109` (seule écriture : `fan_mode`, `auto` restauré en sortie).

| Mode | `0xd4` | Tables `0x6a`–`0x97` | Ventilo à ~61 °C |
|---|---|---|---|
| auto | `0x0d` ✓ | valeurs d'usine | 36 % |
| silent | `0x1d` ✓ | **inchangées** | 36 % |
| advanced | `0x8d` ✓ | **inchangées** | 36 % |
| auto (restauré) | `0x0d` ✓ | inchangées | 36 % |

- Les valeurs de `fan_mode` de `CONF_G2_5` sont confirmées sur `.110`.
- Changer de mode **ne réécrit pas** les tables : l'EC conserve une seule copie en RAM. Comme MSI Center, la courbe
  personnalisée = écrire les tables puis passer en `advanced`.

## Première écriture de table (2026-10-25, accord du propriétaire)

[`scripts/spike/curve-write-test.sh`](../../scripts/spike/curve-write-test.sh) : en `advanced`, vitesse du palier
actif (CPU palier 1, `0x73`) passée de 36 % à 30 % pendant 20 s, puis restaurée.

| | `0x73` | Ventilo (`0x71`) | RPM (`fan1_input`) |
|---|---|---|---|
| Table usine | 36 | 36 % | ~2200–2430 |
| Après écriture | **30** | **30 %** en < 2 s | **~2070** |
| Restauré, `auto` | 36 | palier 0 (CPU 53 °C) | 0 → arrêt progressif |

- ✅ **En `advanced`, l'EC applique la table en direct**, et relit la valeur écrite à l'identique.
- `fan1_input` est bien en tr/min (0 quand le palier 0 = 0 % est actif).
- Les bas régimes sont proches les uns des autres (36 % → 30 % = −130 tr/min) : le gain acoustique viendra de la
  forme de la courbe (démarrage précoce et doux), pas de vitesses très basses.
- La température instantanée `0x68` a des pics brefs (jusqu'à 90 °C au repos) : le daemon doit filtrer
  (S4 sur moyenne glissante ou 2 lectures consécutives) pour ne pas déclencher Cooler Boost à tort.

## Reste à vérifier

- [x] En `advanced`, une table écrite est-elle appliquée par l'EC ? → oui (ci-dessus)
- [ ] Les tables survivent-elles à un changement de shift mode, à la veille, au débranchement secteur ?
      (sinon le daemon réapplique la courbe sur ces événements)
