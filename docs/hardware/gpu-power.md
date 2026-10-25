# Gestion d'alimentation GPU & sources de télémétrie

> Relevé **en lecture seule** sur la machine du propriétaire le 2026-10-25 (noyau `7.3.0-4-cachyos`, GNOME 51
> Wayland, pilote NVIDIA open `615.78.08`), en utilisateur normal, sans sudo. Aucune écriture dans `/sys`,
> `/proc`, `/dev` ou `/etc`. Sert de base à la phase 5 (gestionnaire GPU) et à la télémétrie du daemon.

## Objectif

| Profil | État dGPU visé | Mécanisme |
|---|---|---|
| Éco | **retiré du bus PCI** (`Removed`) | `remove` sur chaque fonction PCI du dGPU, `rescan` pour le rétablir |
| Balanced | **RTD3** quand inactif (`Suspended`) | `power/control = auto` + `NVreg_DynamicPowerManagement=0x02` |
| Gaming | actif (`Active`) | `power/control = on` (ou `auto` avec charge) |

Le tout **sans redémarrage**.

## dGPU : RTX 4070 Laptop (`10de:2820`)

### Topologie PCI

| Élément | Chemin | Valeur observée |
|---|---|---|
| Fonction GPU | `/sys/bus/pci/devices/0000:01:00.0` → `/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0` | `vendor=0x10de`, `class=0x030000`, pilote `nvidia` |
| Fonction audio HDMI | `/sys/bus/pci/devices/0000:01:00.1` | `class=0x040300`, pilote `snd_hda_intel` |
| Pont racine (parent) | `/sys/devices/pci0000:00/0000:00:01.0` | ACPI `\_SB_.PC00.RP12`, `power_resources_D3hot` → `LNXPOWER:01` (ressource `_PR3` présente → D3cold possible) |
| Nœuds DRM | `card0` (`/dev/dri/by-path/pci-0000:01:00.0-card`), `renderD129` | connecteurs `card0-HDMI-A-1`, `card0-DP-3`, `card0-eDP-2` câblés sur le dGPU |
| Nœuds NVIDIA | `/dev/nvidia0`, `/dev/nvidiactl`, `/dev/nvidia-modeset`, `/dev/nvidia-uvm`, `/dev/nvidia-uvm-tools` | `crw-rw-rw-` |

L'iGPU est `0000:00:02.0` (`card1`, `renderD128`, pilote `i915` ; `xe` est chargé mais non lié).

### État d'alimentation

| Fichier | GPU `01:00.0` | Audio `01:00.1` | Pont `00:01.0` |
|---|---|---|---|
| `power/control` | `auto` | `auto` | `auto` |
| `power/runtime_status` | **`active`** | `suspended` | `active` |
| `power/runtime_suspended_time` | `0` (jamais suspendu depuis le boot) | | |
| `power_state` | `D0` | `D3hot` | `D0` |
| `d3cold_allowed` | `1` | | `1` |
| `power/autosuspend_delay_ms` | `EIO` (non géré par le pilote) | | |

`/proc/driver/nvidia/gpus/0000:01:00.0/power` :

```
Runtime D3 status:          Disabled by default
Video Memory:               Active
GPU Hardware Support:
 Video Memory Self Refresh: Supported
 Video Memory Off:          Supported
S0ix Power Management:
 Platform Support:          Supported
 Status:                    Disabled
```

**Conclusion** : `power/control` est déjà `auto` mais le pilote NVIDIA a lui-même désactivé le RTD3 (« Disabled
by default »). Le matériel et la plateforme le supportent. C'est la cause des ~1,4 W au repos.

### Paramètres du module `nvidia`

