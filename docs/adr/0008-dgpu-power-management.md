# 0008 — dGPU : retrait PCI sans kill, télémétrie NVML conditionnée à l'état

- **Statut** : Proposé
- **Date** : 2026-10-25

## Contexte

Le profil Éco doit couper le RTX 4070 sans redémarrer, Balanced doit le laisser en RTD3, et la barre du haut
affiche l'usage dGPU. Sous GNOME Wayland, `gnome-shell` et `Xwayland` tiennent le dGPU ; `nvidia-powerd` aussi.
Interroger NVML réveille un GPU suspendu et, à 1 Hz, l'empêche de se rendormir. Détails :
[`docs/hardware/gpu-power.md`](../hardware/gpu-power.md).

## Décision

1. Éco = `driver/unbind` puis `remove` de chaque fonction PCI du dGPU ; retour par `/sys/bus/pci/rescan`. Les
   modules NVIDIA restent chargés (le rescan relie le pilote tout seul).
2. Le daemon **ne tue jamais** de processus : s'il reste un holder (scan de `/proc/*/fd`), un écran branché, ou
   des processus non inspectables, il refuse avec une erreur typée qui liste les holders.
3. Usage/température dGPU via NVML (`nvml-wrapper`, bibliothèque chargée à l'exécution), **uniquement** si
   `power/runtime_status` vaut `active` et si le profil l'autorise (Gaming). Sinon le handle NVML est relâché.
4. Toutes les I/O passent par une racine de système de fichiers injectable, testée sur des arbres factices.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Copier supergfxctl (logout, `kill`, `rmmod`, blacklist modprobe) | éprouvé | déconnexion obligatoire, tue des processus, écrit dans `/etc` |
| `nvidia-smi` en sous-processus | pas de dépendance | fork chaque seconde, réveille le GPU pareil |
| NVML en permanence | simple | empêche le RTD3, rend le daemon holder du dGPU |
| Pas d'usage dGPU | zéro risque | demande explicite du propriétaire |

## Conséquences

- Éco exige qu'aucune application ne tienne le dGPU : en pratique une règle udev `mutter-device-ignore`
  (action propriétaire) et l'arrêt de `nvidia-powerd` par le daemon.
- En Balanced, la barre affiche l'état du dGPU mais pas son pourcentage.
- Dépendance `nvml-wrapper` (MIT/Apache-2.0) ; le daemon démarre même sans `libnvidia-ml`.
