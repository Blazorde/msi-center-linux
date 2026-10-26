# Roadmap

Source de vérité détaillée : le [PRD](../../.claude/PRPs/prds/msi-center-linux.prd.md). Cette page en est le résumé.

| # | Phase | Livrable | Dépend de | Statut |
|---|---|---|---|---|
| 1 | Fondations | Dépôt, workspace, docs, CI, workflow git | – | ✅ |
| 2 | Spike EC | Version firmware EC, carte des registres de courbe (lecture seule) | 1 | ✅ |
| 3 | Fork msi-ec | Attributs sysfs courbe ventilo + paquet DKMS | 2 | ✅ testé sur la machine |
| 4 | msi-core + msi-daemon | Courbe exponentielle, garde-fous, profils, limite de charge, D-Bus, CLI `msictl` — **MVP** | 3 | ✅ v0.1.0 (pilotage CPU hybride, profils ventilo) |
| 5 | Gestion GPU | RTD3, détection des process qui tiennent la 4070, retrait/remise PCI à chaud | 1 | 🟡 veille runtime OK ; retrait à chaud non testé (GNOME tient le dGPU) |
| 6 | clip-core + clipd | Stockage, tags, recherche, purge au logout | 1 | ✅ |
| 7 | Extension GNOME | Top bar configurable, menu profils, `Super+V` | 4, 5, 6 | ✅ v0.1.0 |
| 8 | App msi-center | Onglets Ventilos · Performance/GPU · Batterie · Presse-papiers · Réglages | 4, 5, 6 | ✅ v0.1.0 |
| 9 | Packaging | PKGBUILD, systemd, polkit, release `v0.1.0` | 7, 8 | ✅ v0.1.0 |

Les phases 2, 5 et 6 peuvent avancer en parallèle ; 7 et 8 aussi.

## Profils (validés)

| Profil | Mode EC (shift) | RTX 4070 | Courbe ventilo |
|---|---|---|---|
| 🔋 Éco batterie | `eco` | retirée du bus PCI | silencieuse |
| ⚖️ Balanced | `comfort` | veille auto (RTD3) | progressive |
| 🎮 Gaming/Perf | `turbo` | active | agressive |

Bascule automatique secteur ↔ batterie : activée par défaut, désactivable.

## Paramètres retenus

- Presse-papiers : `Super+V`, 500 éléments non épinglés max par session, images ≤ 10 Mo, tri par tags,
  pas d'exclusion des gestionnaires de mots de passe.
- Batterie : limite de charge réglable (défaut proposé 80 %).
- Top bar : chaque métrique activable dans les réglages.