| Paramètre (`/sys/module/nvidia/parameters/…`) | Valeur | Commentaire |
|---|---|---|
| `NVreg_DynamicPowerManagement` | `3` | valeur par défaut « auto » ; ici résolue en *désactivé* |
| `NVreg_DynamicPowerManagementVideoMemoryThreshold` | `200` (Mio) | |
| `NVreg_EnableS0ixPowerManagement` | `0` | |
| `NVreg_PreserveVideoMemoryAllocations` | `2` | |
| `NVreg_EnableGpuFirmware` | `18` | GSP actif |
| `nvidia_drm.modeset` / `fbdev` | `Y` / `Y` | |

`/etc/modprobe.d/` ne contient rien pour NVIDIA. `/usr/lib/modprobe.d/nvidia-utils.conf` blackliste
`nouveau`/`nova_*` et déclare `softdep nvidia post: nvidia-uvm nvidia-drm` ; `/usr/lib/modprobe.d/nvidia.conf`
met `NVreg_InitializeSystemMemoryAllocations=0`.

Compteurs de références : `nvidia` 48, `nvidia_drm` 3, `nvidia_modeset` 3, `nvidia_uvm` 0.

### Services

| Service | Activé | Actif |
|---|---|---|
| `nvidia-powerd` (Dynamic Boost) | oui | **oui** (pid 867, root) |
| `nvidia-persistenced` | non | non |
| `nvidia-suspend` / `-resume` / `-hibernate` | non | non |
| `switcheroo-control` | oui | oui |
| `power-profiles-daemon` | non (activé par D-Bus) | oui |
| `supergfxd` | absent | – |

### Qui tient le dGPU

Scan de `/proc/*/fd` en utilisateur : **395 processus sur 468 illisibles** (root, autres utilisateurs, dont
`nvidia-powerd`). Le daemon (root) verra tout. Parmi les processus lisibles :

| PID | Processus | Descripteurs ouverts sur le dGPU |
|---|---|---|
| 3876 | `gnome-shell` | `/dev/nvidia0` ×12, `/dev/nvidiactl` ×4, `/dev/nvidia-modeset` ×2, `/dev/dri/renderD129` ×2 |
| 4255 | `Xwayland` | `/dev/dri/card0` ×1 |
| 867 | `nvidia-powerd` | non lisible en utilisateur ; tient `/dev/nvidia*` par conception |

Journal de `gnome-shell` : `Added device '/dev/dri/card0' (nvidia-drm) using atomic mode setting`,
`Created gbm renderer for '/dev/dri/card0'`, `GPU /dev/dri/card1 selected primary from builtin panel presence`.
Mutter ouvre donc le dGPU comme **GPU secondaire** parce que des connecteurs (HDMI, DP-3, eDP-2) y sont câblés.
Le tag udev de `card0` est `:master-of-seat:seat:switcheroo-discrete-gpu:uaccess:` (pas de `mutter-device-ignore`).

## Faisabilité du retrait à chaud (profil Éco)

### Ce que fait supergfxctl (mode `Integrated`)

Lu dans le code source (`gitlab.com/asus-linux/supergfxctl`, `src/actions.rs`, `src/lib.rs`,
`src/pci_device.rs`). Séquence `Hybrid → Integrated` :

1. `WaitLogout` : attend que logind ne liste plus de session graphique (délai `logout_timeout_s`, 3 min) ;
2. `StopDisplayManager` : `systemctl stop display-manager.service` ;
3. `systemctl stop nvidia-persistenced.service` puis `nvidia-powerd.service` ;
4. `KillNvidia` : `lsof /dev/nvidia0` puis `kill` de chaque pid restant ;
5. `UnloadGpuDrivers` : `rmmod` de `nvidia_drm`, `nvidia_modeset`, `nvidia_uvm`, `nvidia`,
   `nvidia_wmi_ec_backlight` (avec réessais) ;
6. `UnbindRemoveGpu` : pour **chaque fonction** du dGPU (GPU, audio, USB-C/UCSI éventuels) écrit dans
   `driver/unbind` puis `1` dans `remove` ;
