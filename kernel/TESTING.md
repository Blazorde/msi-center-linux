# Test du fork msi-ec par le propriétaire

> À exécuter **à la main** par le propriétaire (les agents ne chargent pas de module et n'écrivent pas dans `/sys`).
> Contrat testé : [`docs/hardware/fan-curve-sysfs.md`](../docs/hardware/fan-curve-sysfs.md).
> Durée : ~5 min. Fenêtre de risque : la courbe de test ne diffère de l'usine que d'un palier (36 → 30 %).

**Version automatisée** : `scripts/kernel-build.sh && sudo kernel/test-fan-curve.sh` (étapes ci-dessous, restauration
garantie).

**Résultat 2026-10-25 (A1VGG, fw `14K2EMS1.110`, noyau 7.3.0-4-cachyos)** : module `0.13-blazorde.1` chargé sans
forçage, 6 tables lues = valeurs d'usine, 6 écritures invalides refusées sans effet, écriture + relecture OK,
restauration OK. ✅

Toutes les commandes partent de la racine du dépôt. `D=/sys/devices/platform/msi-ec`.

## 1. Construire

```bash
scripts/kernel-build.sh
```

Attendu : dernière ligne `✓ target/kernel/msi-ec/msi-ec.ko built for 7.3.0-4-cachyos (LLVM=1)`, aucune ligne
`: warning: `.

## 2. Charger le module (sans `debug`, sans `firmware=`)

```bash
sudo rmmod msi_ec 2>/dev/null; lsmod | grep msi_ec   # rien : aucun msi_ec chargé
sudo insmod target/kernel/msi-ec/msi-ec.ko
cat /sys/module/msi_ec/version
D=/sys/devices/platform/msi-ec
cat $D/fw_version
sudo dmesg | grep msi_ec | tail -3
```

Attendu :

```
0.13-blazorde.1
14K2EMS1.110
msi_ec: module_init
```

Pas de `Your firmware version is not supported!` ni de `fan curve attributes disabled`.

## 3. Lire les 6 tables

```bash
for f in cpu gpu; do for t in speeds up_temps down_temps; do
  printf '%s/%-20s ' $f fan_curve_$t; cat $D/$f/fan_curve_$t; done; done
ls -l $D/cpu/fan_curve_* $D/gpu/fan_curve_*
cat $D/fan_mode
```

Attendu (valeurs d'usine) :

```
cpu/fan_curve_speeds     0 36 40 50 62 80 96
cpu/fan_curve_up_temps   57 62 70 77 83 90
cpu/fan_curve_down_temps 53 57 65 73 80 87
gpu/fan_curve_speeds     0 36 42 50 62 80 96
gpu/fan_curve_up_temps   50 60 65 70 78 83
gpu/fan_curve_down_temps 45 55 63 67 75 80
```

Les six fichiers en `-rw-r--r-- root root`. `fan_mode` = `auto`.

## 4. Rejets (rien ne doit être écrit)

```bash
echo "0 36 40 50 62 80"        | sudo tee $D/cpu/fan_curve_speeds     # 6 valeurs au lieu de 7
echo "0 36 35 50 62 80 96"     | sudo tee $D/cpu/fan_curve_speeds     # décroissant
echo "0 36 40 50 62 80 101"    | sudo tee $D/cpu/fan_curve_speeds     # > 100
echo "0  36 40 50 62 80 96"    | sudo tee $D/cpu/fan_curve_speeds     # double espace
echo "57 62 62 77 83 90"       | sudo tee $D/cpu/fan_curve_up_temps   # non strictement croissant
echo "50 62 70 77 83 90"       | sudo tee $D/cpu/fan_curve_up_temps   # up[0]=50 <= down[0]=53 (règle croisée)
cat $D/cpu/fan_curve_speeds $D/cpu/fan_curve_up_temps
```

Attendu : chaque `tee` affiche `tee: /sys/devices/platform/msi-ec/...: Invalid argument`, et les deux `cat`
renvoient toujours `0 36 40 50 62 80 96` et `57 62 70 77 83 90`.

## 5. Courbe de test : usine sauf CPU palier 1 = 30 %

Le palier 1 est actif entre 57 °C (montée) et 53 °C (descente). Au repos le CPU est souvent au palier 0 (0 %) :
si `cpu/realtime_temperature` < 57, lancer une petite charge (`stress-ng --cpu 2 -t 60s` ou une compilation)
jusqu'à ce que le ventilo tourne à 36 %.

```bash
cat $D/cpu/realtime_temperature $D/cpu/realtime_fan_speed     # ex. 60 / 36
echo "0 30 40 50 62 80 96" | sudo tee $D/cpu/fan_curve_speeds
cat $D/cpu/fan_curve_speeds
echo advanced | sudo tee $D/fan_mode
for i in $(seq 10); do printf '%s °C  %s %%\n' "$(cat $D/cpu/realtime_temperature)" "$(cat $D/cpu/realtime_fan_speed)"; sleep 2; done
```

Attendu : `0 30 40 50 62 80 96` relu ; tant que le CPU est au palier 1 (53–62 °C), `realtime_fan_speed` passe
à **30** en moins de 2 s (au palier 2+ il suit la table : 40, 50…). Aucun `read back mismatch` dans
`sudo dmesg | grep msi_ec`.

## 6. Restaurer l'usine et le mode `auto`

```bash
echo "0 36 40 50 62 80 96" | sudo tee $D/cpu/fan_curve_speeds
echo auto | sudo tee $D/fan_mode
cat $D/cpu/fan_curve_speeds $D/fan_mode $D/cpu/realtime_fan_speed
```

Attendu : `0 36 40 50 62 80 96`, `auto`, vitesse de nouveau celle de la table d'usine (36 % au palier 1).

## 7. Décharger (facultatif)

```bash
sudo rmmod msi_ec
```

En cas de doute à n'importe quelle étape : `echo auto | sudo tee $D/fan_mode`, puis `sudo rmmod msi_ec`
(l'EC garde la dernière table écrite jusqu'au prochain redémarrage ou réécriture ; l'étape 6 la remet à l'usine).

Reporter dans la PR : la sortie des étapes 2, 3 et 5 et `sudo dmesg | grep msi_ec`.
