# Décisions d'architecture (ADR)

Une décision structurante = un fichier `NNNN-titre.md`, à partir de [`0000-template.md`](0000-template.md).
Un ADR accepté n'est pas modifié : on en crée un nouveau qui le remplace (`Superseded by`).

| # | Décision | Statut |
|---|---|---|
| [0001](0001-rust-monorepo.md) | Monorepo Rust (workspace Cargo) | Accepté |
| [0002](0002-ec-access-via-msi-ec-fork.md) | Accès EC via un fork de `msi-ec`, jamais de reflash | Accepté |
| [0003](0003-dbus-polkit-ipc.md) | IPC D-Bus (zbus) + polkit | Accepté |
| [0004](0004-gnome-extension-gjs.md) | Extension GNOME en GJS, minimale | Accepté |
| [0005](0005-clipboard-storage-split.md) | Presse-papiers : tmpfs pour la session, SQLite pour les épinglés | Accepté |
| [0006](0006-exponential-fan-curve.md) | Courbe ventilo exponentielle échantillonnée + planchers | Accepté |
| [0007](0007-local-ci.md) | CI locale (hooks + `scripts/ci.sh`) au lieu de GitHub Actions | Accepté |
| [0008](0008-dgpu-power-management.md) | dGPU : retrait PCI sans kill, NVML seulement si le dGPU est actif | Proposé |
| [0009](0009-step-curve-and-daemon-runtime.md) | Courbe EC par paliers ; daemon : secteur par sondage, veille via logind, `state.toml`, watchdog | Proposé |
| [0010](0010-hybrid-cpu-fan-control.md) | Ventilo CPU hybride : seuils d'usine, paliers bas modulés en direct par le daemon | Proposé |
| [0011](0011-gtk-app-runtime-and-glass-style.md) | App GTK : D-Bus sur un thread tokio, verre dépoli simulé sans sous-classe, rendu hors dGPU | Proposé |
| [0012](0012-fan-profiles.md) | Profils de ventilation : Auto persisté, choix retenu par profil, `SetFanCurve` = Personnalisé | Proposé |
| [0013](0013-staged-cpu-fan-curve.md) | Courbe à paliers du ventilo CPU : autant de paliers que souhaité, retombée propre à chaque palier, piloté en direct | Proposé |
