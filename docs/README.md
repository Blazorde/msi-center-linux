# Documentation — sommaire

> Cette arborescence est pensée pour être publiée telle quelle en wiki GitHub (`docs/README.md` = page d'accueil).
> Les docs humaines sont en français. Le guide agent est [`CLAUDE.md`](../CLAUDE.md) (anglais).

## 🎯 Produit
- [Roadmap & phases](product/roadmap.md)
- [PRD (spécification produit)](../.claude/PRPs/prds/msi-center-linux.prd.md)

## 🏗️ Architecture
- [Vue d'ensemble](architecture/overview.md)
- [API D-Bus](architecture/dbus-api.md)
- [Décisions d'architecture (ADR)](adr/README.md)

## 🔧 Matériel
- [MSI Stealth 14 AI Studio A1VGG](hardware/stealth-14-a1vgg.md)
- [Carte EC (firmware 14K2EMS1.110)](hardware/ec-map.md)
- [Sécurité ventilateurs & thermique](hardware/fan-safety.md)
- [Interface sysfs des courbes ventilo (fork msi-ec)](hardware/fan-curve-sysfs.md)
- [Alimentation GPU & sources de télémétrie](hardware/gpu-power.md)

## 👩‍💻 Développement
- [Installer l'environnement](development/setup.md)
- [Workflow git, branches, CI, releases](development/git-workflow.md)
- [Contribuer](../CONTRIBUTING.md)

## 📖 Utilisateur
- [Installation](user/installation.md)
- [L'application MSI Center](user/app.md)
- [Design de l'app (Liquid Glass × Golden Gate, captures)](design/app-liquid-glass.md)
- [Extension GNOME (barre, menu, `Super+V`)](user/extension.md)
