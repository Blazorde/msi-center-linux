# Installation

> Disponible à partir de la phase 9 (`v0.1.0`).

## Prévu

```bash
git clone git@github.com:Blazorde/msi-center-linux.git
cd msi-center-linux/packaging/arch
makepkg -si
```

Le paquet installera :

- le module `msi-ec` modifié (DKMS, recompilé automatiquement à chaque mise à jour du noyau) ;
- `msi-daemon` (service systemd système) et `clipd` (service utilisateur) ;
- l'application `msi-center` (dès maintenant, sans paquet : [`scripts/app-install.sh`](app.md)) ;
- l'extension GNOME (à activer : `gnome-extensions enable msi-center@blazorde`).

## Dès maintenant : installation de développement (démarrage automatique)

En attendant le paquet, [`scripts/dev-install.sh`](../../scripts/dev-install.sh) installe le daemon, `msictl` et
`clipd` depuis `target/release/` (manifeste dans `/var/lib/msi-center/dev-install.manifest`) :

```bash
cargo build --release -p msi-daemon -p clipd     # sans sudo
sudo scripts/dev-install.sh --enable              # installe + démarre à chaque démarrage / session
sudo scripts/dev-install.sh --uninstall           # défait tout ce que le manifeste liste
```

`--enable` (idempotent, à relancer après chaque reconstruction) :

1. installe `kernel/msi-ec` en DKMS (paquet `msi-ec-blazorde`, recompilé à chaque nouveau noyau) et vérifie que
   `modinfo -n msi_ec` pointe vers `updates/dkms/` (le module du noyau porte le même nom : il n'est **pas** mis en
   liste noire, `depmod` donne la priorité à `updates/`) ;
2. écrit `/etc/modules-load.d/msi-ec.conf` (chargement au démarrage) ;
3. active `msi-daemon.service` et installe son activation D-Bus système (sans `--enable`, aucun appel D-Bus ne
   peut démarrer le daemon) ;
4. si `clipd` est construit : binaire, unité utilisateur, activation D-Bus de session et
   `systemctl --global enable clipd` (chaque session).

Sans option, le script installe et redémarre le daemon une fois, sans démarrage automatique. L'app se lance à
l'ouverture de session via [`scripts/app-install.sh`](app.md).

## Désinstallation

`sudo pacman -Rns msi-center-linux` — à l'arrêt, le daemon réécrit les tables d'usine puis remet l'EC en mode
`auto` (S5).