7. écrit `/etc/modprobe.d/supergfxd.conf` qui blackliste `nvidia*` (sinon un `rescan` ou un reboot recharge tout) ;
8. vérifie l'ICD Vulkan, `StartDisplayManager`.

Retour `Integrated → Hybrid` : réécriture du modprobe, `echo 1 > /sys/bus/pci/rescan`, `modprobe` des pilotes,
redémarrage de `nvidia-persistenced`/`nvidia-powerd`. supergfxctl **exige une déconnexion** pour Hybrid ↔
Integrated (`no_logind` permet de sauter la vérification, à vos risques).

### Pièges connus avec GNOME/mutter sous Wayland

- **mutter tient le dGPU** dès qu'il a des connecteurs (cas ici). Retirer le périphérique PCI sous un
  compositeur qui a un fd KMS/EGL ouvert dessus peut faire planter `gnome-shell` (= perte de la session).
  C'est la raison de la déconnexion obligatoire chez supergfxctl.
- Le tag udev `mutter-device-ignore` (règle `SUBSYSTEM=="drm", KERNEL=="card*", ATTRS{vendor}=="0x10de",
  TAG+="mutter-device-ignore"`) fait ignorer le dGPU par mutter. Effet de bord : les sorties HDMI/DP câblées sur
  le dGPU ne fonctionnent plus. Le tag n'est pris en compte qu'à l'ouverture de session (piège autologin).
- Toute application Vulkan/EGL/CUDA (navigateur, Steam, `nvidia-smi`, NVML) ouvre `/dev/nvidia*` au démarrage
  pour énumérer les GPU, même sans s'en servir.
