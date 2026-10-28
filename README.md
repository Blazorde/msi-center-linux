# MSI Center Linux

Remplaçant natif de MSI Center pour le **MSI Stealth 14 AI Studio A1VGG** sous Linux (CachyOS, GNOME Wayland),
plus un gestionnaire de presse-papiers avec historique, épinglage et tags.

> ⚠️ Projet en phase de fondations : rien n'écrit encore dans l'EC. Voir la [roadmap](docs/product/roadmap.md).

## Fonctionnalités prévues

| Domaine | Ce que ça fait |
|---|---|
| 🌀 Ventilateurs | Courbe exponentielle CPU/GPU écrite dans l'EC, garde-fous thermiques, Cooler Boost |
| ⚡ Profils | 🔋 Éco · ⚖️ Balanced · 🎮 Gaming, sans redémarrage, bascule auto secteur/batterie |
| 🎮 GPU | Extinction de la RTX 4070 à chaud (Éco), mise en veille auto (RTD3) en Balanced |
| 🔋 Batterie | Limite de charge pour la longévité |
| 📊 Top bar | CPU / GPU / iGPU / RAM / NPU en %, températures, RPM — chaque valeur activable |
| 📋 Presse-papiers | Texte + images, `Super+V`, épingler, tags, recherche ; non épinglés effacés au logout |

## Architecture en bref

```
GNOME Shell extension (GJS)          msi-center (GTK4/libadwaita, Rust)
  top bar · Super+V · capture          fenêtre complète
            └──────────────┬─────────────────┘
                     D-Bus │  system : org.blazorde.MsiCenter1
                           │  session: org.blazorde.Clipd1
            ┌──────────────┴─────────────────┐
  msi-daemon (root, Rust)              clipd (user, Rust)
            │ sysfs                          │ tmpfs + SQLite
  msi-ec fork (DKMS, C)                $XDG_RUNTIME_DIR · $XDG_DATA_HOME
            │
  EC (contrôleur embarqué)
```

Détails : [docs/architecture/overview.md](docs/architecture/overview.md).

## Documentation

Tout est dans le [wiki](https://github.com/Blazorde/msi-center-linux/wiki), généré depuis [`docs/`](docs/README.md)
(`scripts/wiki-sync.sh`).

## Développement

```bash
git config core.hooksPath .githooks   # une fois : CI locale au push, Conventional Commits au commit
cargo build --workspace
scripts/ci.sh                         # fmt, clippy, tests, cargo deny
```

Workflow git, conventions et CI : [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

[GPL-3.0-or-later](LICENSE).
