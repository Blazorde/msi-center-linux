# 0002 — Accès EC via un fork de `msi-ec`, jamais de reflash

- **Statut** : Accepté
- **Date** : 2026-10-25

## Contexte

Les ventilos sont mal gérés. L'idée initiale était de modifier le firmware. Le noyau CachyOS n'a pas `ec_sys`, et
`msi_wmi_platform` ne fait que lire les RPM. `msi-ec` upstream supporte déjà ce modèle (`14K2EMS1.*`) mais pas les
courbes.

## Décision

- **Aucune modification du firmware EC.** On écrit uniquement les registres RAM, comme MSI Center.
- Fork de `msi-ec` (`kernel/`), distribué en DKMS, ajoutant des attributs sysfs de courbe avec bornes vérifiées
  côté noyau. Le daemon n'accède à l'EC que via ces attributs.
- Patch minimal, rebasé régulièrement, proposé upstream à terme.

## Alternatives

| Option | Pour | Contre |
|---|---|---|
| Reflash firmware | « Définitif » | Risque de brick, firmware non documenté |
| `ec_sys write_support` / `/dev/port` | Rapide | Absent du noyau, aucune garde, accès brut root |
| `msi-ec` debug `ec_set` | Sans fork | Écriture arbitraire d'octets, interface de debug |
| Patchs `msi-wmi-platform` | Upstream | Non fusionnés (2026), ciblent les MSI Claw |

## Conséquences

Il faut maintenir un module C et un paquet DKMS. En contrepartie, la double validation (noyau + `msi-core`) rend
une écriture dangereuse très improbable.
