# Interface sysfs des courbes ventilo (fork `msi-ec`)

> Contrat entre le module noyau [`kernel/msi-ec`](../../kernel/msi-ec/) et `msi-daemon`. Toute évolution est un
> changement d'interface : mettre à jour ce document, le module et le daemon dans la même PR.
> Disposition EC mesurée : [carte EC](ec-map.md). Règles de sécurité : [fan-safety](fan-safety.md).

## Fichiers

Sous `/sys/devices/platform/msi-ec/cpu/` et `/sys/devices/platform/msi-ec/gpu/`, mode `0644` (lecture pour tous,
écriture root) :

| Fichier | Valeurs | Unité | Adresses EC CPU | Adresses EC GPU |
|---|---|---|---|---|
| `fan_curve_speeds` | 7 (paliers 0 à 6) | % | `0x72`–`0x78` | `0x8a`–`0x90` |
| `fan_curve_up_temps` | 6 | °C | `0x6a`–`0x6f` | `0x82`–`0x87` |
| `fan_curve_down_temps` | 6 | °C | `0x7a`–`0x7f` | `0x92`–`0x97` |

Sémantique (EC) : le palier `i` (i ≥ 1) est atteint quand T ≥ `up_temps[i-1]` et quitté quand
T < `down_temps[i-1]`. Le palier 0 s'applique en dessous de `up_temps[0]`.

## Format

- **Lecture** : entiers décimaux séparés par **un seul espace**, suivis d'un `\n`.
  Ex. : `0 36 40 50 62 80 96\n`.
- **Écriture** : même format, `\n` final facultatif. La table **complète** est obligatoire (7 ou 6 valeurs).
  Tout autre format (espace en trop, valeur manquante ou en trop, signe, hexadécimal, plus de 3 chiffres) →
  `EINVAL`.

## Validation (noyau)

Une écriture invalide est rejetée avec `EINVAL` et **rien n'est écrit** dans l'EC.

| Fichier | Règle |
|---|---|
| `fan_curve_speeds` | chaque valeur dans `0..100`, suite croissante ou égale |
| `fan_curve_up_temps` | chaque valeur dans `20..100`, suite strictement croissante |
| `fan_curve_down_temps` | chaque valeur dans `20..100`, suite strictement croissante |
| `up_temps` / `down_temps` | pour tout `i` : `down[i] < up[i]`, vérifié contre la valeur **actuelle** de l'autre table dans l'EC |

Conséquence pour le daemon : pour passer d'une courbe à une autre, choisir l'ordre d'écriture des deux tables de
températures pour que chaque étape intermédiaire respecte `down[i] < up[i]` (par ex. monter les seuils : écrire
`up` puis `down` ; les descendre : `down` puis `up`). Si aucun ordre ne convient, passer par une table
intermédiaire.

Ces règles sont un garde-fou noyau minimal (S3). Les planchers S2 et la cohérence de la courbe restent vérifiés
par `msi-core` avant tout envoi.

## Après écriture

- Chaque octet écrit est **relu** ; une divergence → `EIO` et message `pr_warn` dans `dmesg`
  (`msi_ec: fan curve: read back mismatch at 0x..: wrote .., read ..`). Le daemon applique alors S7 (retour
  `auto`).
- Une erreur d'accès EC pendant l'écriture renvoie l'erreur EC (la table peut être partiellement écrite :
  le daemon relit et applique S7).
- Les écritures (et lectures) de courbe sont **sérialisées** par un mutex du module.
- L'EC n'applique les tables écrites qu'en `fan_mode` `advanced` (`0xd4 = 0x8d`), en direct
  ([preuve](ec-map.md#première-écriture-de-table-2026-10-25-accord-du-propriétaire)). En `auto` / `silent`,
  elles restent en RAM sans effet garanti.

## Disponibilité (S8)

Les six fichiers n'existent que si :

1. une configuration supportée est chargée, **trouvée à partir de la version lue dans l'EC** : un chargement
   forcé avec le paramètre `firmware=` ou en `debug=1` sur firmware inconnu ne les crée pas ;
2. la configuration déclare les adresses des tables. Seule `CONF_G2_5` (dont `14K2EMS1.110`) le fait.

Absents → le daemon passe en lecture seule (pas de courbe personnalisée).
