# MSI Stealth 14 AI Studio A1VGG

## Identification (relevé le 2026-10-25)

| Élément | Valeur |
|---|---|
| Produit / carte | Stealth 14 AI Studio A1VGG / MS-14K2 |
| BIOS | `E14K2IMS.10F` |
| Firmware EC | `14K2EMS1.110` (04/24/2026) — non listé par msi-ec, voir [ec-map.md](ec-map.md) |
| CPU | Intel Core Ultra 9 185H (Meteor Lake), `intel_pstate` |
| iGPU | Intel Arc (Meteor Lake-P) `8086:7d55` — `i915` / `xe` |
| dGPU | NVIDIA RTX 4070 Laptop (AD106M) `10de:2820`, PCI `0000:01:00.0`, pilote 615.78 |
| NPU | Intel NPU `8086:7d1d`, `intel_vpu`, `/dev/accel0` |
| Noyau | 7.2.11-1-cachyos |

## État du support Linux

| Fonction | Interface actuelle | Statut |
|---|---|---|
| RPM ventilos | `msi_wmi_platform` hwmon (`fan1_input`, `fan2_input`) | ✅ lecture seule |
| Contrôle ventilos | – | ❌ (ni `msi-ec`, ni `ec_sys`) |
| Profils plateforme | power-profiles-daemon, driver `placeholder` | ⚠️ CPU uniquement |
| dGPU runtime PM | `power/runtime_status` = `active`, 1,41 W au repos (P8) ; RTD3 désactivé par le pilote, voir [gpu-power](gpu-power.md) | ⚠️ à optimiser |

## Carte EC (d'après `msi-ec` upstream, config `CONF_G2_5`)

Source : [`msi-ec.c`](https://github.com/BeardOverflow/msi-ec), `ALLOWED_FW_G2_5` contient
`14K2EMS1.104/.107/.108/.109` (« Stealth 14 AI Studio A1VGG / A1VFG »).

| Fonction | Adresse | Valeurs |
|---|---|---|
| Shift mode | `0xd2` | turbo `0xc4` · eco `0xc2` · comfort `0xc1` |
| Fan mode | `0xd4` | auto `0x0d` · silent `0x1d` · advanced `0x8d` |
| Cooler Boost | `0x98` bit 7 | |
| Limite de charge | `0xd7` | |
| Super battery | `0xeb` masque `0x0f` | |
| Temp CPU / RPM CPU | `0x68` / `0x71` | temps réel |
| Temp GPU / RPM GPU | `0x80` / `0x89` | temps réel |
| Webcam | `0x2e` bit 1 (block `0x2f`) | |
| Fn ↔ Win | `0xe8` bit 4 | |
| Rétroéclairage clavier | non supporté | |

## Courbe ventilo — hypothèses à vérifier (phase 2)

Sur les MSI de cette génération, le mode `advanced` utilise des tables EC :

| Table | Adresse supposée | Source |
|---|---|---|
| Vitesses CPU (%) | ~`0x72`… | [GhostDeck #107](https://github.com/wygodad/ghostdeck/issues/107) (modèle frère A13V) |
| Vitesses GPU (%) | ~`0x8A`… | idem |
| Seuils de température | ~`0x6a`… / `0x82`… | à déterminer |

**Méthode du spike (lecture seule)** : installer `msi-ec` upstream, lire `fw_version`, dumper les 256 octets de
l'EC en mode auto / silent / advanced et à différentes températures, comparer, documenter dans `ec-map.md`.
Bonus : comparer avec un dump fait pendant que MSI Center (Windows) applique une courbe connue.