- `nvidia-powerd` tient `/dev/nvidia*` en permanence : il faut l'arrêter avant le retrait.
- Un `rescan` global (par n'importe quel outil) ramène le dGPU et recharge le pilote via modalias.

### Verdict pour cette machine

| Bloqueur | Gravité | Contournement |
|---|---|---|
| `gnome-shell` (mutter) tient `/dev/nvidia0`, `renderD129` | **bloquant** | règle udev `mutter-device-ignore` + reconnexion une fois ; ou déconnexion à chaque passage en Éco |
| `Xwayland` tient `card0` | bloquant | disparaît avec la règle udev (Xwayland reçoit ses GPU de mutter) — à vérifier |
| `nvidia-powerd` tient `/dev/nvidia*` | bloquant | `systemctl stop nvidia-powerd` avant retrait, `start` après rescan |
| applications utilisateur | variable | liste affichée par le daemon (`GpuHolders`), refus typé |

Le retrait à chaud est donc **faisable** à condition que mutter ignore le dGPU. Le daemon refuse le retrait tant
qu'un processus tient le périphérique (erreur `GpuError::Busy` avec la liste pid + nom), sans jamais tuer de
processus (contrairement à supergfxctl). Les modules ne sont pas déchargés : `unbind` + `remove` suffisent à
couper le périphérique, et les garder chargés permet au `rescan` de relier automatiquement le pilote.

## Profil Balanced : RTD3

Le RTD3 est désactivé côté pilote. Après `NVreg_DynamicPowerManagement=0x02` (« fine-grained »), le GPU peut
passer en D3cold même avec des fd ouverts, **tant qu'aucun client ne lui soumet de travail ni ne l'interroge**.
Point d'attention : le GPU secondaire de mutter et `nvidia-powerd` peuvent encore empêcher la mise en veille ; à
mesurer après la modification (`power/runtime_status` doit passer à `suspended`, `runtime_suspended_time` croître).

## Sources de télémétrie

| Mesure | Source | Valeur observée | Coût / remarque |
|---|---|---|---|
| CPU % | `/proc/stat`, ligne `cpu` | `cpu 6847 260 3732 639874 55225 383 262 0 0 0` | 1 lecture |
| RAM % | `/proc/meminfo` `MemTotal`/`MemAvailable` | 32 322 140 / 28 803 132 kB | 1 lecture |
| iGPU % | `/proc/<pid>/fdinfo/<fd>` des fd DRM, `drm-pdev: 0000:00:02.0` | clés i915 : `drm-driver`, `drm-client-id`, `drm-engine-{render,copy,video,video-enhance,compute}` (ns), `drm-engine-capacity-video: 2` | scan complet des fd coûteux → mis en cache, re-scan toutes les 5 s |
| iGPU (xe) | idem, clés `drm-cycles-<engine>` / `drm-total-cycles-<engine>` | (non lié ici) | géré aussi |
| dGPU % | NVML (`libnvidia-ml.so.1` présent) | – | le fdinfo de `nvidia-drm` **n'a pas** de clés `drm-engine-*` |
| NPU % | `/sys/class/accel/accel0/device/npu_busy_time_us` (`0000:00:0b.0`, `intel_vpu`) | `0` ; `npu_max_frequency_mhz=1400`, `runtime_status=suspended` | lecture d'un compteur logiciel, ne réveille pas le NPU |
| Temp CPU | hwmon `coretemp` (`/sys/class/hwmon/hwmon7`), `temp1_label = Package id 0` | 56 °C | |
| Temp GPU | NVML uniquement (le pilote NVIDIA n'expose pas de hwmon) | – | `None` si le dGPU n'est pas actif |
| Ventilos | hwmon `msi_wmi_platform` (`hwmon4`), `fan1_input`, `fan2_input` | 2232 / 2211 RPM (`fan3/4 = 0`) | plus tard : `msi-ec` |
| Autres | `acpitz` 58 °C, zones `TCPU` 56 °C, `SEN2`/`SEN4` 49 °C, NVMe, SPD5118 | | non utilisés |

Les numéros `hwmonN` changent entre boots : le daemon cherche par `name`.

### Ne jamais réveiller le dGPU

Interroger NVML (comme `nvidia-smi`) réveille un GPU suspendu, et un sondage chaque seconde l'empêche de se
rendormir. Garanties du daemon :

1. avant chaque échantillon, lecture de `power/runtime_status` (attribut sysfs logiciel, sans effet sur le
   périphérique) : NVML n'est **initialisé et interrogé que si l'état est `Active`** ;
2. dès que l'état n'est plus `Active`, le handle NVML est **relâché** (fermeture des fd `/dev/nvidia*`), donc le
   daemon n'est jamais lui-même un « holder » qui bloquerait le retrait ;
3. politique par profil : en Balanced le sondage NVML est désactivé (`DgpuPolicy::Off`) car il empêcherait le
   RTD3 ; seul l'état (`active` / `suspended` / `removed`) est remonté. En Gaming : `WhenActive` ;
4. aucun accès à `/proc/driver/nvidia/…` ni à `/dev/nvidia*` dans la boucle de télémétrie.

## Actions propriétaire (root, non appliquées)

1. RTD3 : créer `/etc/modprobe.d/nvidia-pm.conf` avec `options nvidia NVreg_DynamicPowerManagement=0x02`,
   régénérer l'initramfs (`sudo mkinitcpio -P` si `nvidia` y est inclus), redémarrer, vérifier
   `Runtime D3 status: Enabled (fine-grained)`.
2. Éco : règle udev `/etc/udev/rules.d/61-mutter-ignore-nvidia.rules` avec
   `SUBSYSTEM=="drm", KERNEL=="card*", ATTRS{vendor}=="0x10de", TAG+="mutter-device-ignore"`, puis reconnexion.
   Conséquence : HDMI/DP du dGPU inutilisables tant que la règle est active.
3. Décider du sort de `nvidia-powerd` : le daemon devra l'arrêter avant Éco (et le relancer en Gaming).
4. Après 1 et 2 : vérifier avec `msi-daemon --probe` que `holders` est vide et `state = suspended` au repos.
