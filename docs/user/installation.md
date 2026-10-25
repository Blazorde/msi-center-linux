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
- l'application `msi-center` ;
- l'extension GNOME (à activer : `gnome-extensions enable msi-center@blazorde`).

## Désinstallation

`sudo pacman -Rns msi-center-linux` — le daemon remet l'EC en mode `auto` à l'arrêt.
