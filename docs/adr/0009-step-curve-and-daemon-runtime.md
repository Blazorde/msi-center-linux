# 0009 — Courbe par paliers, et fonctionnement du daemon (secteur, veille, réglages, watchdog)

- **Statut** : Proposé
- **Date** : 2026-10-25

## Contexte

Le spike EC ([ec-map.md](../hardware/ec-map.md)) a montré que la table EC n'est pas une liste de points : chaque
ventilo a 7 vitesses, 6 seuils de montée et 6 seuils de descente (hystérésis), et l'EC applique la table en
direct en fan mode `advanced`. L'ADR-0006 (courbe exponentielle échantillonnée + planchers) reste valable mais
supposait une disposition provisoire de 7 points. Le daemon (phase 4) doit aussi réagir au branchement secteur et à
la sortie de veille, retenir les réglages du propriétaire et survivre à un blocage.

## Décision

1. **Modèle par paliers** (`msi_core::table::StepCurve`) : une courbe = paramètres exponentiels + vitesse de repos +
   6 seuils de montée + hystérésis ; `speeds[i] = f(up[i−1])`, `down = up − hystérésis`. Les planchers S2 se
   contrôlent sur le palier **montant** (le plus bas possible). Presets recalibrés, dont un vrai mode
   **Silencieux** ([fan-safety.md](../hardware/fan-safety.md)).
2. **Écritures ordonnées** (`write_plan`) pour respecter `down[i] < up[i]` à chaque écriture noyau, avec
   relecture systématique (S7).
3. **Secteur** : lecture de `/sys/class/power_supply/ADP1/online` dans la boucle de 1 s déjà nécessaire à S4.
4. **Veille** : signal logind `PrepareForSleep(false)` → ré-application idempotente ; en plus, contrôle toutes les
   10 s que l'EC détient toujours nos tables (sinon ré-application, `SafetyEvent ec_reset`).
5. **Réglages** : `/var/lib/msi-center/state.toml` (`serde` + `toml`), validés par `msi-core` au chargement,
   écriture atomique.
6. **Watchdog** : client `sd_notify` minimal en Rust sûr (`UnixDatagram`), `WATCHDOG=1` après chaque tick réussi.
7. **`msictl`** : second binaire du paquet `msi-daemon`, qui réutilise le proxy D-Bus testé contre l'interface réelle.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| udev (crate `udev`) pour le secteur | événementiel | FFI libudev, une tâche de plus, gain nul : la boucle de 1 s existe déjà |
| Détecter la veille par saut d'horloge | aucune dépendance au bus | `CLOCK_BOOTTIME` absent de std, fragile |
| Crate `sd-notify` | maintenue | 40 lignes suffisent, testées, une dépendance de moins |
| Fichier de réglages maison (`clé=valeur`) | aucune dépendance | réinventer l'échappement et les tables imbriquées des courbes |
| Crate `msictl` séparée | séparation nette | duplication du proxy ou crate client de plus, pour un binaire de 300 lignes |

## Conséquences

- Nouvelles dépendances : `serde`, `toml` (MIT/Apache-2.0).
- Latence de bascule secteur ≤ 1 s ; coût : une lecture sysfs par seconde.
- Une table perdue par l'EC (veille, reset) est réécrite en ≤ 10 s même si logind n'est pas joignable.
