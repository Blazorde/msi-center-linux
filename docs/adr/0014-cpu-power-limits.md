# 0014 — Limites de puissance CPU (RAPL PL1 / PL2) par profil

- **Statut** : Proposé
- **Date** : 2026-10-28

## Contexte

Le firmware règle le Core Ultra 9 185H à **PL1 = 110 W, PL2 = 160 W** (fenêtre PL1 ≈ 56 s), sur les deux
interfaces RAPL (`intel-rapl:0`, MSR, et `intel-rapl-mmio:0`, MMIO), quel que soit le shift mode. Pour un 14 pouces,
c'est plus du double de la puissance de base Intel (45 W). Relevé du 2026-10-28, profil Équilibré, CPU à ~5 % :

- pics à **97–99 °C** à chaque rafale de turbo, S4 force le Cooler Boost plusieurs fois par heure ;
- au repos, la température filtrée reste vers 70–75 °C : la courbe du propriétaire tourne à 55–83 % en
  permanence, alors qu'elle est correcte ;
- essai A/B de 45 s : `powerprofilesctl set power-saver` → **max 60 °C au lieu de 77 °C**, moyenne −4 °C.

Les ventilateurs ne peuvent pas suivre une rafale de 160 W (inertie mécanique, filtre de température
d'ADR-0010) : il faut agir sur la chaleur produite, pas seulement sur son évacuation.

## Décision

Chaque profil porte des limites de puissance CPU (`ProfileTarget::cpu_power`, `msi-core::power_limit`) :

| Profil | PL1 | PL2 |
|---|---|---|
| Éco | 20 W | 35 W |
| Équilibré | 35 W | 60 W |
| Gaming | firmware (110 W) | firmware (160 W) |

- **Jamais au-dessus du firmware** : `PowerLimits::capped` borne chaque valeur à celle du firmware, et
  PL1 ≤ PL2. Une valeur fausse donne au pire un CPU plus lent, jamais plus chaud. Bornes S3 :
  15 W ≤ PL1 ≤ PL2 ≤ 250 W ; les préréglages sont vérifiés à la compilation.
- **Écriture** (`msi-daemon::rapl`) : les deux zones `package-0` (contraintes `long_term` / `short_term`),
  dans l'ordre qui garde PL1 ≤ PL2, puis relecture. Le CPU obéit à la plus basse des deux zones : si une seule
  zone accepte (MSR verrouillé), c'est suffisant, l'autre est journalisée.
- **Limites du firmware** lues une fois et gardées dans `/run/msi-center/rapl-firmware` (tmpfs) : un
  redémarrage du daemon ne prend pas nos limites abaissées pour celles du firmware ; un reboot repart de zéro.
- **Quand** : après l'EC dans `apply_profile` (y compris le profil de ventilation Auto), donc aussi au
  démarrage, après la veille et au changement secteur/batterie ; la vérification de dérive (toutes les 10 s)
  ré-applique si la limite en vigueur a changé (firmware, autre outil). Un échec n'est jamais fatal.
- **Arrêt normal** : limites du firmware restaurées (`Controller::shutdown`). Les chemins de crash (panic,
  `ExecStopPost`) ne restaurent pas : garder des limites plus basses est sûr.
- **D-Bus** : propriété `CpuPowerLimits` `(uu)` en W (ajout compatible) ; `msictl status` l'affiche.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Courbe ventilo plus agressive | Rien à écrire de plus | Ne coupe pas les pics (inertie), plus de bruit |
| `power-profiles-daemon` en `power-saver` | Déjà installé | EPP seulement, pas de borne en W ; ralentit aussi les tâches courtes ; non lié aux profils MSI |
| `thermald` ou un outil d'undervolt | Outils existants | Une dépendance de plus qui écrit RAPL en concurrence ; undervolt bloqué sur Meteor Lake |
| Limites configurables par l'utilisateur | Souplesse | Plus tard si besoin : il faut d'abord valider les préréglages sur la machine |

## Conséquences

- Équilibré : moins de pics, CPU nettement plus frais au repos, ventilos plus bas ; les tâches longues et
  lourdes plafonnent à 35 W (≈ la puissance de base Intel). Gaming garde toute la puissance.
- Une nouvelle écriture système (`/sys/class/powercap`), hors EC : les règles EC (S1–S9) ne changent pas.
- Si le firmware réécrit les limites (changement de shift mode, secteur), le daemon les ré-applique en ≤ 10 s.
